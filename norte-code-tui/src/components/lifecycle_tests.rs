use super::{ComponentLifecycle, LifecycleEvent, LifecyclePhase};

#[test]
fn follows_mount_load_refresh_and_unmount_phases() {
    let mut lifecycle = ComponentLifecycle::default();

    assert_eq!(lifecycle.phase(), LifecyclePhase::Dormant);
    assert_eq!(
        lifecycle.transition(LifecycleEvent::Mount),
        Ok(LifecyclePhase::Mounting)
    );
    assert_eq!(
        lifecycle.transition(LifecycleEvent::LoadStarted),
        Ok(LifecyclePhase::Loading)
    );
    assert_eq!(
        lifecycle.transition(LifecycleEvent::Loaded),
        Ok(LifecyclePhase::Ready)
    );
    assert_eq!(
        lifecycle.transition(LifecycleEvent::Refresh),
        Ok(LifecyclePhase::Updating)
    );
    assert_eq!(
        lifecycle.transition(LifecycleEvent::Empty),
        Ok(LifecyclePhase::Empty)
    );
    assert_eq!(
        lifecycle.transition(LifecycleEvent::Unmount),
        Ok(LifecyclePhase::Dormant)
    );
}

#[test]
fn rejects_load_completion_before_loading() {
    let mut lifecycle = ComponentLifecycle::default();

    assert!(lifecycle.transition(LifecycleEvent::Loaded).is_err());
    assert_eq!(lifecycle.phase(), LifecyclePhase::Dormant);
}

#[test]
fn retries_failed_components_from_loading() {
    let mut lifecycle = ComponentLifecycle::default();
    lifecycle.transition(LifecycleEvent::Mount).unwrap();
    lifecycle.transition(LifecycleEvent::LoadStarted).unwrap();
    lifecycle.transition(LifecycleEvent::Failed).unwrap();

    assert_eq!(
        lifecycle.transition(LifecycleEvent::Retry),
        Ok(LifecyclePhase::Loading)
    );
}
