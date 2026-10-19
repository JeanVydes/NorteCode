//! The interactive terminal UI.
//!
//! Structured as model / update / view: one `App` state, one reducer that
//! applies server events and key events, and pure render functions testable
//! without a terminal (ratatui's `TestBackend`). The library never writes to
//! stdout and the render loop stays off the network path.

#![forbid(unsafe_code)]
