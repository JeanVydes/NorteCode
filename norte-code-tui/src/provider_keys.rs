//! Whether the reader has a usable key for a provider catalogue entry.
//!
//! The transport keys live either in the environment (`environment_keys`
//! in `providers.json`, first set and non-blank wins) or in the 0600
//! `secrets.json` beside the config. Neither is the TUI's secret to hand
//! to the model or the host; this module only answers yes/no at the moment
//! a model is picked.

use norte_code_protocol::keys::{environment_value, read_secret, secrets_path};

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

/// Checks the environment and the secrets file for one provider.
///
/// Failures to load the catalogue yield [`ProviderKeyStatus::Unknown`], which
/// does not block the switch.
#[must_use]
pub(crate) fn check(provider_id: &str) -> ProviderKeyStatus {
    let keys = match load_environment_keys(provider_id) {
        Err(()) => return ProviderKeyStatus::Unknown,
        Ok(None) => return ProviderKeyStatus::Available,
        Ok(Some(keys)) => keys,
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
