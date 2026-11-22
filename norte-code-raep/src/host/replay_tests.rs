use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};

use super::ReplayLog;

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn a_poisoned_replay_lock_still_serves_reads_and_writes() {
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "norte-replay-poison-{}-{sequence}",
        std::process::id()
    ));
    let log = ReplayLog::new(root, 1024);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _guard = log.lock.lock().expect("first lock");
        panic!("poison the replay lock");
    }));
    assert!(panic.is_err());

    assert_eq!(log.after("session", 0).frames.len(), 0);
    assert_eq!(log.last_seq("session"), None);
    assert_eq!(log.total(), (0, 0));
    log.remove("session");
}
