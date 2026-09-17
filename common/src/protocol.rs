//! Wire message types (ARCHITECTURE.md §3.2) and their (de)serialization.
//!
//! Every message stays under `MAX_PAYLOAD_BYTES` (checked by this module's
//! own tests) to avoid UDP fragmentation, and the deserializer is always
//! configured with that same limit as a read cap: the socket accepts
//! arbitrary source IPs (§1.6), so a hostile datagram declaring an
//! enormous `String`/`Vec` length is a reachable attack, not a
//! hypothetical one.

use serde::{Deserialize, Serialize};

use crate::config::MAX_PAYLOAD_BYTES;
use crate::maze::MazeSource;
use crate::types::{PlayerId, Tick, Vec2};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    Join {
        name: String,
        protocol_version: u16,
        generator_version: u16,
    },
    /// One `Input` == exactly one fixed simulation step of `INPUT_DT_MS`.
    /// `move_dir` is a *direction*, not a displacement: the server clamps
    /// it to unit length and applies its own speed (§4.3).
    Input {
        input_tick: Tick,
        move_dir: Vec2,
        facing: f32,
        shoot: bool,
    },
    /// Reliability-layer ack (§3.3).
    Ack {
        event_id: u32,
    },
    Ping {
        nonce: u32,
    },
    Leave,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        player_id: PlayerId,
        level: u8,
        level_epoch: u32,
        maze: MazeSource,
        tick_rate_hz: u16,
        server_tick: Tick,
        server_time_ms: u64,
    },
    /// e.g. server full, name taken.
    Rejected {
        reason: String,
    },
    WorldState {
        tick: Tick,
        /// Lets a client discard snapshots for a maze it doesn't have yet
        /// (§4.4) — `LevelChanged` is reliable-but-retried, so several
        /// mismatched snapshots reliably arrive before it does.
        level_epoch: u32,
        players: Vec<PlayerSnapshot>,
    },
    /// Reliable channel (§3.3): join/leave/hit/death/level-change.
    Event {
        event_id: u32,
        kind: EventKind,
    },
    Pong {
        nonce: u32,
        server_tick: Tick,
        server_time_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventKind {
    PlayerJoined { id: PlayerId, name: String },
    PlayerLeft { id: PlayerId },
    Hit { shooter: PlayerId, target: PlayerId, target_hp: u8 },
    Killed { victim: PlayerId, killer: PlayerId },
    Respawned { id: PlayerId, pos: Vec2 },
    LevelChanged { level: u8, level_epoch: u32, maze: MazeSource },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub id: PlayerId,
    pub pos: Vec2,
    pub facing: f32,
    pub hp: u8,
    /// Highest `input_tick` from this player the server has consumed.
    /// Only meaningful to that player; required for reconciliation (§6.3).
    pub last_input_tick: Tick,
}

fn wire_config() -> impl bincode::config::Config {
    bincode::config::standard().with_limit::<MAX_PAYLOAD_BYTES>()
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, bincode::error::EncodeError> {
    bincode::serde::encode_to_vec(msg, wire_config())
}

pub fn decode<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<T, bincode::error::DecodeError> {
    bincode::serde::decode_from_slice(bytes, wire_config()).map(|(value, _consumed)| value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MAX_PLAYERS, PROTOCOL_VERSION};
    use crate::maze::{MazeSpec, GENERATOR_VERSION};

    fn sample_maze_source() -> MazeSource {
        MazeSource::Generated(MazeSpec {
            seed: 1,
            width: 10,
            height: 10,
            braid_factor: 0.3,
            generator_version: GENERATOR_VERSION,
        })
    }

    fn sample_client_msgs() -> Vec<ClientMsg> {
        vec![
            ClientMsg::Join {
                name: "george".into(),
                protocol_version: PROTOCOL_VERSION,
                generator_version: GENERATOR_VERSION,
            },
            ClientMsg::Input {
                input_tick: 5,
                move_dir: Vec2::new(1.0, 0.0),
                facing: 1.57,
                shoot: true,
            },
            ClientMsg::Ack { event_id: 42 },
            ClientMsg::Ping { nonce: 7 },
            ClientMsg::Leave,
        ]
    }

    // Exhaustive match, no wildcard arm: adding a `ClientMsg` variant
    // without updating this function is a compile error, which is the
    // nudge to also add a sample instance above (ARCHITECTURE.md §3.2).
    fn assert_client_msg_variant_covered(msg: &ClientMsg) {
        match msg {
            ClientMsg::Join { .. } => {}
            ClientMsg::Input { .. } => {}
            ClientMsg::Ack { .. } => {}
            ClientMsg::Ping { .. } => {}
            ClientMsg::Leave => {}
        }
    }

    fn sample_event_kinds() -> Vec<EventKind> {
        vec![
            EventKind::PlayerJoined {
                id: 1,
                name: "a".into(),
            },
            EventKind::PlayerLeft { id: 1 },
            EventKind::Hit {
                shooter: 1,
                target: 2,
                target_hp: 50,
            },
            EventKind::Killed {
                victim: 2,
                killer: 1,
            },
            EventKind::Respawned {
                id: 2,
                pos: Vec2::new(3.0, 4.0),
            },
            EventKind::LevelChanged {
                level: 2,
                level_epoch: 3,
                maze: sample_maze_source(),
            },
        ]
    }

    fn assert_event_kind_variant_covered(kind: &EventKind) {
        match kind {
            EventKind::PlayerJoined { .. } => {}
            EventKind::PlayerLeft { .. } => {}
            EventKind::Hit { .. } => {}
            EventKind::Killed { .. } => {}
            EventKind::Respawned { .. } => {}
            EventKind::LevelChanged { .. } => {}
        }
    }

    fn sample_server_msgs() -> Vec<ServerMsg> {
        vec![
            ServerMsg::Welcome {
                player_id: 1,
                level: 0,
                level_epoch: 0,
                maze: sample_maze_source(),
                tick_rate_hz: 30,
                server_tick: 0,
                server_time_ms: 0,
            },
            ServerMsg::Rejected {
                reason: "server full".into(),
            },
            ServerMsg::WorldState {
                tick: 100,
                level_epoch: 0,
                players: vec![PlayerSnapshot {
                    id: 1,
                    pos: Vec2::new(1.0, 2.0),
                    facing: 0.0,
                    hp: 100,
                    last_input_tick: 99,
                }],
            },
            ServerMsg::Event {
                event_id: 1,
                kind: EventKind::PlayerLeft { id: 1 },
            },
            ServerMsg::Pong {
                nonce: 9,
                server_tick: 5,
                server_time_ms: 1000,
            },
        ]
    }

    fn assert_server_msg_variant_covered(msg: &ServerMsg) {
        match msg {
            ServerMsg::Welcome { .. } => {}
            ServerMsg::Rejected { .. } => {}
            ServerMsg::WorldState { .. } => {}
            ServerMsg::Event { .. } => {}
            ServerMsg::Pong { .. } => {}
        }
    }

    #[test]
    fn client_msg_round_trips() {
        for msg in sample_client_msgs() {
            assert_client_msg_variant_covered(&msg);
            let bytes = encode(&msg).unwrap();
            let decoded: ClientMsg = decode(&bytes).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn server_msg_round_trips() {
        for msg in sample_server_msgs() {
            assert_server_msg_variant_covered(&msg);
            let bytes = encode(&msg).unwrap();
            let decoded: ServerMsg = decode(&bytes).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn event_kind_round_trips() {
        for kind in sample_event_kinds() {
            assert_event_kind_variant_covered(&kind);
            let msg = ServerMsg::Event {
                event_id: 1,
                kind: kind.clone(),
            };
            let bytes = encode(&msg).unwrap();
            let decoded: ServerMsg = decode(&bytes).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn every_variant_serializes_under_payload_budget() {
        // Welcome/LevelChanged historically carried the whole maze grid and
        // are the two variants most likely to blow the budget again if
        // someone reintroduces that (§3.2) — check every variant
        // explicitly, not just WorldState.
        for msg in sample_client_msgs() {
            let bytes = encode(&msg).unwrap();
            assert!(
                bytes.len() <= MAX_PAYLOAD_BYTES,
                "{msg:?} is {} bytes",
                bytes.len()
            );
        }
        for msg in sample_server_msgs() {
            let bytes = encode(&msg).unwrap();
            assert!(
                bytes.len() <= MAX_PAYLOAD_BYTES,
                "{msg:?} is {} bytes",
                bytes.len()
            );
        }

        // WorldState sized against MAX_PLAYERS (bots hold slots too, §4.2),
        // not against the audit's 10 — a budget test that only ever builds
        // a handful of players would exempt exactly the case that matters.
        let full_world_state = ServerMsg::WorldState {
            tick: 1,
            level_epoch: 0,
            players: (0..MAX_PLAYERS as u32)
                .map(|id| PlayerSnapshot {
                    id,
                    pos: Vec2::new(1.0, 2.0),
                    facing: 1.0,
                    hp: 100,
                    last_input_tick: 1,
                })
                .collect(),
        };
        let bytes = encode(&full_world_state).unwrap();
        assert!(
            bytes.len() <= MAX_PAYLOAD_BYTES,
            "WorldState with MAX_PLAYERS players is {} bytes, over budget",
            bytes.len()
        );
    }

    #[test]
    fn hostile_payload_is_rejected_without_large_allocation() {
        // Hand-built ClientMsg::Join whose `name` field declares a length
        // of u64::MAX. Byte 0 is the enum variant index (Join is variant
        // 0, and bincode's varint format writes any value <= 250 as a
        // single literal byte). Byte 1 is bincode's u64-varint tag (253),
        // followed by 8 little-endian bytes of the declared length. No
        // payload bytes follow it — this is a ~10-byte spoofed datagram
        // trying to make the receiver allocate exabytes. Byte layout
        // confirmed against bincode 2.0.1's own source
        // (varint/mod.rs, features/serde/ser.rs), not guessed.
        let mut malicious = vec![0x00u8, 253];
        malicious.extend_from_slice(&u64::MAX.to_le_bytes());

        let result: Result<ClientMsg, _> = decode(&malicious);
        assert!(
            result.is_err(),
            "expected a byte-limited decode to reject a huge declared length, got {result:?}"
        );
    }
}
