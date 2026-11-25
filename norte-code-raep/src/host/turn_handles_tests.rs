//! Turn tasks are released when they finish.

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
