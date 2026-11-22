//! Service startup settings stay bounded and predictable.

use norte_code_client::config::ServiceLifecycleConfig;

use super::worker_count_from;

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
