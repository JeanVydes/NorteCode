use super::*;

#[test]
fn unknown_phase_fails_to_deserialize() {
    let result: Result<Phase, _> = serde_json::from_str("\"teleporting\"");
    assert!(result.is_err());
}

#[test]
fn phases_round_trip() {
    for phase in [Phase::Idle, Phase::Streaming, Phase::AwaitingTool] {
        let json = serde_json::to_string(&phase).expect("serialize");
        let decoded: Phase = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, phase);
    }
}
