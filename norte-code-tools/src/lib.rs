//! The tools that Norte does not ship.
//!
//! `glob`, `list_dir`, `grep`, `todo`, `ask_user` and `web_fetch` (opt-in).
//! Every tool follows Norte's conventions: strict argument parsing, bounded
//! output, failures returned as an error tool result rather than an engine
//! error, and the cancellation token honoured.

#![forbid(unsafe_code)]
