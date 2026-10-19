//! A `SQLite` [`norte::Store`] and [`norte::BlobStore`] for Norte Code.
//!
//! Implements Norte's durability contract with compare-and-swap on the session
//! version and one transaction per commit, and adds Norte Code's own tables
//! (logical sessions, compaction links, the cost ledger and settings). It
//! passes Norte's `store_conformance` test suite.

#![forbid(unsafe_code)]
