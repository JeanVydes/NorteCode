//! The arguments a shell call carries, decoded at the wire boundary.

use serde::Deserialize;

/// What a `shell` or `bash` call was asked to run.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ShellArgs {
    /// The script, as named by the shell executor.
    pub(crate) script: Option<String>,
    /// A command under either of the common argument names.
    pub(crate) command: Option<String>,
    /// The third spelling some calls use.
    pub(crate) cmd: Option<String>,
}

impl ShellArgs {
    /// The command to show above the console, when the call carries one.
    pub(crate) fn command(&self) -> Option<&str> {
        self.script
            .as_deref()
            .or(self.command.as_deref())
            .or(self.cmd.as_deref())
            .map(str::trim)
            .filter(|command| !command.is_empty())
    }
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
