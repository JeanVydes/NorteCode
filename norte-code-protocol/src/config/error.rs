//! What can go wrong reading a configuration.

use std::path::PathBuf;

use super::Config;

/// The result of a load: the config plus the keys that were ignored.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// The merged configuration.
    pub config: Config,
    /// Unknown keys, as dotted paths, to report to the user.
    pub warnings: Vec<String>,
}

/// A configuration failure.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A file was not valid JSON, or did not match the schema.
    #[error("{path} is not valid config: {source}")]
    Parse {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: serde_json::Error,
    },
    /// A file could not be written.
    #[error("could not write {path}: {source}")]
    Write {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A value was present but wrong.
    #[error("config is invalid: {path}: {message}")]
    Invalid {
        /// The offending field.
        path: String,
        /// What is wrong with it.
        message: String,
    },
    /// An API key could not be resolved.
    #[error("could not resolve the API key for {provider}: {message}")]
    Secret {
        /// The provider.
        provider: String,
        /// What went wrong.
        message: String,
    },
}
