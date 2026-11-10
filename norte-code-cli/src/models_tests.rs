use super::*;

#[test]
fn the_choice_the_flow_made_is_the_pair_the_session_opens_on() {
    // The startup flow writes the choice into the configuration and hands
    // back the same pair, and the session is opened on that pair: a run that
    // dropped it and read the configuration instead died on "no provider
    // specified" right after the question was answered.
    let choice = ModelChoice::new("nvidia", "nvidia/nemotron-3.5-super-vl-120b-a12b");
    let (provider, model) = session_choice(Some(&choice), None, None);
    assert_eq!(provider.as_deref(), Some("nvidia"));
    assert_eq!(
        model.as_deref(),
        Some("nvidia/nemotron-3.5-super-vl-120b-a12b")
    );

    // A flag wins on its own half and leaves the other where it was: a
    // `--model` names a model, not a pair.
    let (provider, model) = session_choice(
        Some(&choice),
        None,
        Some("deepseek-ai/deepseek-v4.1-flash".to_owned()),
    );
    assert_eq!(provider.as_deref(), Some("nvidia"));
    assert_eq!(model.as_deref(), Some("deepseek-ai/deepseek-v4.1-flash"));
    let (provider, model) = session_choice(Some(&choice), Some("river".to_owned()), None);
    assert_eq!(provider.as_deref(), Some("river"));
    assert_eq!(
        model.as_deref(),
        Some("nvidia/nemotron-3.5-super-vl-120b-a12b")
    );

    // No choice and no flag: the configuration answers.
    let (provider, model) = session_choice(None, None, None);
    assert!(provider.is_none() && model.is_none());
}
