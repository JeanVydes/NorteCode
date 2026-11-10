use norte_code_wire::ServerEvent;

#[allow(clippy::wildcard_imports)]
use crate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeqStep {
    Apply,
    Duplicate,
    Gap { expected: u64 },
}

pub(crate) fn seq_step(previous: Option<u64>, seq: u64) -> SeqStep {
    match previous {
        Some(previous) if seq <= previous => SeqStep::Duplicate,
        Some(previous) if seq > previous.saturating_add(1) => SeqStep::Gap {
            expected: previous.saturating_add(1),
        },
        _ => SeqStep::Apply,
    }
}

/// Drains complete SSE blocks from `buffer`, in wire order.
///
/// Each complete block becomes one event; a trailing partial block stays in
/// the buffer for the next chunk. Blocks that are not `data:` frames or
/// that do not decode are skipped, so a keepalive or a foreign event never
/// wedges the stream. Terminal events (`TurnFinished`, `TurnStopped`) are
/// flagged but do not stop the drain: events the server sent after the
/// terminal in the same flush must still reach the caller.
pub(crate) fn drain_sse_buffer(
    buffer: &mut Vec<u8>,
) -> Vec<(Option<ServerEvent>, bool, Option<u64>)> {
    let mut events = Vec::new();
    while let Some(end) = buffer.windows(2).position(|window| window == b"\n\n") {
        let block = buffer[..end].to_vec();
        buffer.drain(..end + 2);
        let Ok(block) = std::str::from_utf8(&block) else {
            continue;
        };
        let Some(data) = block.lines().find_map(|line| line.strip_prefix("data: ")) else {
            continue;
        };
        // The SSE boundary is bytes: the frame is decoded to a typed
        // `FrameWire` here, at the edge, so nothing downstream sees `Value`.
        let Ok(frame) = serde_json::from_str::<FrameWire>(data) else {
            continue;
        };
        let frame = frame.into_envelope();
        let seq = frame.seq;
        let event = server_event_from_frame(&frame).ok();
        let terminal = event.as_ref().is_some_and(|event| {
            matches!(
                event.kind,
                norte_code_wire::ServerEventKind::TurnFinished { .. }
                    | norte_code_wire::ServerEventKind::TurnStopped { .. }
            )
        });
        events.push((event, terminal, seq));
    }
    events
}
