use std::sync::{Arc, Barrier, Mutex};

use super::run_at;

#[test]
fn failed_terminal_still_stops_service_when_no_client_remains() {
    let root = std::env::temp_dir().join(format!("norte-lifecycle-error-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let stopped = std::cell::Cell::new(false);
    let result = run_at(
        &root,
        |_| Ok(()),
        || Err(std::io::Error::other("terminal")),
        || {
            stopped.set(true);
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(stopped.get());
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn two_clients_keep_service_until_the_last_one_exits() {
    let root = std::env::temp_dir().join(format!("norte-lifecycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let first_ready = Arc::new(Barrier::new(2));
    let second_ready = Arc::new(Barrier::new(2));
    let first_close = Arc::new(Barrier::new(2));
    let events = Arc::new(Mutex::new(Vec::new()));
    let first = {
        let root = root.clone();
        let ready = first_ready.clone();
        let close = first_close.clone();
        let events = events.clone();
        std::thread::spawn(move || {
            run_at(
                &root,
                |active| {
                    assert!(!active);
                    Ok(())
                },
                || {
                    ready.wait();
                    close.wait();
                    Ok(())
                },
                || {
                    events.lock().expect("events").push("first stopped");
                    Ok(())
                },
            )
            .expect("first");
        })
    };
    first_ready.wait();
    let second = {
        let root = root.clone();
        let ready = second_ready.clone();
        let events = events.clone();
        std::thread::spawn(move || {
            run_at(
                &root,
                |active| {
                    assert!(active);
                    Ok(())
                },
                || {
                    ready.wait();
                    ready.wait();
                    Ok(())
                },
                || {
                    events.lock().expect("events").push("last stopped");
                    Ok(())
                },
            )
            .expect("second");
        })
    };
    second_ready.wait();
    first_close.wait();
    first.join().expect("first thread");
    assert!(events.lock().expect("events").is_empty());
    second_ready.wait();
    second.join().expect("second thread");
    assert_eq!(*events.lock().expect("events"), ["last stopped"]);
    std::fs::remove_dir_all(root).expect("cleanup");
}
