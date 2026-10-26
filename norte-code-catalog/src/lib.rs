//! The models.dev catalogue.
//!
//! The catalogue is the source of provider facts: `npm` selects the wire,
//! `api` is the endpoint, `env` names the key variables, and the model entries
//! carry the limits, modalities and reasoning options Norte Code needs.
//!
//! Loading never blocks startup on the network. The order is always: a fresh
//! disk cache, a conditional request, the stale cache, and finally the bundled
//! snapshot. A bad download never replaces a good cache.

#![forbid(unsafe_code)]

pub mod cache;
pub mod map;
pub mod schema;

pub use cache::{Cache, LoadOptions, Loaded, Source, cache_dir, load_catalog};
pub use map::{Wire, capabilities, modality_of_kind, reasoning};
pub use schema::{
    Catalog, CatalogError, Limit, Modalities, Model, ModelEntry, Provider, ReasoningOption,
};
