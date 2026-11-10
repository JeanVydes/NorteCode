#[cfg(feature = "native")]
use norte_code_wire::ServerEvent;

use crate::decode::{DecodedEvent, decode_frame};

#[cfg(feature = "native")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeqStep {
    Apply,
    Duplicate,
    Gap { expected: u64 },
}

#[cfg(feature = "native")]
pub(crate) fn seq_step(previous: Option<u64>, seq: u64) -> SeqStep {
    match previous {
        Some(previous) if seq <= previous => SeqStep::Duplicate,
        Some(previous) if seq > previous.saturating_add(1) => SeqStep::Gap {
            expected: previous.saturating_add(1),
        },
        _ => SeqStep::Apply,
    }
}

/// The offset of the next complete SSE block and the length of its delimiter.
///
/// Both `\n\n` and `\r\n\r\n` are boundaries; the earliest one wins.
fn next_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2));
    let crlf = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4));
    match (lf, crlf) {
        (Some(lf), Some(crlf)) => Some(if lf.0 <= crlf.0 { lf } else { crlf }),
        (Some(found), None) | (None, Some(found)) => Some(found),
        (None, None) => None,
    }
}

/// The envelope fields this reader fills, with defaults for the ones the spec
/// makes optional or the host omits on canonical-only frames.
#[derive(serde::Deserialize)]
struct FrameIn {
    #[serde(default)]
    version: String,
    #[serde(default)]
    id: Option<raep_types::FrameId>,
    #[serde(default)]
    seq: Option<u64>,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    turn_id: Option<raep_types::TurnId>,
    #[serde(default)]
    job_id: Option<raep_types::JobId>,
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    kind: Option<raep_types::FrameType>,
    #[serde(default)]
    payload: serde_json::Value,
    #[serde(default)]
    ext: Option<serde_json::Value>,
}

impl FrameIn {
    fn into_envelope(self) -> raep_types::Envelope<serde_json::Value> {
        raep_types::Envelope {
            version: if self.version.is_empty() {
                raep_types::WIRE_VERSION.to_owned()
            } else {
                self.version
            },
            id: self.id.unwrap_or_else(raep_types::FrameId::generate),
            seq: self.seq,
            reference: None,
            session_id: self.session_id,
            turn_id: self.turn_id,
            job_id: self.job_id,
            timestamp: self.timestamp,
            // A canonical-only block names its event in the payload, not in
            // the envelope: that is a valid frame, not a broken one.
            kind: self
                .kind
                .unwrap_or_else(|| raep_types::FrameType::Extension("canonical".to_owned())),
            payload: self.payload,
            ext: self.ext,
        }
    }
}

/// Drains every complete SSE block from `buffer` into decoded events.
///
/// A trailing partial block stays in the buffer for the next chunk. Blocks
/// that carry no `data:` field (keepalives, comments) and blocks that do not
/// decode are skipped, so a foreign event never wedges the stream. This is
/// the one SSE reader: every client feeds it bytes and receives protocol
/// events, so parsing cannot drift between clients.
pub fn take_sse_events(buffer: &mut Vec<u8>) -> Vec<DecodedEvent> {
    let mut events = Vec::new();
    while let Some((end, delimiter)) = next_boundary(buffer) {
        let block: Vec<u8> = buffer.drain(..end).collect();
        buffer.drain(..delimiter);
        let Ok(block) = std::str::from_utf8(&block) else {
            continue;
        };
        let data = block
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|line| line.strip_prefix(' ').unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let Ok(frame) = serde_json::from_str::<FrameIn>(&data) else {
            continue;
        };
        if let Ok(decoded) = decode_frame(&frame.into_envelope()) {
            events.push(decoded);
        }
    }
    events
}

/// Drains SSE blocks for the HTTP adapter, flagging terminal events.
///
/// Events after a terminal in the same flush must still reach the caller, so
/// the drain never stops at one.
#[cfg(feature = "native")]
pub(crate) fn drain_sse_buffer(
    buffer: &mut Vec<u8>,
) -> Vec<(Option<ServerEvent>, bool, Option<u64>)> {
    take_sse_events(buffer)
        .into_iter()
        .map(|decoded| {
            let terminal = matches!(
                decoded.event.kind,
                norte_code_wire::ServerEventKind::TurnFinished { .. }
                    | norte_code_wire::ServerEventKind::TurnStopped { .. }
            );
            (Some(decoded.event), terminal, decoded.seq)
        })
        .collect()
}
