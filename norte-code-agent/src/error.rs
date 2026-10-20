//! The service's error type.

use norte_code_config::ConfigError;

/// A failure in the service or in building its providers.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The configuration could not be loaded or a key resolved.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The catalogue could not be loaded or written.
    #[error(transparent)]
    Catalog(#[from] norte_code_catalog::CatalogError),
    /// A model reference was not `provider/model`.
    #[error("`{0}` is not a `provider/model` reference")]
    ModelReference(String),
    /// The provider is not one of the four this build knows.
    #[error("unknown provider `{0}`")]
    UnknownProvider(String),
    /// The provider is known but the catalogue does not list the model.
    #[error("the catalogue does not list `{provider}/{model}`")]
    UnknownModel {
        /// The provider id.
        provider: String,
        /// The model id.
        model: String,
    },
    /// The engine failed.
    #[error(transparent)]
    Norte(#[from] norte::Error),
    /// The named session is not known.
    #[error("session `{0}` is not known")]
    UnknownSession(String),
    /// No running turn is waiting on that question.
    #[error("question `{0}` is not pending for this session")]
    UnknownQuestion(String),
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
    /// The catalogue's endpoint template names an unset environment variable.
    #[error(
        "provider `{provider}` needs the environment variable `{variable}` to build its API URL; \
         set it or configure `providers.{provider}.endpoint`"
    )]
    MissingEndpointVariable {
        /// The provider id.
        provider: String,
        /// The variable the template names.
        variable: String,
    },
    /// The catalogue's endpoint template is malformed.
    #[error("provider `{provider}` has an unusable endpoint template `{template}`")]
    EndpointTemplate {
        /// The provider id.
        provider: String,
        /// The template as the catalogue spells it.
        template: String,
    },
    /// The feature exists on the wire but is not built yet.
    #[error("not implemented yet: {0}")]
    Unsupported(&'static str),
}
