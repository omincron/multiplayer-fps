use common::config::GHOST_TIMEOUT_MS;
use common::protocol::{EventKind, PlayerSnapshot};
use common::types::{PlayerId, Vec2};
use std::collections::HashMap;

use crate::interp::{SnapshotBuffer, TimedSnapshot};

#[derive(Debug)]
pub struct RemotePlayer {
    pub id: PlayerId,
    pub name: String,
    pub pos: Vec2,
    pub facing: f32,
    pub hp: u8,
    last_seen_ms: u64,
    snapshots: SnapshotBuffer,
}

impl RemotePlayer {
    pub fn render_state(&self, target_server_time_ms: f64) -> (Vec2, f32) {
        self.snapshots
            .sample(target_server_time_ms)
            .map(|sample| (sample.pos, sample.facing))
            .unwrap_or((self.pos, self.facing))
    }
}

#[derive(Debug, Default)]
pub struct RemotePlayers {
    players: HashMap<PlayerId, RemotePlayer>,
    known_names: HashMap<PlayerId, String>,
}

impl RemotePlayers {
    pub fn apply_snapshot(
        &mut self,
        self_id: PlayerId,
        snapshots: &[PlayerSnapshot],
        snapshot_server_time_ms: f64,
        now_ms: u64,
    ) {
        for snapshot in snapshots.iter().filter(|player| player.id != self_id) {
            let fallback_name = || format!("Player {}", snapshot.id);
            let name = self
                .known_names
                .get(&snapshot.id)
                .cloned()
                .unwrap_or_else(fallback_name);
            let player = self
                .players
                .entry(snapshot.id)
                .or_insert_with(|| RemotePlayer {
                    id: snapshot.id,
                    name,
                    pos: snapshot.pos,
                    facing: snapshot.facing,
                    hp: snapshot.hp,
                    last_seen_ms: now_ms,
                    snapshots: SnapshotBuffer::default(),
                });
            player.pos = snapshot.pos;
            player.facing = snapshot.facing;
            player.hp = snapshot.hp;
            player.last_seen_ms = now_ms;
            player.snapshots.push(TimedSnapshot {
                server_time_ms: snapshot_server_time_ms,
                pos: snapshot.pos,
                facing: snapshot.facing,
            });
        }

        self.players
            .retain(|_, player| now_ms.saturating_sub(player.last_seen_ms) <= GHOST_TIMEOUT_MS);
    }

    pub fn apply_event(&mut self, event: &EventKind) {
        match event {
            EventKind::PlayerJoined { id, name } => {
                self.known_names.insert(*id, name.clone());
                if let Some(player) = self.players.get_mut(id) {
                    player.name.clone_from(name);
                }
            }
            EventKind::PlayerLeft { id } => {
                self.players.remove(id);
                self.known_names.remove(id);
            }
            EventKind::Respawned { id, pos } => {
                // Self-respawn is handled separately by the caller
                // (`Predictor::respawn_to`) — this player, if present here
                // at all, is a remote one.
                if let Some(player) = self.players.get_mut(id) {
                    player.pos = *pos;
                    player.snapshots.clear();
                }
            }
            EventKind::Hit { .. } | EventKind::Killed { .. } | EventKind::LevelChanged { .. } => {}
        }
    }

    pub fn get(&self, id: PlayerId) -> Option<&RemotePlayer> {
        self.players.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &RemotePlayer> {
        self.players.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::config::GHOST_TIMEOUT_MS;
    use common::protocol::{EventKind, PlayerSnapshot};
    use common::types::Vec2;

    fn snapshot(id: u32, x: f32) -> PlayerSnapshot {
        PlayerSnapshot {
            id,
            pos: Vec2::new(x, 2.0),
            facing: 0.5,
            hp: 100,
            last_input_tick: 4,
        }
    }

    #[test]
    fn unknown_snapshot_player_is_created_with_placeholder_name() {
        let mut players = RemotePlayers::default();
        players.apply_snapshot(1, &[snapshot(1, 1.0), snapshot(9, 3.0)], 100.0, 100);

        let remote = players.get(9).unwrap();
        assert_eq!(remote.name, "Player 9");
        assert_eq!(remote.pos, Vec2::new(3.0, 2.0));
        assert!(players.get(1).is_none(), "self is not a remote player");
    }

    #[test]
    fn late_join_event_replaces_placeholder_name() {
        let mut players = RemotePlayers::default();
        players.apply_snapshot(1, &[snapshot(9, 3.0)], 100.0, 100);

        players.apply_event(&EventKind::PlayerJoined {
            id: 9,
            name: "bob".into(),
        });

        assert_eq!(players.get(9).unwrap().name, "bob");
    }

    #[test]
    fn absent_player_is_removed_after_ghost_timeout_without_leave_event() {
        let mut players = RemotePlayers::default();
        players.apply_snapshot(1, &[snapshot(9, 3.0)], 100.0, 100);

        players.apply_snapshot(1, &[], 200.0, 100 + GHOST_TIMEOUT_MS);
        assert!(players.get(9).is_some());
        players.apply_snapshot(1, &[], 300.0, 101 + GHOST_TIMEOUT_MS);
        assert!(players.get(9).is_none());
    }

    #[test]
    fn respawned_event_snaps_a_remote_player_to_the_new_position_instead_of_sliding() {
        let mut players = RemotePlayers::default();
        // Two snapshots far from the eventual respawn point, so the
        // interpolation buffer has real (stale) history to discard.
        players.apply_snapshot(1, &[snapshot(9, 3.0)], 100.0, 100);
        players.apply_snapshot(1, &[snapshot(9, 3.5)], 133.0, 100);

        players.apply_event(&EventKind::Respawned {
            id: 9,
            pos: Vec2::new(40.0, 40.0),
        });

        let remote = players.get(9).unwrap();
        assert_eq!(remote.pos, Vec2::new(40.0, 40.0));
        // A query at a time between the old snapshots would previously have
        // interpolated to somewhere around x=3.0-3.5 — asserting the render
        // state directly is what would catch a fix that only updates `pos`
        // but forgets to also clear the stale buffer.
        let (rendered_pos, _) = remote.render_state(115.0);
        assert_eq!(rendered_pos, Vec2::new(40.0, 40.0));
    }
}
