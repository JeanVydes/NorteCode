//! Norte Code configuration.
//!
//! Loads, validates and merges `config.json` (user file plus an optional
//! per-project overlay) and resolves the secrets each configured provider
//! needs. Every key has a documented default and a line-accurate validation
//! error.

#![forbid(unsafe_code)]
