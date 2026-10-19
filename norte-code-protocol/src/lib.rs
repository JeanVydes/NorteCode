//! The Norte Code client protocol.
//!
//! Versioned serde types only. Nothing here depends on Norte: the service maps
//! between these types and the engine, so a client does not compile the engine
//! and the wire can move on its own schedule.
//!
//! The two halves of the wire are the client request and the server event.
//! They are landed milestone by milestone; the crate is empty until the
//! service exists to serve it.

#![forbid(unsafe_code)]
