//! Which colours the terminal can show.
//!
//! The decision reads the environment once per style, so it lives here, apart
//! from the escape encoding, to be testable with an environment of its own.

/// How many colours the destination can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Colors {
    /// No escape sequences at all.
    None,
    /// The classic 16-colour palette.
    Ansi16,
    /// The xterm 256-colour palette.
    Ansi256,
    /// 24-bit RGB.
    True,
}

/// The environment variables the decision reads.
///
/// Passing them in keeps the choice testable: a test can describe a terminal
/// without mutating the process environment, which every other test shares.
#[derive(Default)]
pub(crate) struct Environment {
    /// Whether the destination is a terminal.
    pub(crate) terminal: bool,
    /// `NO_COLOR`, which opts out of styling when non-empty.
    pub(crate) no_color: Option<String>,
    /// `CLICOLOR`, where `0` opts out.
    pub(crate) clicolor: Option<String>,
    /// `CLICOLOR_FORCE`, which overrides `CLICOLOR=0` when `1`.
    pub(crate) clicolor_force: Option<String>,
    /// `COLORTERM`, where `truecolor` or `24bit` means RGB.
    pub(crate) colorterm: Option<String>,
    /// `TERM`, where `dumb` opts out and `256color` selects the palette.
    pub(crate) term: Option<String>,
}

impl Environment {
    /// The environment of the current process, for the given stream.
    pub(crate) fn current(is_terminal: bool) -> Self {
        let variable = |name: &str| std::env::var(name).ok();
        Self {
            terminal: is_terminal,
            no_color: std::env::var("NO_COLOR").ok(),
            clicolor: variable("CLICOLOR"),
            clicolor_force: variable("CLICOLOR_FORCE"),
            colorterm: variable("COLORTERM"),
            term: variable("TERM"),
        }
        .with_no_color(std::env::var_os("NO_COLOR"))
    }

    /// `NO_COLOR` opts out unless it is set to the empty string.
    fn with_no_color(mut self, value: Option<std::ffi::OsString>) -> Self {
        if value.is_some_and(|value| !value.is_empty()) {
            self.no_color = Some(String::new());
        }
        self
    }

    /// Which colours this terminal can show.
    pub(crate) fn colors(&self) -> Colors {
        if !self.terminal {
            return Colors::None;
        }
        if self
            .no_color
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            return Colors::None;
        }
        if self.term.as_deref().is_some_and(|term| term == "dumb") {
            return Colors::None;
        }
        if self.clicolor.as_deref().is_some_and(|value| value == "0")
            && !self
                .clicolor_force
                .as_deref()
                .is_some_and(|value| value == "1")
        {
            return Colors::None;
        }
        if self
            .colorterm
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("truecolor") || value == "24bit")
        {
            return Colors::True;
        }
        if self
            .term
            .as_deref()
            .is_some_and(|term| term.contains("256color") || term.contains("direct"))
        {
            return Colors::Ansi256;
        }
        Colors::Ansi16
    }
}

#[cfg(test)]
mod tests {
    use super::{Colors, Environment};

    fn colors(environment: Environment) -> Colors {
        environment.colors()
    }

    #[test]
    fn a_pipe_or_an_opt_out_gets_no_escapes() {
        assert_eq!(colors(Environment::default()), Colors::None);
        assert_eq!(
            colors(Environment {
                terminal: true,
                no_color: Some("1".to_owned()),
                ..Environment::default()
            }),
            Colors::None
        );
        assert_eq!(
            colors(Environment {
                terminal: true,
                term: Some("dumb".to_owned()),
                ..Environment::default()
            }),
            Colors::None
        );
        assert_eq!(
            colors(Environment {
                terminal: true,
                clicolor: Some("0".to_owned()),
                ..Environment::default()
            }),
            Colors::None
        );
        // `CLICOLOR_FORCE=1` wins over `CLICOLOR=0`.
        assert_ne!(
            colors(Environment {
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
            colors(Environment {
                terminal: true,
                colorterm: Some("truecolor".to_owned()),
                term: Some("xterm-256color".to_owned()),
                ..Environment::default()
            }),
            Colors::True
        );
        assert_eq!(
            colors(Environment {
                terminal: true,
                term: Some("xterm-256color".to_owned()),
                ..Environment::default()
            }),
            Colors::Ansi256
        );
        assert_eq!(
            colors(Environment {
                terminal: true,
                term: Some("xterm".to_owned()),
                ..Environment::default()
            }),
            Colors::Ansi16
        );
    }
}
