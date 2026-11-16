use super::*;

#[test]
fn a_child_is_found_only_under_its_parent() {
    let registry = ChildRegistry::new();
    let parent = SessionKey::new("parent".to_owned());
    let other = SessionKey::new("other".to_owned());
    let child = SessionKey::new("child".to_owned());
    registry.register(
        parent.clone(),
        Child::new(child.clone(), "call-1".to_owned()),
    );

    assert!(registry.get(&parent, &child).is_some());
    assert!(
        registry.get(&other, &child).is_none(),
        "a child is not visible to a session that did not spawn it"
    );
    assert_eq!(registry.of(&parent).len(), 1);
    assert!(registry.of(&other).is_empty());
}

#[test]
fn suspending_holds_and_releasing_flushes() {
    let registry = ChildRegistry::new();
    let parent = SessionKey::new("parent".to_owned());
    let child = SessionKey::new("child".to_owned());
    registry.register(
        parent.clone(),
        Child::new(child.clone(), "call-1".to_owned()),
    );

    let (suspended, held) = registry
        .set_suspended(&parent, &child, true)
        .expect("registered");
    assert!(suspended);
    assert!(held.is_empty(), "suspending holds nothing new");

    assert_eq!(
        registry.hold(&parent, &child, "focus on auth".to_owned()),
        1
    );
    assert_eq!(registry.hold(&parent, &child, "and tests".to_owned()), 2);

    let (suspended, held) = registry
        .set_suspended(&parent, &child, false)
        .expect("registered");
    assert!(!suspended);
    assert_eq!(held, vec!["focus on auth", "and tests"]);
    assert_eq!(
        registry
            .get(&parent, &child)
            .expect("registered")
            .held
            .len(),
        0,
        "a release leaves nothing held"
    );
}

#[tokio::test]
async fn finishing_a_child_wakes_waiters() {
    let registry = ChildRegistry::new();
    let parent = SessionKey::new("parent".to_owned());
    let child = SessionKey::new("child".to_owned());
    registry.register(
        parent.clone(),
        Child::new(child.clone(), "call-1".to_owned()),
    );
    let registration = registry.get(&parent, &child).expect("registered");
    let notified = registration.changed.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();

    assert_eq!(
        registry.finish_and_take_message(&parent, &child, RunEnd::Idle),
        None
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), notified)
        .await
        .expect("waiter notified");
    assert!(
        registry
            .get(&parent, &child)
            .expect("registered")
            .is_finished()
    );
}

#[test]
fn queued_messages_run_in_arrival_order_and_only_start_once() {
    let registry = ChildRegistry::new();
    let parent = SessionKey::new("parent".to_owned());
    let child = SessionKey::new("child".to_owned());
    registry.register(
        parent.clone(),
        Child::new(child.clone(), "call-1".to_owned()),
    );
    assert_eq!(
        registry.finish_and_take_message(&parent, &child, RunEnd::Idle),
        None
    );

    assert_eq!(
        registry.queue_message(&parent, &child, "first".to_owned()),
        Some(true)
    );
    assert_eq!(
        registry.queue_message(&parent, &child, "second".to_owned()),
        Some(false)
    );
    assert_eq!(
        registry.take_queued_message(&parent, &child).as_deref(),
        Some("first")
    );
    assert_eq!(
        registry.finish_and_take_message(&parent, &child, RunEnd::Idle),
        Some("second".to_owned())
    );
    assert!(
        !registry
            .get(&parent, &child)
            .expect("registered")
            .is_finished()
    );
    assert_eq!(
        registry.finish_and_take_message(&parent, &child, RunEnd::Idle),
        None
    );
    assert!(
        registry
            .get(&parent, &child)
            .expect("registered")
            .is_finished()
    );
}
