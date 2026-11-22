use norte::{Message, Role};

use super::recent_start;

#[test]
fn keeping_no_recent_messages_does_not_index_past_the_end() {
    let messages = vec![Message::user("first"), Message::tool(vec![])];

    assert_eq!(recent_start(&messages, 0), messages.len());
}

#[test]
fn a_tool_suffix_is_not_split_from_its_call() {
    let messages = vec![
        Message::user("first"),
        Message::assistant("call"),
        Message::new(Role::Tool, vec![]),
    ];

    assert_eq!(recent_start(&messages, 1), 1);
}
