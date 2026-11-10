use super::*;

#[test]
fn the_flag_beats_the_configuration_and_the_configuration_beats_nothing() {
    // The order is the whole point: `--verbose` is for one run, so it wins;
    // a run that says nothing gets the file. Reading the flag's own default
    // instead would make `ui.verbosity` unreachable, which is the bug this
    // function exists to prevent.
    let mut config = Config::default();
    config.ui.verbosity = "high".to_owned();
    assert_eq!(
        resolve_verbosity(Some("low"), &config),
        "low",
        "the flag is the run's own setting"
    );
    assert_eq!(
        resolve_verbosity(None, &config),
        "high",
        "no flag means the file decides"
    );
}

#[test]
fn a_configuration_that_says_nothing_leaves_the_client_default() {
    // `medium` is what the client has always shown, and an empty file has to
    // keep behaving exactly as it did before `ui` existed.
    let config = Config::default();
    assert_eq!(resolve_verbosity(None, &config), "medium");
}

#[test]
fn remote_services_cannot_be_managed_as_local_processes() {
    let mut config = Config::default();
    config.ui.service_type = ServiceType::Remote;
    let error = ensure_local_service(&config).expect_err("remote lifecycle must be rejected");
    assert!(error.contains("service_type"));
}

#[test]
fn local_service_uses_the_configured_http_authority() {
    assert_eq!(
        service_bind_env("http://127.0.0.1:8899/raep"),
        Some(("NORTE_CODE_BIND", "127.0.0.1:8899".to_owned()))
    );
    assert_eq!(
        service_bind_env("https://agent.example/raep"),
        None,
        "a hostname without an explicit port lets the server keep its default"
    );
}
