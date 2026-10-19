//! The cost ledger and budgets.
//!
//! Norte Code never prices anything: the only dollar figure is the one the
//! provider reports in a response's usage payload. The ledger stores one row
//! per model request, with the reported cost or nothing, and budgets are
//! evaluated against what was really spent.

#![forbid(unsafe_code)]
