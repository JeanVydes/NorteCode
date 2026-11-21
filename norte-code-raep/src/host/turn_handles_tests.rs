//! Turn tasks are released when they finish or their stream closes.

use super::{TurnHandles, TurnKey};

fn key(session: &str, id: &str) -> TurnKey {
    TurnKey::new(session, raep_types::FrameId::new(id).expect("frame id"))
}

#[tokio::test]
async fn finishing_a_turn_removes_only_its_handle() {
    let handles = TurnHandles::default();
    let first = key("session", "00000000000000000000000000000001");
    let second = key("session", "00000000000000000000000000000002");
    handles.insert(first.clone(), tokio::spawn(std::future::pending()));
    handles.insert(second, tokio::spawn(std::future::pending()));

    handles.finish(&first);

    assert_eq!(handles.len(), 1);
}

#[tokio::test]
async fn closing_a_stream_drains_only_its_sessions_handles() {
    let handles = TurnHandles::default();
    handles.insert(
        key("one", "00000000000000000000000000000001"),
        tokio::spawn(std::future::pending()),
    );
    handles.insert(
        key("two", "00000000000000000000000000000002"),
        tokio::spawn(std::future::pending()),
    );

    let drained = handles.drain_session("one");

    assert_eq!(drained.len(), 1);
    assert_eq!(handles.len(), 1);
}
