use super::{TranscriptNode, TranscriptNodeKind, TurnKey};

#[test]
fn a_history_turn_and_a_live_turn_are_the_same_key() {
    assert_eq!(TurnKey::from_history("turn_3"), TurnKey::Live(3));
}

#[test]
fn a_history_turn_with_a_name_of_its_own_keeps_that_name() {
    assert_eq!(
        TurnKey::from_history("client-7"),
        TurnKey::Named("client-7".to_owned())
    );
}

#[test]
fn a_node_holds_the_nodes_under_it() {
    let mut turn = TranscriptNode::new(TranscriptNodeKind::Turn {
        key: TurnKey::Live(1),
    });
    turn.push(TranscriptNode::new(TranscriptNodeKind::Notice {
        text: "un aviso".to_owned(),
    }));

    assert_eq!(turn.turn_key(), Some(&TurnKey::Live(1)));
    assert_eq!(turn.children().len(), 1);
}
