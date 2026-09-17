//! Single source of truth for tunable constants (ARCHITECTURE.md §9).
//! Import these everywhere instead of hardcoding a value inline — a
//! hardcoded duplicate is exactly the kind of drift a "single source of
//! truth" is supposed to prevent.
//!
//! The runtime-configurable `Config` struct (§9.1) belongs to Milestone 6,
//! once the server exists to read it. These plain consts are needed much
//! earlier — Milestone 2's payload-budget tests reference `MAX_PAYLOAD_BYTES`
//! and `MAX_PLAYERS` — so they're their own module now rather than bundled
//! with a struct that doesn't exist yet.

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
