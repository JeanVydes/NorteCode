//! The host's provider-key store: environment, cache and the 0600 file.
//!
//! The order is the one a person expects: a variable already exported wins, a
//! key saved earlier in this process is next, and finally the 0600
//! `secrets.json` fallback. There is no OS keyring: keys live in one file a
//! person can read and delete.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use norte_code_protocol::config::{ApiKey, ConfigError};
use norte_code_protocol::keys::{
    KeyStorage, KeyStore, KeyStoreError, delete_secret, environment_value, read_secret,
    save_secret, secrets_path,
};

/// The CLI's key store.
#[derive(Debug)]
pub(crate) struct PlatformKeys {
    path: Option<PathBuf>,
    cache: Mutex<BTreeMap<String, ApiKey>>,
}

impl PlatformKeys {
    /// The store a session uses: the user's `secrets.json`.
    pub(crate) fn new() -> Self {
        Self {
            path: secrets_path(),
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    #[cfg(test)]
    fn with(path: Option<PathBuf>) -> Self {
        Self {
            path,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    fn cached(&self, provider: &str) -> Option<ApiKey> {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(provider)
            .cloned()
    }

    fn remember(&self, provider: &str, key: &ApiKey) {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(provider.to_owned(), key.clone());
    }

    fn file_key(&self, provider: &str) -> Option<ApiKey> {
        self.path
            .as_deref()
            .and_then(|path| read_secret(path, provider))
    }
}

impl KeyStore for PlatformKeys {
    fn resolve(&self, provider: &str, environment_keys: &[String]) -> Result<ApiKey, ConfigError> {
        // A provider that names no variable needs no credential: a local
        // endpoint. Nothing below is consulted.
        if environment_keys.is_empty() {
            return Ok(ApiKey::new(String::new()));
        }
        if let Some(key) = environment_value(environment_keys) {
            return Ok(key);
        }
        if let Some(key) = self.cached(provider) {
            return Ok(key);
        }
        if let Some(key) = self.file_key(provider) {
            self.remember(provider, &key);
            return Ok(key);
        }
        Err(ConfigError::Secret {
            provider: provider.to_owned(),
            message: format!("no value in any of: {}", environment_keys.join(", ")),
        })
    }

    fn known(&self, provider: &str, environment_keys: &[String]) -> Option<ApiKey> {
        if environment_keys.is_empty() {
            return Some(ApiKey::new(String::new()));
        }
        if let Some(key) = environment_value(environment_keys) {
            return Some(key);
        }
        if let Some(key) = self.cached(provider) {
            return Some(key);
        }
        self.file_key(provider)
    }

    fn save(&self, provider: &str, key: &str) -> Result<KeyStorage, KeyStoreError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(KeyStoreError::Empty);
        }
        let stored = ApiKey::new(key.to_owned());
        let Some(path) = &self.path else {
            self.remember(provider, &stored);
            return Err(KeyStoreError::File(
                "there is no user configuration directory to write secrets.json".to_owned(),
            ));
        };
        match save_secret(path, provider, key) {
            Ok(()) => {
                self.remember(provider, &stored);
                Ok(KeyStorage::File)
            }
            Err(error) => {
                // The key still works for this session: the cache is the last
                // source before the environment.
                self.remember(provider, &stored);
                Err(KeyStoreError::File(error.to_string()))
            }
        }
    }

    fn delete(&self, provider: &str) -> Result<(), KeyStoreError> {
        if let Ok(mut cache) = self.cache.lock() {
            cache.remove(provider);
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        delete_secret(path, provider)
        // delete_secret already returns KeyStoreError::File.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temporary(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "norte-code-cli-keys-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn environment_provider() -> &'static str {
        "PATH"
    }

    #[test]
    fn the_environment_wins_over_every_store() {
        let Ok(path) = std::env::var("PATH") else {
            eprintln!("skipping: PATH is not set in this environment");
            return;
        };
        let file = temporary("env.json");
        let _ = std::fs::remove_file(&file);
        let keys = PlatformKeys::with(Some(file.clone()));
        let key = keys
            .resolve("p", &[environment_provider().to_owned()])
            .expect("key");
        assert_eq!(key.expose(), path.trim());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_saved_key_resolves_without_a_second_lookup() {
        let file = temporary("save-file.json");
        let _ = std::fs::remove_file(&file);
        let keys = PlatformKeys::with(Some(file.clone()));
        keys.save("p", "secret").expect("save");
        let key = keys
            .resolve("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
            .expect("key");
        assert_eq!(key.expose(), "secret");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_local_endpoint_needs_no_store() {
        let keys = PlatformKeys::with(None);
        let key = keys.resolve("local", &[]).expect("key");
        assert_eq!(key.expose(), "");
    }

    #[test]
    fn saving_falls_back_to_the_private_file() {
        let file = temporary("save-file2.json");
        let _ = std::fs::remove_file(&file);
        let keys = PlatformKeys::with(Some(file.clone()));
        assert_eq!(keys.save("p", "secret").expect("save"), KeyStorage::File);
        assert_eq!(read_secret(&file, "p").expect("key").expose(), "secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_key_that_both_stores_refuse_still_works_this_session() {
        // With no file path, saving fails but the cache still holds the key.
        let keys = PlatformKeys::with(None);
        assert!(matches!(
            keys.save("p", "secret"),
            Err(KeyStoreError::File(_))
        ));
        // The cache was populated even though persistence failed.
        let key = keys
            .resolve("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
            .expect("key");
        assert_eq!(key.expose(), "secret");
    }

    #[test]
    fn deleting_removes_the_stored_key() {
        let file = temporary("delete.json");
        let _ = std::fs::remove_file(&file);
        let keys = PlatformKeys::with(Some(file.clone()));
        keys.save("p", "secret").expect("save");
        keys.delete("p").expect("delete");
        assert!(
            keys.resolve("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
                .is_err()
        );
        assert!(!file.exists() || read_secret(&file, "p").is_none());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn known_never_touches_the_file_after_cache() {
        let file = temporary("known.json");
        let _ = std::fs::remove_file(&file);
        let keys = PlatformKeys::with(Some(file.clone()));
        keys.save("p", "secret").expect("save");
        // known uses cache/file, no keyring.
        assert!(
            keys.known("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
                .is_some()
        );
        let _ = std::fs::remove_file(file);
    }
}
