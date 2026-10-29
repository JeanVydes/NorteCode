//! The catalogue: where models come from and what they can do.
//!
//! The catalogue is two files a person owns, under the configuration directory:
//! `providers.json` says who serves models and over which wires, and
//! `models.json` is a flat list of the models themselves. Nothing here fetches
//! anything: loading blocks on no network, and a file that is missing is an
//! error that names it rather than a snapshot to fall back on.
//!
//! A plugin may build the same two shapes from wherever it likes — one reads
//! models.dev, one reads an `OpenRouter` listing — and hand them to
//! [`Catalog::new`], which is the same validation the files go through.

#![forbid(unsafe_code)]

pub mod map;
pub mod schema;
pub mod source;

pub use map::{capabilities, modality_of, reasoning};
pub use schema::{
    Catalog, CatalogError, Limit, Modalities, Modality, Model, ModelEntry, ModelStatus, Models,
    Pricing, Provider, Providers, Reasoning, SupportedEffort, SupportedParams, Wire, WireSchema,
    load_catalog_files,
};
pub use source::{catalog_dir, models_path, providers_path};
