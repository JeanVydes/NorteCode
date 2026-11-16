//! Whether the reader has a usable key for a provider catalogue entry.
//!
//! The transport keys live either in the environment (`environment_keys`
//! in `providers.json`, first set and non-blank wins) or in the 0600
//! `secrets.json` beside the config. Neither is the TUI's secret to hand
//! to the model or the host; this module only answers yes/no at the moment
//! a model is picked.

use norte_code_protocol::keys::{environment_value, read_secret, secrets_path};

use crate::ui_model::UiModel;

/// The outcome of looking up a provider's key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProviderKeyStatus {
    /// A usable key exists, or the provider names none and needs none.
    Available,
    /// The provider names variables, and neither the environment nor the
    /// secrets file holds one.
    Missing {
        /// The provider id missing its key.
        provider: String,
        /// The environment variables the provider would have read.
        environment_keys: Vec<String>,
    },
    /// The catalogue could not be loaded; the status is unknown and the
    /// switch is allowed to proceed.
    Unknown,
}

impl ProviderKeyStatus {
    /// The status as every reader of it treats it.
    ///
    /// Only [`ProviderKeyStatus::Missing`] blocks the switch.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn blocks(&self) -> bool {
        matches!(self, Self::Missing { .. })
    }
}

/// What happened to a key that was erased.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProviderKeyEraseOutcome {
    /// The key is published through `$VAR`; the TUI does not unset environment
    /// variables.
    FromEnvironment {
        /// The variable that holds the key.
        variable: String,
    },
    /// A saved key was removed from the secrets file.
    Removed {
        /// The file it was removed from.
        path: String,
    },
    /// No key was stored for this provider.
    Missing,
    /// The removal could not be carried out.
    ///
    /// The message is shown as-is; nothing was changed.
    Error {
        /// Why the removal failed.
        message: String,
    },
}

/// The outcome of erasing a provider's key.
///
/// The environment is checked first, matching how the host resolves keys,
/// then the secrets file. An environment variable is never removed; the
/// reader is told which one holds the key instead.
#[must_use]
pub(crate) fn erase(model: &UiModel, provider_id: &str) -> ProviderKeyEraseOutcome {
    let environment_keys = find_environment_keys(model, provider_id);
    if let Some(variable) = first_set_env_var(&environment_keys) {
        return ProviderKeyEraseOutcome::FromEnvironment { variable };
    }
    // The file is always attempted, plugin-injected providers included,
    // because this is where a saved key lives.
    let Some(path) = secrets_path() else {
        return ProviderKeyEraseOutcome::Error {
            message: "could not resolve the secrets.json path".to_owned(),
        };
    };
    if read_secret(&path, provider_id).is_none() {
        return ProviderKeyEraseOutcome::Missing;
    }
    match norte_code_protocol::keys::delete_secret(&path, provider_id) {
        Ok(()) => ProviderKeyEraseOutcome::Removed {
            path: path.display().to_string(),
        },
        Err(error) => ProviderKeyEraseOutcome::Error {
            message: error.to_string(),
        },
    }
}

