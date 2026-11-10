use super::{Colors, Environment};

fn colors(environment: &Environment) -> Colors {
    environment.colors()
}

#[test]
fn a_pipe_or_an_opt_out_gets_no_escapes() {
    assert_eq!(colors(&Environment::default()), Colors::None);
    assert_eq!(
        colors(&Environment {
            terminal: true,
            no_color: Some("1".to_owned()),
            ..Environment::default()
        }),
        Colors::None
    );
    assert_eq!(
        colors(&Environment {
            terminal: true,
            term: Some("dumb".to_owned()),
            ..Environment::default()
        }),
        Colors::None
    );
    assert_eq!(
        colors(&Environment {
            terminal: true,
            clicolor: Some("0".to_owned()),
            ..Environment::default()
        }),
        Colors::None
    );
    // `CLICOLOR_FORCE=1` wins over `CLICOLOR=0`.
    assert_ne!(
        colors(&Environment {
            terminal: true,
            clicolor: Some("0".to_owned()),
            clicolor_force: Some("1".to_owned()),
            ..Environment::default()
        }),
        Colors::None
    );
}

#[test]
fn truecolor_is_preferred_and_256_is_the_fallback() {
    assert_eq!(
        colors(&Environment {
            terminal: true,
            colorterm: Some("truecolor".to_owned()),
            term: Some("xterm-256color".to_owned()),
            ..Environment::default()
        }),
        Colors::True
    );
    assert_eq!(
        colors(&Environment {
            terminal: true,
            term: Some("xterm-256color".to_owned()),
            ..Environment::default()
        }),
        Colors::Ansi256
    );
    assert_eq!(
        colors(&Environment {
            terminal: true,
            term: Some("xterm".to_owned()),
            ..Environment::default()
        }),
        Colors::Ansi16
    );
}
