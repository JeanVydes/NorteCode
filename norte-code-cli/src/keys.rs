//! The host's provider-key store: environment, cache, OS keyring, 0600 file.
//!
//! The order is the one a person expects: a variable already exported wins, a
//! key saved earlier in this process is next, then the OS credential store, and
//! finally the 0600 `secrets.json` fallback. A key typed at `/model` or
//! `/models` is saved to the keyring, or to the file when the keyring is not
//! available.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use norte_code_protocol::config::{ApiKey, ConfigError};
use norte_code_protocol::keys::{
    KeyStorage, KeyStore, KeyStoreError, environment_value, read_secret, save_secret, secrets_path,
};

/// The service name every entry is stored under in the OS credential store.
///
/// The account is the provider id (`ollamacloud`), so a person looking in
/// Keychain Access or Credential Manager finds one entry per provider.
const SERVICE: &str = "norte-code";

/// The CLI's key store.
#[derive(Debug)]
pub(crate) struct PlatformKeys {
    path: Option<PathBuf>,
    backend: Arc<dyn SecretBackend>,
    cache: Mutex<BTreeMap<String, ApiKey>>,
}

impl PlatformKeys {
    /// The store a session uses: the OS keyring and the user's `secrets.json`.
    pub(crate) fn new() -> Self {
        Self {
            path: secrets_path(),
            backend: Arc::new(OsKeyring),
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    #[cfg(test)]
    fn with(backend: Arc<dyn SecretBackend>, path: Option<PathBuf>) -> Self {
        Self {
            path,
            backend,
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
        // endpoint. Nothing below, including the keyring, is consulted.
        if environment_keys.is_empty() {
            return Ok(ApiKey::new(String::new()));
        }
        if let Some(key) = environment_value(environment_keys) {
            return Ok(key);
        }
        if let Some(key) = self.cached(provider) {
            return Ok(key);
        }
        if let Ok(Some(value)) = self.backend.get(provider) {
            let value = value.trim();
            if !value.is_empty() {
                let key = ApiKey::new(value.to_owned());
                self.remember(provider, &key);
                return Ok(key);
            }
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
        let keyring = self.backend.set(provider, key);
        if keyring.is_ok() {
            self.remember(provider, &stored);
            return Ok(KeyStorage::Keyring);
        }
        let keyring = keyring.expect_err("the error branch");
        let Some(path) = &self.path else {
            self.remember(provider, &stored);
            return Err(KeyStoreError::Both {
                keyring,
                file: "there is no user configuration directory to write secrets.json".to_owned(),
            });
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
                Err(KeyStoreError::Both {
                    keyring,
                    file: error.to_string(),
                })
            }
        }
    }
}

/// One OS credential store, or a test double.
pub(crate) trait SecretBackend: Send + Sync + fmt::Debug {
    fn get(&self, provider: &str) -> Result<Option<String>, String>;
    fn set(&self, provider: &str, key: &str) -> Result<(), String>;
}

/// The platform credential store.
#[derive(Debug, Default)]
struct OsKeyring;

impl SecretBackend for OsKeyring {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        let provider = provider.to_owned();
        in_thread(move || {
            let entry =
                keyring::Entry::new(SERVICE, &provider).map_err(|error| error.to_string())?;
            match entry.get_password() {
                Ok(secret) => Ok(Some(secret)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        })
    }

    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        let provider = provider.to_owned();
        let key = key.to_owned();
        in_thread(move || {
            keyring::Entry::new(SERVICE, &provider)
                .map_err(|error| error.to_string())?
                .set_password(&key)
                .map_err(|error| error.to_string())
        })
    }
}

/// Runs one keyring call off the async runtime's thread.
///
/// The Linux Secret Service backend blocks internally, and doing that on the
/// thread that drives Tokio can deadlock; a short-lived thread of our own has
/// no runtime to nest inside.
fn in_thread<T: Send + 'static>(
    call: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    std::thread::spawn(call)
        .join()
        .map_err(|_| "the keyring thread stopped unexpectedly".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    #[derive(Debug, Default)]
    struct MemoryBackend {
        entries: Mutex<BTreeMap<String, String>>,
        reads: AtomicUsize,
        fail: AtomicBool,
    }

    impl MemoryBackend {
        fn failing() -> Self {
            Self {
                fail: AtomicBool::new(true),
                ..Self::default()
            }
        }
    }

    impl SecretBackend for MemoryBackend {
        fn get(&self, provider: &str) -> Result<Option<String>, String> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.fail.load(Ordering::Relaxed) {
                return Err("the keyring is locked".to_owned());
            }
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(provider)
                .cloned())
        }

        fn set(&self, provider: &str, key: &str) -> Result<(), String> {
            if self.fail.load(Ordering::Relaxed) {
                return Err("the keyring is locked".to_owned());
            }
            self.entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(provider.to_owned(), key.to_owned());
            Ok(())
        }
    }

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
        let backend = Arc::new(MemoryBackend::default());
        backend
            .entries
            .lock()
            .expect("entries")
            .insert("p".to_owned(), "from keyring".to_owned());
        let file = temporary("env.json");
        save_secret(&file, "p", "from file").expect("save");
        let keys = PlatformKeys::with(backend, Some(file.clone()));
        let key = keys
            .resolve("p", &[environment_provider().to_owned()])
            .expect("key");
        assert_eq!(key.expose(), path.trim());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn the_keyring_wins_over_the_file() {
        let backend = Arc::new(MemoryBackend::default());
        backend
            .entries
            .lock()
            .expect("entries")
            .insert("p".to_owned(), "from keyring".to_owned());
        let file = temporary("keyring.json");
        save_secret(&file, "p", "from file").expect("save");
        let keys = PlatformKeys::with(backend, Some(file.clone()));
        let key = keys
            .resolve("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
            .expect("key");
        assert_eq!(key.expose(), "from keyring");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_failing_keyring_falls_through_to_the_file() {
        let file = temporary("fallback.json");
        save_secret(&file, "p", "from file").expect("save");
        let keys = PlatformKeys::with(Arc::new(MemoryBackend::failing()), Some(file.clone()));
        let key = keys
            .resolve("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
            .expect("key");
        assert_eq!(key.expose(), "from file");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_local_endpoint_needs_no_store() {
        let backend = Arc::new(MemoryBackend::default());
        let keys = PlatformKeys::with(backend.clone(), None);
        let key = keys.resolve("local", &[]).expect("key");
        assert_eq!(key.expose(), "");
        assert_eq!(backend.reads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn saving_prefers_the_keyring() {
        let backend = Arc::new(MemoryBackend::default());
        let file = temporary("save-keyring.json");
        let keys = PlatformKeys::with(backend.clone(), Some(file.clone()));
        assert_eq!(
            keys.save("p", " secret ").expect("save"),
            KeyStorage::Keyring
        );
        assert_eq!(
            backend
                .entries
                .lock()
                .expect("entries")
                .get("p")
                .map(String::as_str),
            Some("secret")
        );
        assert!(
            !file.exists(),
            "the file is not used when the keyring works"
        );
    }

    #[test]
    fn saving_falls_back_to_the_private_file() {
        let file = temporary("save-file.json");
        let keys = PlatformKeys::with(Arc::new(MemoryBackend::failing()), Some(file.clone()));
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
        let keys = PlatformKeys::with(Arc::new(MemoryBackend::failing()), None);
        assert!(matches!(
            keys.save("p", "secret"),
            Err(KeyStoreError::Both { .. })
        ));
        let key = keys
            .resolve("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
            .expect("key");
        assert_eq!(key.expose(), "secret");
    }

    #[test]
    fn known_never_asks_the_keyring() {
        let backend = Arc::new(MemoryBackend::default());
        backend
            .entries
            .lock()
            .expect("entries")
            .insert("p".to_owned(), "from keyring".to_owned());
        let keys = PlatformKeys::with(backend.clone(), None);
        assert!(
            keys.known("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
                .is_none()
        );
        assert_eq!(backend.reads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_saved_key_resolves_without_a_second_lookup() {
        let backend = Arc::new(MemoryBackend::default());
        let keys = PlatformKeys::with(backend.clone(), None);
        keys.save("p", "secret").expect("save");
        let reads = backend.reads.load(Ordering::Relaxed);
        let key = keys
            .resolve("p", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
            .expect("key");
        assert_eq!(key.expose(), "secret");
        assert_eq!(backend.reads.load(Ordering::Relaxed), reads);
    }
}
