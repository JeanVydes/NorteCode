use super::*;

fn steered(id: &str) -> Steered {
    Steered::new(
        id.to_owned(),
        format!("text of {id}"),
        Vec::new(),
        Vec::new(),
    )
}

#[test]
fn take_if_pending_removes_one_and_keeps_the_rest() {
    let steering = Steering::default();
    let session = SessionId::new();
    steering.push(session, steered("a"));
    steering.push(session, steered("b"));

    let taken = steering
        .take_if_pending(session, "a")
        .expect("first is pending");
    assert_eq!(taken.id, "a");

    assert!(!steering.is_pending(session, "a"));
    assert!(steering.is_pending(session, "b"));
    let remaining = steering.take(session);
    assert_eq!(
        remaining
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
}

#[test]
fn take_if_pending_with_an_unknown_id_changes_nothing() {
    let steering = Steering::default();
    let session = SessionId::new();
    steering.push(session, steered("a"));

    assert!(steering.take_if_pending(session, "missing").is_none());
    assert!(steering.is_pending(session, "a"));
    assert!(
        steering.applied(session, 1).is_empty(),
        "the applied bookkeeping is not touched either"
    );
}
