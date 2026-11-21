//! Cached lookups of provider keys in the host's secrets file.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use norte_code_protocol::keys::{environment_value, read_secret, secrets_path};

thread_local! {
    /// Disk lookups are rare but happen on every catalog refresh, so keep
    /// the result until a key is saved or the secrets file moves.
    static KEY_STATUS: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

pub(crate) fn cache_key(path: &Path, provider_id: &str) -> String {
    format!("{}:{}", path.display(), provider_id)
}

/// Whether a provider has a usable key, on disk or in the environment.
pub(crate) fn provider_has_key(provider_id: &str, environment_keys: &[String]) -> bool {
    if environment_keys.is_empty() || environment_value(environment_keys).is_some() {
        return true;
    }
    let Some(path) = secrets_path() else {
        return false;
    };
    let key = cache_key(&path, provider_id);
    if let Some(cached) = KEY_STATUS.with(|cache| cache.borrow().get(&key).copied()) {
        return cached;
    }
    let present = read_secret(&path, provider_id).is_some();
    KEY_STATUS.with(|cache| {
        cache.borrow_mut().insert(key, present);
    });
    present
}

/// Forget the cached status after the host stores a key.
pub(crate) fn invalidate_key(provider_id: &str) {
    let Some(path) = secrets_path() else {
        return;
    };
    let key = cache_key(&path, provider_id);
    KEY_STATUS.with(|cache| {
        cache.borrow_mut().remove(&key);
    });
}
