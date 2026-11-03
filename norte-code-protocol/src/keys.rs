//! Provider keys: where a host finds one and where a typed one is saved.
//!
//! The catalogue names the environment variables a provider accepts
//! (`environment_keys`). A host may install a [`KeyStore`] on the
//! [`Config`](crate::config::Config) to add the OS keyring and the 0600
//! `secrets.json` fallback; without one, only the environment is read.
//!
//! The file is a flat object keyed by provider id:
//!
//! ```json
//! { "ollamacloud": "sk-..." }
//! ```
//!
//! It is written atomically with mode 0600, and never holds a project key: the
//! path is always the user's configuration directory.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{ApiKey, ConfigError, atomic_write_with, user_config_path};

/// Where a typed key was stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyStorage {
    /// The OS keyring.
    Keyring,
    /// The 0600 `secrets.json` file.
    File,
}

/// A failure while saving a key.
#[derive(Clone, Debug, thiserror::Error)]
pub enum KeyStoreError {
    /// This host has no store that can persist a key.
    #[error("this host has no key store")]
    Unsupported,
    /// The value to save was empty.
    #[error("the key is empty")]
    Empty,
    /// The OS keyring refused the key.
    #[error("the OS keyring refused the key: {0}")]
    Keyring(String),
    /// The 0600 file could not be written.
    #[error("could not write the key file: {0}")]
    File(String),
    /// Neither store accepted the key.
    #[error(
        "the OS keyring refused the key ({keyring}) and the key file could not be written ({file})"
    )]
    Both {
        /// Why the keyring failed.
        keyring: String,
        /// Why the file failed.
        file: String,
    },
}

/// A host's provider-key source.
///
/// [`resolve`](Self::resolve) is the chain a request uses: environment, the
/// host's cache, the OS keyring and the 0600 file. [`known`](Self::known) is
/// the subset available without asking the OS store, for redaction.
pub trait KeyStore: Send + Sync + fmt::Debug {
    /// Resolves a provider's key, or explains why none was found.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Secret`] when no source holds a key.
    fn resolve(&self, provider: &str, environment_keys: &[String]) -> Result<ApiKey, ConfigError>;

    /// The key already known without querying the OS keyring.
    ///
    /// This is what a redaction pass uses: a provider whose key sits in the OS
    /// keyring but was never used this run does not unlock it. An empty
    /// `environment_keys` list returns the empty key a local endpoint uses.
    fn known(&self, provider: &str, environment_keys: &[String]) -> Option<ApiKey>;

    /// Saves a key the person typed.
    ///
    /// # Errors
    ///
    /// Returns [`KeyStoreError`] when neither the OS keyring nor the 0600 file
    /// accepted it.
    fn save(&self, provider: &str, key: &str) -> Result<KeyStorage, KeyStoreError>;

    /// Removes a key the person no longer wants stored.
    ///
    /// A missing entry is not an error: there is nothing to remove. The
    /// environment is not touched — a variable set in the shell stays set.
    ///
    /// # Errors
    ///
    /// Returns [`KeyStoreError`] when neither store could be updated.
    fn delete(&self, provider: &str) -> Result<(), KeyStoreError>;
}

/// The key in the catalogue's environment variables, or `None`.
///
/// The first variable that is set and whose value is not blank wins.
#[must_use]
pub fn environment_value(environment_keys: &[String]) -> Option<ApiKey> {
    for name in environment_keys {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(ApiKey::new(value.to_owned()));
            }
        }
    }
    None
}

/// The environment key, or the error a provider with no key reports.
///
/// A provider that names no variable needs no credential: that is a local
/// endpoint, and it resolves to an empty key.
///
/// # Errors
///
/// Returns [`ConfigError::Secret`] when the provider names variables and none
/// of them holds a value.
pub fn environment_key(provider: &str, environment_keys: &[String]) -> Result<ApiKey, ConfigError> {
    if environment_keys.is_empty() {
        return Ok(ApiKey::new(String::new()));
    }
    if let Some(key) = environment_value(environment_keys) {
        return Ok(key);
    }
    Err(ConfigError::Secret {
        provider: provider.to_owned(),
        message: format!("no value in any of: {}", environment_keys.join(", ")),
    })
}

/// The user-level `secrets.json`, beside `config.json`.
#[must_use]
pub fn secrets_path() -> Option<PathBuf> {
    user_config_path().map(|path| path.with_file_name("secrets.json"))
}

