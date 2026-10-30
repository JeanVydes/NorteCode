//! Where the two catalogue files live.
//!
//! They sit beside the configuration, because they are the same kind of thing:
//! a file a person owns and edits, read at startup and never fetched. `XDG` is
//! honoured when it is set, and the absence of both `XDG_CONFIG_HOME` and `HOME`
//! is reported rather than guessed at — a relative path would resolve against
//! whatever directory the process happened to start in and point at a file that
//! was never written, which is exactly how a catalogue looks empty when it is
//! not.

use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_files_sit_beside_the_configuration() {
        // The paths are derived, and a test that set the variables would race
        // every other test in the process: what is asserted is the shape, that
        // both files are under one directory and named as documented.
        let dir = catalog_dir().expect("a directory");
        assert_eq!(providers_path().expect("path"), dir.join("providers.json"));
        assert_eq!(models_path().expect("path"), dir.join("models.json"));
        assert!(dir.ends_with("norte-code"), "{}", dir.display());
    }
}
