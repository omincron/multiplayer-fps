//! Shared, pure logic for both binaries: wire protocol, reliability layer,
//! maze generation, movement/shooting resolution, interpolation math.
//!
//! This crate does no I/O of any kind — no sockets, no rendering, no clock
//! reads. Every item in it is a pure function of its inputs, which is what
//! lets the whole thing be tested without a server or a window
//! (ARCHITECTURE.md §2).

pub mod config;
pub mod maze;
pub mod protocol;
pub mod reliability;
pub mod types;
