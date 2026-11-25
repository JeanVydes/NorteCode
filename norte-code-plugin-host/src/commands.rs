//! Public registry API for plugin-provided slash commands.

use std::collections::{BTreeMap, BTreeSet};

/// A validated, globally unique slash-command name without the leading `/`.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CommandName(String);

impl CommandName {
    /// Creates a lower-case command name made of words separated by `-`.
    ///
    /// # Errors
    ///
    /// Returns [`CommandRegistrationError::InvalidName`] when the name is
    /// empty, too long, or contains characters outside that format.
    pub fn new(value: impl Into<String>) -> Result<Self, CommandRegistrationError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 48
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && value
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric);
        if valid {
            Ok(Self(value))
        } else {
            Err(CommandRegistrationError::InvalidName(value))
        }
    }

    /// The spelling used in the composer, without `/`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One argument accepted by a plugin command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandArgument {
    /// Argument name shown in help and completion.
    pub name: String,
    /// Short explanation shown in help.
    pub description: String,
    /// Whether the argument must be supplied.
    pub required: bool,
    /// Closed values offered for completion and validation; empty means free text.
    pub choices: Vec<String>,
}

/// A plugin command and its typed argument hints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    /// Globally unique command name.
    pub name: CommandName,
    /// Short description shown in the floating menu.
    pub description: String,
    /// Arguments in the order they are parsed.
    pub arguments: Vec<CommandArgument>,
}

/// A plugin command declaration registered with its owning plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredCommand {
    /// Plugin that owns and executes the command.
    pub plugin: String,
    /// The command definition.
    pub spec: CommandSpec,
    pub(crate) owner_index: Option<usize>,
}

/// Registry conflict or invalid command declaration.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CommandRegistrationError {
    /// The command name does not follow the lower-case name rules.
    #[error("invalid slash command name `{0}`")]
    InvalidName(String),
    /// A built-in command already owns this name.
    #[error("`/{0}` is reserved by Norte Code")]
    Reserved(String),
    /// Another plugin already registered this command name.
    #[error("slash command `/{0}` is already registered")]
    Duplicate(String),
    /// The plugin did not provide a usable description or argument layout.
    #[error("slash command `/{command}` has an invalid definition: {reason}")]
    InvalidDefinition {
        /// The command name.
        command: String,
        /// What made the definition invalid.
        reason: String,
    },
}

/// Commands offered by loaded plugins.
#[derive(Clone, Debug, Default)]
pub struct CommandRegistry {
    entries: BTreeMap<CommandName, RegisteredCommand>,
    registering_plugin: Option<(usize, String)>,
}

impl CommandRegistry {
    /// Registers one plugin command. Built-in names are reserved globally.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is reserved or duplicated, or the
    /// command's argument metadata is invalid.
    pub fn register(
        &mut self,
        plugin: &str,
        spec: CommandSpec,
    ) -> Result<(), CommandRegistrationError> {
        let name = spec.name.as_str();
        if self
            .registering_plugin
            .as_ref()
            .is_some_and(|(_, owner)| owner != plugin)
        {
            return Err(CommandRegistrationError::InvalidDefinition {
                command: name.to_owned(),
                reason: "a plugin may only register commands under its own name".to_owned(),
            });
        }
        if matches!(name, "compact" | "new" | "open" | "reasoning") {
            return Err(CommandRegistrationError::Reserved(name.to_owned()));
        }
        if spec.description.trim().is_empty() {
            return Err(CommandRegistrationError::InvalidDefinition {
                command: name.to_owned(),
                reason: "description is empty".to_owned(),
            });
        }
        let mut optional_seen = false;
        let mut argument_names = BTreeSet::new();
        let invalid_reason = spec.arguments.iter().find_map(|argument| {
            if !valid_identifier(&argument.name) {
                return Some("argument names must be lower-case words separated by hyphens");
            }
            if !argument_names.insert(argument.name.as_str()) {
                return Some("argument names must be unique");
            }
            if argument.description.trim().is_empty() {
                return Some("argument descriptions cannot be empty");
            }
            if argument
                .choices
                .iter()
                .any(|choice| choice.trim().is_empty())
            {
                return Some("argument choices cannot be empty");
            }
            let mut choices = BTreeSet::new();
            if argument
                .choices
                .iter()
                .any(|choice| !choices.insert(choice))
            {
                return Some("argument choices must be unique");
            }
            if !argument.required {
                optional_seen = true;
            } else if optional_seen {
                return Some("required arguments cannot follow optional arguments");
            }
            None
        });
        if let Some(reason) = invalid_reason {
            return Err(CommandRegistrationError::InvalidDefinition {
                command: name.to_owned(),
                reason: reason.to_owned(),
            });
        }
        if self.entries.contains_key(&spec.name) {
            return Err(CommandRegistrationError::Duplicate(name.to_owned()));
        }
        self.entries.insert(
            spec.name.clone(),
            RegisteredCommand {
                plugin: plugin.to_owned(),
                spec,
                owner_index: self.registering_plugin.as_ref().map(|(index, _)| *index),
            },
        );
        Ok(())
    }

    /// Every accepted command, sorted by name.
    pub fn commands(&self) -> impl Iterator<Item = &RegisteredCommand> {
        self.entries.values()
    }

    /// One command by name.
    #[must_use]
    pub fn get(&self, name: &CommandName) -> Option<&RegisteredCommand> {
        self.entries.get(name)
    }

    pub(crate) fn set_registration_owner(&mut self, index: usize, name: String) {
        self.registering_plugin = Some((index, name));
    }

    pub(crate) fn clear_registration_owner(&mut self) {
        self.registering_plugin = None;
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

/// A plugin action failed or rejected its arguments.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CommandExecutionError {
    /// The requested command is not registered by this plugin.
    #[error("the plugin command is not registered")]
    UnknownCommand,
    /// The plugin rejected the supplied arguments.
    #[error("{0}")]
    Rejected(String),
    /// The plugin action failed while running.
    #[error("{0}")]
    Failed(String),
}

/// The visible result returned by a plugin command action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    /// Short user-facing result text.
    pub message: String,
}
