//! The models.dev catalogue.
//!
//! Fetches and caches the model catalogue, maps an entry to Norte
//! [`norte::Capabilities`] in one place, and never blocks startup on the
//! network: the disk cache, the stale copy and the bundled snapshot are the
//! fallbacks, in that order.

#![forbid(unsafe_code)]
