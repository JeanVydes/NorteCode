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
    /// A name matched more than one session.
    ///
    /// The footer shows a session by a prefix of its id, so a client may send
    /// one; when two sessions share it the service says so instead of picking,
    /// because the wrong session is silent and the right one is not.
    #[error("`{prefix}` matches {} session(s): {}", .matches.len(), .matches.join(", "))]
    AmbiguousSession {
        /// The prefix the client sent.
        prefix: String,
        /// The ids it matches.
        matches: Vec<String>,
    },
    /// No running turn is waiting on that question.
    #[error("question `{0}` is not pending for this session")]
    UnknownQuestion(String),
    /// A budget is malformed or cannot be measured.
    #[error("budget: {0}")]
    Budget(String),
    /// A reasoning setting is malformed or unsupported by the model.
    #[error("reasoning: {0}")]
    Reasoning(String),
    /// An approval rule could not be read or written.
    #[error("approval: {0}")]
    Approval(String),
    /// Compaction could not produce a safe summary.
    #[error("compaction: {0}")]
    Compaction(String),
    /// A session could not be named after its first message.
    #[error("titling: {0}")]
    Titling(String),
    /// The feature exists on the wire but is not built yet.
    #[error("not implemented yet: {0}")]
    Unsupported(&'static str),
}
