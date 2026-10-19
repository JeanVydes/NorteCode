//! The service's error type.

use norte_code_config::ConfigError;

/// A failure in the service or in building its providers.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The configuration could not be loaded or a key resolved.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A model reference was not `provider/model`.
    #[error("`{0}` is not a `provider/model` reference")]
    ModelReference(String),
    /// The provider is not one of the four this build knows.
    #[error("unknown provider `{0}`")]
    UnknownProvider(String),
    /// The engine failed.
    #[error(transparent)]
    Norte(#[from] norte::Error),
    /// The named session is not known.
    #[error("session `{0}` is not known")]
    UnknownSession(String),
    /// A provider has no endpoint and no native preset.
    #[error(
        "provider `{provider}` speaks the {wire:?} wire but has no API URL; \
         the catalogue supplies it, or set `providers.{provider}.endpoint`"
    )]
    MissingEndpoint {
        /// The provider id.
        provider: String,
        /// The wire it speaks.
        wire: crate::provider::Wire,
    },
    /// The feature exists on the wire but is not built yet.
    #[error("not implemented yet: {0}")]
    Unsupported(&'static str),
}
