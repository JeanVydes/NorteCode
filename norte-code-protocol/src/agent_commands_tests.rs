use super::*;

#[test]
fn parses_closed_operation_names() {
    let command: AgentCommand =
        serde_json::from_str(r#"{"operation":"list_jobs"}"#).expect("command");
    assert!(matches!(command, AgentCommand::ListJobs));
}

#[test]
fn rejects_unknown_operations() {
    let result = serde_json::from_str::<AgentCommand>(r#"{"operation":"delete_everything"}"#);
    assert!(result.is_err());
}