/// The key stored in `path` for `provider`, when there is one.
///
/// A missing or malformed file reads as no keys; the file is a cache of typed
/// keys, not a configuration that should stop a run.
#[must_use]
pub fn read_secret(path: &Path, provider: &str) -> Option<ApiKey> {
    let value = read_secrets(path).remove(provider)?;
    let value = value.trim();
    (!value.is_empty()).then(|| ApiKey::new(value.to_owned()))
}

/// Stores `key` under `provider`, preserving the other entries.
///
/// The file is written atomically and always ends with mode 0600, even when it
/// already existed with broader permissions.
///
/// # Errors
///
/// Returns [`KeyStoreError::File`] when the file cannot be read, serialized or
/// written.
pub fn save_secret(path: &Path, provider: &str, key: &str) -> Result<(), KeyStoreError> {
    let mut secrets = read_secrets(path);
    secrets.insert(provider.to_owned(), key.to_owned());
    let body = serde_json::to_vec_pretty(&secrets)
        .map_err(|error| KeyStoreError::File(error.to_string()))?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| KeyStoreError::File(format!("{}: {error}", parent.display())))?;
    }
    atomic_write_with(path, &body, true).map_err(|error| KeyStoreError::File(error.to_string()))
}

/// Removes `provider` from the 0600 file, preserving the other entries.
///
/// A missing file or a missing entry is not an error: there is nothing to
/// remove. When no entries remain the file is removed, so an empty
/// `secrets.json` never lingers.
///
/// # Errors
///
/// Returns [`KeyStoreError::File`] when the file cannot be read, serialized or
/// written.
pub fn delete_secret(path: &Path, provider: &str) -> Result<(), KeyStoreError> {
    let mut secrets = read_secrets(path);
    if secrets.remove(provider).is_none() {
        // Nothing to do: the entry was never there.
        return Ok(());
    }
    if secrets.is_empty() {
        // Drop the file rather than leaving an empty object behind.
        if path.exists() {
            fs::remove_file(path).map_err(|error| KeyStoreError::File(error.to_string()))?;
        }
        Ok(())
    } else {
        let body = serde_json::to_vec_pretty(&secrets)
            .map_err(|error| KeyStoreError::File(error.to_string()))?;
        atomic_write_with(path, &body, true).map_err(|error| KeyStoreError::File(error.to_string()))
    }
}

fn read_secrets(path: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temporary(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "norte-code-keys-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn a_present_variable_wins_over_an_absent_one() {
        // The environment is read, not mutated: other tests share this process.
        let Ok(path) = std::env::var("PATH") else {
            eprintln!("skipping: PATH is not set in this environment");
            return;
        };
        let key = environment_key(
            "p",
            &["NORTE_CODE_TEST_ABSENT_KEY".to_owned(), "PATH".to_owned()],
        )
        .expect("key");
        assert_eq!(key.expose(), path.trim());
    }

    #[test]
    fn a_provider_with_no_variable_needs_no_key() {
        let key = environment_key("p", &[]).expect("key");
        assert_eq!(key.expose(), "");
    }

    #[test]
    fn a_missing_key_names_what_was_tried() {
        let error = environment_key(
            "ollamacloud",
            &[
                "NORTE_CODE_TEST_MISSING_A".to_owned(),
                "NORTE_CODE_TEST_MISSING_B".to_owned(),
            ],
        )
        .expect_err("no key");
        let message = error.to_string();
        assert!(message.contains("ollamacloud"), "{message}");
        assert!(message.contains("NORTE_CODE_TEST_MISSING_A"), "{message}");
        assert!(message.contains("NORTE_CODE_TEST_MISSING_B"), "{message}");
    }

    #[test]
    fn a_saved_key_round_trips_and_keeps_its_neighbours() {
        let path = temporary("secrets.json");
        save_secret(&path, "one", " first ").expect("save one");
        save_secret(&path, "two", "second").expect("save two");
        assert_eq!(read_secret(&path, "one").expect("one").expose(), "first");
        assert_eq!(read_secret(&path, "two").expect("two").expose(), "second");
        assert!(read_secret(&path, "three").is_none());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_corrupt_file_reads_as_empty_and_is_replaced() {
        let path = temporary("corrupt.json");
        fs::write(&path, "{not json").expect("write");
        assert!(read_secret(&path, "one").is_none());
        save_secret(&path, "one", "key").expect("save");
        assert_eq!(read_secret(&path, "one").expect("one").expose(), "key");
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_key_is_private_even_over_a_loose_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = temporary("private.json");
        fs::write(&path, "{}").expect("write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
        save_secret(&path, "one", "key").expect("save");
        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the file holds a key");
        let _ = fs::remove_file(path);
    }
}
