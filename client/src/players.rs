use common::config::GHOST_TIMEOUT_MS;
use common::protocol::{EventKind, PlayerSnapshot};
use common::types::{PlayerId, Vec2};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct RemotePlayer {
    pub id: PlayerId,
    pub name: String,
    pub pos: Vec2,
    pub facing: f32,
    pub hp: u8,
    last_seen_ms: u64,
}

#[derive(Debug, Default)]
pub struct RemotePlayers {
    players: HashMap<PlayerId, RemotePlayer>,
    known_names: HashMap<PlayerId, String>,
}

impl RemotePlayers {
    pub fn apply_snapshot(&mut self, self_id: PlayerId, snapshots: &[PlayerSnapshot], now_ms: u64) {
        for snapshot in snapshots.iter().filter(|player| player.id != self_id) {
            let fallback_name = || format!("Player {}", snapshot.id);
            let name = self
                .known_names
                .get(&snapshot.id)
                .cloned()
                .unwrap_or_else(fallback_name);
            let player = self.players.entry(snapshot.id).or_insert(RemotePlayer {
                id: snapshot.id,
                name,
                pos: snapshot.pos,
                facing: snapshot.facing,
                hp: snapshot.hp,
                last_seen_ms: now_ms,
            });
            player.pos = snapshot.pos;
            player.facing = snapshot.facing;
            player.hp = snapshot.hp;
            player.last_seen_ms = now_ms;
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
            EventKind::Hit { .. }
            | EventKind::Killed { .. }
            | EventKind::Respawned { .. }
            | EventKind::LevelChanged { .. } => {}
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
        players.apply_snapshot(1, &[snapshot(1, 1.0), snapshot(9, 3.0)], 100);

        let remote = players.get(9).unwrap();
        assert_eq!(remote.name, "Player 9");
        assert_eq!(remote.pos, Vec2::new(3.0, 2.0));
        assert!(players.get(1).is_none(), "self is not a remote player");
    }

    #[test]
    fn late_join_event_replaces_placeholder_name() {
        let mut players = RemotePlayers::default();
        players.apply_snapshot(1, &[snapshot(9, 3.0)], 100);

        players.apply_event(&EventKind::PlayerJoined {
            id: 9,
            name: "bob".into(),
        });

        assert_eq!(players.get(9).unwrap().name, "bob");
    }

    #[test]
    fn absent_player_is_removed_after_ghost_timeout_without_leave_event() {
        let mut players = RemotePlayers::default();
        players.apply_snapshot(1, &[snapshot(9, 3.0)], 100);

        players.apply_snapshot(1, &[], 100 + GHOST_TIMEOUT_MS);
        assert!(players.get(9).is_some());
        players.apply_snapshot(1, &[], 101 + GHOST_TIMEOUT_MS);
        assert!(players.get(9).is_none());
    }
}
