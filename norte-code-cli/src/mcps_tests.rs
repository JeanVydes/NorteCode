use serde_json::json;

use super::{agent_at_mut, default_agent_name, find_agent_index};

#[test]
fn an_out_of_range_agent_index_is_an_error() {
    let mut root = json!({ "agents": [{ "name": "main" }] });

    assert!(agent_at_mut(&mut root, 1).is_err());
}

#[test]
fn a_missing_agent_name_is_an_error() {
    let mut root = json!({ "agents": [{ "name": "main" }] });

    assert!(find_agent_index(&mut root, Some("other")).is_err());
}

#[test]
fn a_single_agent_without_a_name_is_not_a_default() {
    assert_eq!(default_agent_name(&json!({ "agents": [{}] })), None);
}
