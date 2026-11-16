//! The environment the tools run in.
//!
//! A child process does not inherit the server's environment: the shell tool
//! clears it. That is deliberate, so a secret in the server's environment does
//! not reach a command the model wrote. The consequence is that the tools
//! cannot assume anything a person's shell would have, and a tool that needs
//! `HOME` or a display has to be told which one.

use std::collections::BTreeMap;

/// The variables a tool needs to behave like the person's own session.
#[derive(Clone, Debug, Default)]
pub(super) struct ToolEnvironment {
    /// The variables a command inherits.
    variables: BTreeMap<String, String>,
    /// The graphical session, when the host has one to offer.
    display: Option<String>,
}

impl ToolEnvironment {
    /// Reads the environment a tool needs from this process.
    ///
    /// Only the variables a command needs to work are carried: `PATH`, the
    /// home directory and, when the host runs under a graphical session, the
    /// display it draws on. Nothing else crosses, so a provider key in the
    /// server's environment stays out of every command.
    #[must_use]
    pub(super) fn from_process() -> Self {
        let mut variables = BTreeMap::new();
        for key in ["PATH", "HOME", "USER", "TMPDIR"] {
            if let Ok(value) = std::env::var(key)
                && !value.is_empty()
            {
                variables.insert(key.to_owned(), value);
            }
        }
        Self {
            variables,
            display: display_from_process(),
        }
    }

    /// The variables a command inherits.
    pub(super) fn variables(&self) -> BTreeMap<String, String> {
        self.variables.clone()
    }

    /// The home directory, when the host has one.
    pub(super) fn home(&self) -> Option<&str> {
        self.variables.get("HOME").map(String::as_str)
    }

    /// The graphical session a browser could be launched into.
    pub(super) fn display(&self) -> Option<&str> {
        self.display.as_deref()
    }

    /// The note a tool description carries about the environment.
    ///
    /// The model reads the declarations, not this process, so what a command
    /// can rely on has to be in the text it is handed.
    #[must_use]
    pub(super) fn note(&self) -> String {
        let home = self
            .home()
            .map_or_else(|| "not set".to_owned(), |home| format!("`{home}`"));
        let display = self.display.as_deref().map_or_else(
            || {
                "none: this host has no graphical session, so a browser cannot open a window"
                    .to_owned()
            },
            |display| format!("`{display}`"),
        );
        format!(
            "Each command starts with a clean environment: `HOME` is {home}, `PATH` is \
             inherited from the host, and nothing else is. The graphical session is {display}."
        )
    }
}

/// The display a graphical program would draw on, when there is one.
fn display_from_process() -> Option<String> {
    if let Ok(display) = std::env::var("WAYLAND_DISPLAY")
        && !display.is_empty()
    {
        return Some(format!("wayland:{display}"));
    }
    if let Ok(display) = std::env::var("DISPLAY")
        && !display.is_empty()
    {
        return Some(format!("x11:{display}"));
    }
    None
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;