/// The environment keys of one provider, from wherever we can learn them.
///
/// The live catalogue is tried first (it carries plugin-injected providers
/// too), then the catalogue files. A provider found nowhere returns no keys:
/// no environment variable can hold the key, so the secrets file is attempted.
fn find_environment_keys(model: &UiModel, provider_id: &str) -> Vec<String> {
    if let Some(provider) = model
        .catalog
        .providers
        .iter()
        .find(|provider| provider.provider_id == provider_id)
    {
        return provider.environment_keys.clone();
    }
    load_environment_keys(provider_id)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// The first environment variable of `members` that is set and non-blank.
fn first_set_env_var(members: &[String]) -> Option<String> {
    members.iter().find(|name| {
        std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
    }).cloned()
}

/// Checks the environment and the secrets file for one provider.
///
/// The host's catalogue is preferred: it carries every provider the host
/// knows, including the ones a plugin injected. A provider the host did not
/// report falls back to the catalogue files; neither yields [`ProviderKeyStatus::Unknown`].
#[must_use]
pub(crate) fn check(model: &UiModel, provider_id: &str) -> ProviderKeyStatus {
    let keys = match environment_keys_for(model, provider_id) {
        Ok(Some(keys)) => keys,
        Ok(None) => return ProviderKeyStatus::Available,
        Err(()) => return ProviderKeyStatus::Unknown,
    };
    match decide(&keys, environment_value, || {
        secrets_path().and_then(|path| read_secret(&path, provider_id))
    }) {
        ProviderKeyDecision::Available => ProviderKeyStatus::Available,
        ProviderKeyDecision::Missing {
            environment_keys,
        } => ProviderKeyStatus::Missing {
            provider: provider_id.to_owned(),
            environment_keys,
        },
    }
}

fn environment_keys_for(
    model: &UiModel,
    provider_id: &str,
) -> Result<Option<Vec<String>>, ()> {
    if let Some(provider) = model
        .catalog
        .providers
        .iter()
        .find(|provider| provider.provider_id == provider_id)
    {
        return Ok(Some(provider.environment_keys.clone()));
    }
    load_environment_keys(provider_id)
}

/// The decision, with the two readers injected so it can be tested.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ProviderKeyDecision {
    /// Available.
    Available,
    /// Missing, what the provider would have tried.
    Missing {
        /// The environment variables the provider names.
        environment_keys: Vec<String>,
    },
}

fn decide(
    environment_keys: &[String],
    env_reader: fn(&[String]) -> Option<norte_code_protocol::config::ApiKey>,
    secret_reader: impl FnOnce() -> Option<norte_code_protocol::config::ApiKey>,
) -> ProviderKeyDecision {
    let has_one = env_reader(environment_keys).is_some() || secret_reader().is_some();
    if environment_keys.is_empty() || has_one {
        ProviderKeyDecision::Available
    } else {
        ProviderKeyDecision::Missing {
            environment_keys: environment_keys.to_vec(),
        }
    }
}

/// The environment keys of one provider, from the catalogue.
///
/// `Err(())` means the catalogue could not be read at all. `Ok(None)` means
/// the provider is not in the file — a plugin-injected entry, or a provider
/// this file does not name — so there is nothing to check.
fn load_environment_keys(provider_id: &str) -> Result<Option<Vec<String>>, ()> {
    let (providers, _models) = norte_code_protocol::catalog::load_catalog_raw().map_err(|_| ())?;
    Ok(providers
        .providers
        .get(provider_id)
        .map(|provider| provider.environment_keys.clone()))
}

#[cfg(test)]
mod tests {
    use super::{ProviderKeyDecision, ProviderKeyStatus, decide};

    #[test]
    fn a_provider_with_no_keys_needs_none() {
        assert_eq!(
            decide(&[], |_| None, || None),
            ProviderKeyDecision::Available
        );
    }

    #[test]
    fn an_environment_value_satisfies() {
        let keys = decide(&["A".to_owned()], |k| k.first().map(|_| norte_code_protocol::config::ApiKey::new("sk".to_owned())), || None);
        assert_eq!(keys, ProviderKeyDecision::Available);
    }

    #[test]
    fn a_secret_value_satisfies() {
        let keys = decide(&["A".to_owned()], |_| None, || Some(norte_code_protocol::config::ApiKey::new("sk".to_owned())));
        assert_eq!(keys, ProviderKeyDecision::Available);
    }

    #[test]
    fn neither_blocks() {
        let keys = decide(&["A".to_owned()], |_| None, || None);
        assert_eq!(
            keys,
            ProviderKeyDecision::Missing {
                environment_keys: vec!["A".to_owned()]
            }
        );
    }

    #[test]
    fn missing_blocks_the_switch() {
        assert!(ProviderKeyStatus::Missing {
            provider: "tokenlab".to_owned(),
            environment_keys: vec!["TOKENLAB_API_KEY".to_owned()],
        }
        .blocks());
        assert!(!ProviderKeyStatus::Available.blocks());
        assert!(!ProviderKeyStatus::Unknown.blocks());
    }
}
