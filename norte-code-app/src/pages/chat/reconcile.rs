//! One part of the conversation page.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

/// The usage each turn reported, keyed by turn id, as measured live.
pub(super) fn usage_by_turn(
    context: &AppContext,
) -> std::collections::HashMap<String, crate::domain::TurnUsage> {
    let mut measured = std::collections::HashMap::new();
    for item in context.chat_items.read().iter() {
        if let (Some(turn), Some(usage)) = (&item.turn_id, &item.usage) {
            measured
                .entry(turn.clone())
                .and_modify(|existing: &mut crate::domain::TurnUsage| existing.merge(usage))
                .or_insert_with(|| usage.clone());
        }
    }
    measured
}

/// Joins the server's canonical history with what the live stream built.
///
/// History is the source of record for what was said and which tools ran; the
/// stream is the source for what history cannot carry, the measured cost and
/// speed of each turn. Empty items are dropped: a tool call the stream opened
/// and history never got (it was cancelled) would otherwise render as a blank
/// row.
/// The live answer rows history has not committed.
///
/// Committed rows are matched by content, not by turn id: the stream names a
/// turn by the id this page minted or by the service's number, and history
/// names it `turn_N`, so the two never compare equal. A live bubble is kept
/// only when it is newer than everything committed and says something history
/// does not, that is the answer still being written.
pub(super) fn live_answers_missing_from(
    merged: &[ConversationItem],
    live: Vec<ConversationItem>,
) -> Vec<ConversationItem> {
    let newest_committed = merged.iter().map(|item| item.created_at).max().unwrap_or(0);
    let committed_text: std::collections::HashSet<&str> = merged
        .iter()
        .filter(|item| matches!(item.kind, HistoryKind::Assistant | HistoryKind::Reasoning))
        .map(|item| item.content.as_str())
        .collect();
    live.into_iter()
        .filter(|item| {
            matches!(item.kind, HistoryKind::Assistant | HistoryKind::Reasoning)
                && item.id.starts_with("stream:")
                && item.created_at > newest_committed
                && !committed_text.contains(item.content.as_str())
        })
        .collect()
}

/// The live tool rows whose call history does not carry.
///
/// History names a call only by the text it replayed, `name {json}`, with no
/// id and no live activity, so the two sides are matched by tool name within
/// the same turn. A call in flight today is rendered as a live row; the same
/// call after the answer lands is a history row, and carrying both would show
/// it twice: once as it was and once as it is.
pub(super) fn live_tools_missing_from(
    merged: &[ConversationItem],
    live: Vec<ConversationItem>,
) -> Vec<ConversationItem> {
    live.into_iter()
        .filter(|item| {
            if !matches!(item.kind, HistoryKind::ToolCall | HistoryKind::ToolResult) {
                return false;
            }
            let Some(activity) = &item.tool else {
                return false;
            };
            !merged.iter().any(|historical| {
                historical.kind == HistoryKind::ToolCall
                    && historical.turn_id == item.turn_id
                    && historical
                        .content
                        .trim_start()
                        .split([' ', '{'])
                        .next()
                        .is_some_and(|name| name == activity.name)
            })
        })
        .collect()
}

pub(super) fn reconcile(
    fresh: Vec<ConversationItem>,
    context: &AppContext,
    measured: &std::collections::HashMap<String, crate::domain::TurnUsage>,
) -> Vec<ConversationItem> {
    let live = context.chat_items.read().clone();
    // History is the record of what happened, tools included. It used to be
    // stripped of every tool row on the theory that the live stream had them -
    // so the calls vanished from the transcript the moment the answer landed.
    // What history cannot carry is the *output* a live stream showed while the
    // call ran, and empty text: those two things are taken from the live side.
    let mut merged: Vec<ConversationItem> = fresh
        .into_iter()
        .filter(|item| item.usage.is_some() || !item.content.trim().is_empty())
        .collect();
    // A tool row the stream opened but history has not committed yet (the
    // turn is still running) stays where it is.
    merged.extend(live_tools_missing_from(&merged, live.clone()));
    // The same for the answer itself. A history read can land while the next
    // turn is already printing, the end of one turn and the start of the
    // next are milliseconds apart, and replacing the transcript with a
    // snapshot that predates those deltas erased the answer until a reload.
    // A live bubble whose turn history does not carry is kept, in its place.
    merged.extend(live_answers_missing_from(&merged, live));
    // The measurement belongs to the answer the turn produced, not to every
    // row the turn touched: it was being attached to the user's message and to
    // the reasoning block as well, so one request's cost printed three times.
    for item in &mut merged {
        if item.kind != HistoryKind::Assistant {
            item.usage = None;
            continue;
        }
        if let Some(turn) = &item.turn_id {
            item.usage = measured.get(turn).cloned();
        }
    }
    merged.sort_by_key(|item| item.created_at);
    merged
}
