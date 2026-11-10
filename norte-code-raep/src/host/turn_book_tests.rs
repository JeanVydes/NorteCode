use super::*;

fn queued(turn_id: &str, text: &str) -> QueuedTurn {
    QueuedTurn {
        turn_id: TurnId::new(turn_id).expect("turn id"),
        text: text.to_owned(),
        attachments: Vec::new(),
        budget: None,
        reasoning: None,
        metadata: None,
        output: vec![Modality::Text],
        response_format: None,
        replaces_turn_id: None,
    }
}

fn id(value: &str) -> TurnId {
    TurnId::new(value).expect("turn id")
}

#[test]
fn reserving_and_releasing_leaves_the_session_free() {
    let mut book = TurnBook::default();
    book.reserve("s", id("turn-1"));
    assert!(book.is_occupied("s"));
    assert_eq!(book.reserved("s"), Some(id("turn-1")));
    assert_eq!(book.active("s"), None);

    book.release("s", &id("turn-1"));
    assert!(!book.is_occupied("s"));
    assert_eq!(book.reserved("s"), None);
}

#[test]
fn a_stale_release_does_not_clear_the_running_turn() {
    let mut book = TurnBook::default();
    book.reserve("s", id("turn-2"));

    book.release("s", &id("turn-1"));

    assert_eq!(book.reserved("s"), Some(id("turn-2")));
}

#[test]
fn activating_names_the_turn_the_client_knows() {
    let mut book = TurnBook::default();
    book.reserve("s", id("client-1"));
    book.activate("s", id("client-1"));

    assert_eq!(book.active("s"), Some(id("client-1")));
    assert_eq!(book.reserved("s"), None);
    assert!(book.is_active("s", &id("client-1")));
    assert!(!book.is_active("s", &id("other")));
}

#[test]
fn a_queue_drains_in_order_and_keeps_no_empty_entry() {
    let mut book = TurnBook::default();
    assert_eq!(book.enqueue("s", queued("a", "first"), false), 1);
    assert_eq!(book.enqueue("s", queued("b", "second"), false), 2);
    assert_eq!(book.queue_len("s"), 2);

    assert_eq!(book.dequeue("s").map(|turn| turn.turn_id), Some(id("a")));
    assert_eq!(book.dequeue("s").map(|turn| turn.turn_id), Some(id("b")));
    assert_eq!(book.queue_len("s"), 0);
    assert!(book.dequeue("s").is_none());
}

#[test]
fn an_interrupting_input_takes_the_front() {
    let mut book = TurnBook::default();
    book.enqueue("s", queued("behind", "queued"), false);

    assert_eq!(book.enqueue("s", queued("next", "interrupt"), true), 1);
    assert_eq!(book.dequeue("s").map(|turn| turn.turn_id), Some(id("next")));
}

#[test]
fn removing_a_queued_turn_reports_whether_it_was_there() {
    let mut book = TurnBook::default();
    book.enqueue("s", queued("a", "first"), false);

    assert!(book.remove_queued("s", &id("a")));
    assert!(!book.remove_queued("s", &id("a")));
    assert_eq!(book.queue_len("s"), 0);
}

#[test]
fn releasing_clears_the_pending_id_and_its_modalities() {
    let mut book = TurnBook::default();
    book.reserve("s", id("turn-1"));
    book.pending_push("s", id("turn-1"));
    book.set_pending_output("s", vec![Modality::Text, Modality::Image]);

    book.release("s", &id("turn-1"));

    assert_eq!(book.take_pending("s"), None);
    assert!(book.take_pending_output("s").is_none());
}

#[test]
fn aliases_are_bounded_and_find_each_other() {
    let mut book = TurnBook::default();
    book.set_alias("s", 1, id("client-1"), 2);
    book.set_alias("s", 2, id("client-2"), 2);
    book.set_alias("s", 3, id("client-3"), 2);

    assert_eq!(book.alias("s", 1), None, "the oldest alias is dropped");
    assert_eq!(book.alias("s", 3), Some(id("client-3")));
    assert_eq!(book.service_turn("s", &id("client-2")), Some(2));
}

#[test]
fn turn_output_is_scoped_to_one_turn() {
    let mut book = TurnBook::default();
    book.set_turn_output("s", 1, vec![Modality::Image]);

    assert_eq!(book.turn_output("s", 1), Some(&vec![Modality::Image]));
    assert_eq!(book.turn_output("s", 2), None);

    book.remove_turn_output("s", 1);
    assert_eq!(book.turn_output("s", 1), None);
}
