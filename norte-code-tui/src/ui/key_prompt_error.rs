//! Why a key prompt could not store a secret.

use std::error::Error;
use std::fmt;

/// A failed attempt to store a provider key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum KeyPromptError {
    /// The prompt had no provider to store a key for.
    NoProvider,
    /// The host environment has no secrets file location.
    NoSecretsPath,
    /// The secrets file rejected the write.
    Save(String),
}

impl fmt::Display for KeyPromptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProvider => formatter.write_str("no provider"),
            Self::NoSecretsPath => formatter.write_str("no secrets path"),
            Self::Save(error) => formatter.write_str(error),
        }
    }
}

impl Error for KeyPromptError {}
