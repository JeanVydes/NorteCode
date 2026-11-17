//! The host view: what the server offers, section by section.
//!
//! The overview reads as a dashboard: health badges, a small key-value table,
//! then capabilities and limits. The other sections are lists whose rows carry
//! a state color, so a failed MCP server or a denied tool is visible at a
//! glance.

mod overview;

pub(crate) use overview::overview;
