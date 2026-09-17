//! Single source of truth for tunable constants (ARCHITECTURE.md §9).
//! Import these everywhere instead of hardcoding a value inline — a
//! hardcoded duplicate is exactly the kind of drift a "single source of
//! truth" is supposed to prevent.
//!
//! The runtime-configurable `Config` struct (§9.1) is below, added at
//! Milestone 6 now that the server exists to read it. Several of the
//! plain consts above it can't be `pub const` at the point of use, because
//! server integration tests need to lower them: an idle-timeout test that
//! waits a real 5 seconds is too slow, a capacity test needs to open more
//! sockets than a low `max_players` would allow, and the bind address is
//! `0.0.0.0:PORT` in production but `127.0.0.1:0` under test. A `const`
//! can't be overridden per test, so those live in `Config`, and the
//! server reads the struct — never the constant — at every use site.

use std::net::SocketAddr;

pub const PROTOCOL_VERSION: u16 = 1;
/// Bump on ANY change to maze generation output (`common::maze::generate`).
pub const GENERATOR_VERSION: u16 = 1;

pub const SERVER_TICK_HZ: u16 = 30;
pub const CLIENT_INPUT_HZ: u16 = 30;
pub const INPUT_DT_MS: f32 = 1000.0 / CLIENT_INPUT_HZ as f32;
pub const PLAYER_SPEED_UPS: f32 = 3.0;
pub const MAX_INPUT_QUEUE: usize = 6;
/// 10 humans (audit requirement) + 8 bots + headroom; bots hold real
/// player slots (§4.2), so this must cover both, not just 10.
pub const MAX_PLAYERS: usize = 20;
pub const CLIENT_TIMEOUT_MS: u64 = 5000;
pub const GHOST_TIMEOUT_MS: u64 = 2000;
pub const RETRY_INTERVAL_MS: u64 = 100;
pub const MAX_RETRY_ATTEMPTS: u32 = 10;
pub const JOIN_RETRY_BASE_MS: u64 = 250;
pub const JOIN_MAX_ATTEMPTS: u32 = 6;
pub const INTERP_DELAY_MS: f32 = 100.0;
pub const MAX_EXTRAPOLATION_MS: f32 = 250.0;
pub const SNAPSHOT_BUFFER_LEN: usize = 5;
pub const MAX_RTT_S: f32 = 1.0;
pub const FPS_AVG_WINDOW_FRAMES: usize = 60;
/// Recv buffer size AND the deserializer's read limit — every wire
/// message must fit under this to avoid UDP fragmentation (§3.2).
pub const MAX_PAYLOAD_BYTES: usize = 1200;

/// Production UDP port. Not part of the ARCHITECTURE.md §9 constant table
/// (that section only names the field, "0.0.0.0:PORT") — picked here as
/// the concrete default so `Config::default()` is a complete, runnable
/// value. Tests always override `bind_addr` to `127.0.0.1:0` and never
/// reference this.
pub const DEFAULT_PORT: u16 = 7777;

/// Runtime-configurable server settings (§9.1). `Default` is built from
/// the constants above; every server use site reads a `Config` value —
/// never a bare constant — precisely so a test can shrink `max_players`,
/// shorten `client_timeout_ms`, or bind to an OS-assigned loopback port
/// without touching production behavior.
#[derive(Debug, Clone)]
pub struct Config {
    /// `0.0.0.0:PORT` in production (accepts non-loopback source IPs,
    /// per the audit's "connect from another computer" requirement);
    /// `127.0.0.1:0` (OS-assigned port) in tests.
    pub bind_addr: SocketAddr,
    pub tick_hz: u16,
    pub max_players: usize,
    pub client_timeout_ms: u64,
    pub retry_interval_ms: u64,
    pub max_retry_attempts: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            tick_hz: SERVER_TICK_HZ,
            max_players: MAX_PLAYERS,
            client_timeout_ms: CLIENT_TIMEOUT_MS,
            retry_interval_ms: RETRY_INTERVAL_MS,
            max_retry_attempts: MAX_RETRY_ATTEMPTS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_the_constants_it_is_built_from() {
        let config = Config::default();
        assert_eq!(config.bind_addr.port(), DEFAULT_PORT);
        assert!(config.bind_addr.ip().is_unspecified(), "production default must bind 0.0.0.0, not loopback");
        assert_eq!(config.tick_hz, SERVER_TICK_HZ);
        assert_eq!(config.max_players, MAX_PLAYERS);
        assert_eq!(config.client_timeout_ms, CLIENT_TIMEOUT_MS);
        assert_eq!(config.retry_interval_ms, RETRY_INTERVAL_MS);
        assert_eq!(config.max_retry_attempts, MAX_RETRY_ATTEMPTS);
    }
}
