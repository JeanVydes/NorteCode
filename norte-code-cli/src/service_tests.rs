//! Service startup settings stay bounded and predictable.

use norte_code_client::config::ServiceLifecycleConfig;

use super::{cmdline_is_service, restart_local, worker_count_from};

#[test]
fn restarting_stops_the_service_before_starting_a_new_one() {
    let events = std::cell::RefCell::new(Vec::new());
    restart_local(
        "local",
        || {
            events.borrow_mut().push("stop");
            Ok(())
        },
        |_| {
            events.borrow_mut().push("probe");
            false
        },
        || {
            events.borrow_mut().push("start");
            Ok(())
        },
    )
    .expect("restart");
    assert_eq!(*events.borrow(), ["stop", "probe", "start"]);
}

#[test]
fn restarting_does_not_reuse_a_service_that_failed_to_stop() {
    let started = std::cell::Cell::new(false);
    let result = restart_local(
        "local",
        || Ok(()),
        |_| true,
        || {
            started.set(true);
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(!started.get());
}

fn lifecycle_with_workers(workers: usize) -> ServiceLifecycleConfig {
    ServiceLifecycleConfig {
        tokio_workers: workers,
        ..ServiceLifecycleConfig::default()
    }
}

#[test]
fn worker_count_defaults_to_the_configured_value_and_never_exceeds_the_machine() {
    assert_eq!(worker_count_from(None, 12, &lifecycle_with_workers(4)), 4);
    assert_eq!(worker_count_from(None, 12, &lifecycle_with_workers(9)), 9);
    assert_eq!(worker_count_from(None, 2, &lifecycle_with_workers(9)), 2);
}

#[test]
fn worker_count_honors_a_valid_override_over_the_configured_value() {
    let config = lifecycle_with_workers(7);
    assert_eq!(worker_count_from(Some(" 3 "), 12, &config), 3);
    assert_eq!(worker_count_from(Some("32"), 12, &config), 12);
}

#[test]
fn worker_count_ignores_invalid_overrides() {
    let config = lifecycle_with_workers(5);
    assert_eq!(worker_count_from(Some("many"), 12, &config), 5);
    assert_eq!(worker_count_from(Some("0"), 12, &config), 1);
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn service_process_is_found_by_its_arguments_not_its_name() {
    assert!(cmdline_is_service(&args(&[
        "/home/jean/.cargo/bin/norte",
        "service",
        "serve"
    ])));
}

#[test]
fn tui_and_other_service_subcommands_are_not_the_daemon() {
    assert!(!cmdline_is_service(&args(&["/home/jean/.cargo/bin/norte"])));
    assert!(!cmdline_is_service(&args(&[
        "/home/jean/.cargo/bin/norte",
        "service",
        "status"
    ])));
    assert!(!cmdline_is_service(&args(&[
        "/home/jean/.cargo/bin/norte",
        "service",
        "restart"
    ])));
}

#[test]
fn probe_resolves_hostnames_and_preserves_the_base_path() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("address").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection");
        let mut request = [0; 4096];
        let count = stream.read(&mut request).expect("request");
        let request = String::from_utf8_lossy(&request[..count]);
        assert!(request.starts_with("GET /proxy/raep/v1/health/live HTTP/1.1"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"status\":\"live\"}")
            .expect("response");
    });
    assert!(super::probe(&format!("http://localhost:{port}/proxy")));
    server.join().expect("server thread");
}
