//! Reading and merging configuration files.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{ConfigError, MAX_CONFIG_BYTES};

/// Reads a regular file, refusing a special file and a size over `limit`.
pub(super) fn read_limited(path: &Path, limit: usize) -> std::io::Result<String> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let file = fs::File::open(path)?;
    let mut text = String::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_string(&mut text)?;
    if text.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file is larger than {limit} bytes"),
        ));
    }
    Ok(text)
}

/// Reads a JSON file into a value.
pub(super) fn read_value(path: &Path) -> Result<Value, ConfigError> {
    let text = read_limited(path, MAX_CONFIG_BYTES).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    read_text(&text).map_err(|error| match error {
        ConfigError::Parse { source, .. } => ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        },
        other => other,
    })
}

/// Parses JSON text.
pub(super) fn read_text(text: &str) -> Result<Value, ConfigError> {
    serde_json::from_str(text).map_err(|source| ConfigError::Parse {
        path: PathBuf::from("<memory>"),
        source,
    })
}

/// Deep-merges `overlay` into `base`; `overlay` wins. Objects merge, everything
/// else replaces.
pub(super) fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay,
    }
}

pub(super) fn invalid(path: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        path: path.into(),
        message: message.into(),
    }
}

/// Checks a provider and a model that are written together.
///
/// Both or neither: a model with no provider cannot be resolved, and a provider
/// with no model names half a session. Neither is checked for a slash — a model
/// id is a string that may carry one, and the provider is its own key.
pub(super) fn validate_pair(
    provider_path: &str,
    model_path: &str,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<(), ConfigError> {
    match (provider, model) {
        (Some(provider), Some(_)) if provider.trim().is_empty() => {
            Err(invalid(provider_path, "must not be empty"))
        }
        (Some(_), None) => Err(invalid(
            provider_path,
            format!("is set without a {model_path} to go with it"),
        )),
        (None, Some(_)) => Err(invalid(
            model_path,
            format!("is set without a {provider_path} to go with it"),
        )),
        _ => Ok(()),
    }
}
