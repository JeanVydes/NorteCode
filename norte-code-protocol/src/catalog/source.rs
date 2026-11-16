//! Where the two catalogue files live.
//!
//! They sit beside the configuration, because they are the same kind of thing:
//! a file a person owns and edits, read at startup and never fetched. `XDG` is
//! honoured when it is set, and the absence of both `XDG_CONFIG_HOME` and `HOME`
//! is reported rather than guessed at — a relative path would resolve against
//! whatever directory the process happened to start in and point at a file that
//! was never written, which is exactly how a catalogue looks empty when it is
//! not.
//!
//! The files are written when they are missing from the built-in defaults,
//! giving a first run a usable `OpenRouter` catalogue without overwriting a
//! catalogue the person already owns.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::schema::CatalogError;

/// The built-in provider catalogue.
const DEFAULT_PROVIDERS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../default/providers.json"
));

/// The built-in model catalogue.
const DEFAULT_MODELS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../default/models.json"
));

/// The directory the catalogue files live in.
///
/// # Errors
///
/// Returns a message naming the missing variable when neither
/// `XDG_CONFIG_HOME` nor `HOME` is set.
pub fn catalog_dir() -> Result<PathBuf, String> {
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config).join("norte-code"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".config").join("norte-code"));
    }
    Err(
        "neither XDG_CONFIG_HOME nor HOME is set, so the catalogue has no location; \
         set HOME, or XDG_CONFIG_HOME to name it directly"
            .to_owned(),
    )
}

/// The providers file.
///
/// # Errors
///
/// Returns a message naming the missing variable when neither
/// `XDG_CONFIG_HOME` nor `HOME` is set.
pub fn providers_path() -> Result<PathBuf, String> {
    Ok(catalog_dir()?.join("providers.json"))
}

/// The models file.
///
/// # Errors
///
/// Returns a message naming the missing variable when neither
/// `XDG_CONFIG_HOME` nor `HOME` is set.
pub fn models_path() -> Result<PathBuf, String> {
    Ok(catalog_dir()?.join("models.json"))
}

/// The files a call to [`ensure_catalog_files`] wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CreatedCatalog {
    /// Whether `providers.json` was written.
    pub providers: bool,
    /// Whether `models.json` was written.
    pub models: bool,
}

impl CreatedCatalog {
    /// Whether anything was written.
    #[must_use]
    pub fn any(&self) -> bool {
        self.providers || self.models
    }
}

/// Writes the two catalogue files when they are missing.
///
/// A file that is already there is never touched: what a person wrote is theirs,
/// and a catalogue that is empty because they emptied it stays empty.
///
/// What is written is the checked-in default catalogue. It can be edited after
/// creation, and existing files are never replaced.
///
/// # Errors
///
/// Returns [`CatalogError`] when the directory cannot be found or created, and
/// when a file that was missing cannot be written.
pub fn ensure_catalog_files() -> Result<CreatedCatalog, CatalogError> {
    let dir = catalog_dir().map_err(|message| CatalogError::NoDirectory { message })?;
    ensure_in(&dir)
}

/// Writes the two files in `dir` when they are missing.
fn ensure_in(dir: &Path) -> Result<CreatedCatalog, CatalogError> {
    std::fs::create_dir_all(dir).map_err(|source| CatalogError::Write {
        path: dir.to_path_buf(),
        source,
    })?;
    let providers = write_missing(&dir.join("providers.json"), DEFAULT_PROVIDERS)?;
    let models = write_missing(&dir.join("models.json"), DEFAULT_MODELS)?;
    Ok(CreatedCatalog { providers, models })
}

/// Writes `body` to `path` unless the path is already there.
///
/// The file is created exclusively, so two processes racing here leave one
/// winner and one file: neither of them writes over what the other wrote.
fn write_missing(path: &Path, body: &str) -> Result<bool, CatalogError> {
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(source) => {
            return Err(CatalogError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    file.write_all(body.as_bytes())
        .map_err(|source| CatalogError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(true)
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
