//! Library target so `server/tests/` integration tests can call into
//! `net::spawn` directly — a bin-only crate can't be linked by its own
//! integration tests.

pub mod net;
pub mod world;
