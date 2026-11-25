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

#[tokio::test]
async fn clearing_waits_until_every_turn_task_is_cancelled() {
    let handles = TurnHandles::default();
    let (stopped_tx, mut stopped_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let _guard = DropSignal(stopped_tx);
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    handles.insert(key("session", "00000000000000000000000000000001"), task);

    handles.abort_all().await;

    assert_eq!(stopped_rx.try_recv(), Ok(()));
    assert_eq!(handles.len(), 0);
}

struct DropSignal(tokio::sync::mpsc::UnboundedSender<()>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}
