//! Transport-neutral agent commands exposed through RAEP.
//!
//! The enum itself lives in `norte_code_protocol` (see
//! [`norte_code_protocol::AgentCommand`]), the crate both the client and
//! this host already depend on; this module re-exports it so call sites
//! here read naturally.

pub use norte_code_protocol::AgentCommand;
