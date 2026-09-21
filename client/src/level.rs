//! Tracks the client's current maze/level state and decides whether an
//! incoming `WorldState` belongs to it (Milestone 14, ARCHITECTURE.md
//! §4.4). A mismatch is not a rare edge case worth skipping: `LevelChanged`
//! rides the reliable channel and is retried on a 100ms timer while
//! `WorldState` arrives roughly every 33ms, so one dropped `LevelChanged`
//! datagram guarantees several snapshots for a maze the client doesn't
//! have yet arrive first — rendering them puts players inside walls from
//! geometry that no longer exists.

use common::maze::{MazeData, MazeSource};
use common::types::Vec2;

use crate::maze::{MazeBuildError, build_and_validate};

pub struct LevelState {
    level: u8,
    level_epoch: u32,
    maze: MazeData,
}

impl LevelState {
    pub fn new(level: u8, level_epoch: u32, maze: MazeData) -> Self {
        Self {
            level,
            level_epoch,
            maze,
        }
    }

    pub fn maze(&self) -> &MazeData {
        &self.maze
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn level_epoch(&self) -> u32 {
        self.level_epoch
    }

    /// Whether a `WorldState` stamped with `incoming_epoch` belongs to the
    /// maze this client currently holds. §4.4: on a mismatch the caller
    /// must discard the snapshot, never render it.
    pub fn accepts(&self, incoming_epoch: u32) -> bool {
        incoming_epoch == self.level_epoch
    }

    /// Applies a `LevelChanged` event: rebuilds and validates the new
    /// maze *first*, and only on success adopts it as current — on
    /// failure the client keeps whatever (still valid) maze/epoch it had,
    /// rather than adopting a half-broken new one. The caller decides what
    /// to do with an `Err` (log it; the event will simply be retried by
    /// the reliability layer like any other unacked event).
    pub fn apply_level_changed(
        &mut self,
        level: u8,
        level_epoch: u32,
        source: MazeSource,
    ) -> Result<(), MazeBuildError> {
        let maze = build_and_validate(source)?;
        self.level = level;
        self.level_epoch = level_epoch;
        self.maze = maze;
        Ok(())
    }

    /// The new maze's center cell — where prediction and, authoritatively,
    /// the server both place every player right after a level change
    /// (`server::world`'s `grid_center_spawn`, same formula).
    pub fn spawn_point(&self) -> Vec2 {
        grid_center(&self.maze)
    }
}

pub fn grid_center(maze: &MazeData) -> Vec2 {
    Vec2::new(
        (maze.grid.width / 2) as f32 + 0.5,
        (maze.grid.height / 2) as f32 + 0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::config::GENERATOR_VERSION;
    use common::maze::MazeSpec;

    fn generated(seed: u64, size: u16) -> MazeSource {
        MazeSource::Generated(MazeSpec {
            seed,
            width: size,
            height: size,
            braid_factor: 0.3,
            generator_version: GENERATOR_VERSION,
        })
    }

    fn initial_state() -> LevelState {
        let maze = build_and_validate(generated(1, 10)).unwrap();
        LevelState::new(0, 5, maze)
    }

    #[test]
    fn accepts_only_the_current_epoch() {
        let state = initial_state();
        assert!(state.accepts(5));
        assert!(!state.accepts(4), "a stale epoch must not be accepted");
        assert!(!state.accepts(6), "an epoch the client hasn't adopted yet must not be accepted");
    }

    #[test]
    fn applying_a_level_change_adopts_the_new_maze_and_epoch() {
        let mut state = initial_state();

        state
            .apply_level_changed(1, 6, generated(2, 20))
            .expect("valid maze should apply");

        assert_eq!(state.level(), 1);
        assert_eq!(state.level_epoch(), 6);
        assert!(state.accepts(6));
        assert!(!state.accepts(5), "the old epoch must be rejected once a new one is adopted");
        assert_eq!(state.maze().grid.width, 20);
    }

    #[test]
    fn a_failed_level_change_leaves_the_old_maze_and_epoch_in_place() {
        let mut state = initial_state();
        // generator_version mismatch is the cheapest way to force
        // `build_and_validate` to fail without hand-building a broken grid.
        let bad_source = MazeSource::Generated(common::maze::MazeSpec {
            seed: 99,
            width: 20,
            height: 20,
            braid_factor: 0.3,
            generator_version: GENERATOR_VERSION + 1,
        });

        let result = state.apply_level_changed(1, 6, bad_source);

        assert!(result.is_err());
        assert_eq!(state.level(), 0);
        assert_eq!(state.level_epoch(), 5);
        assert!(state.accepts(5), "old epoch must still be accepted after a failed apply");
        assert_eq!(state.maze().grid.width, 10);
    }

    #[test]
    fn spawn_point_is_the_current_mazes_center_cell() {
        let state = initial_state();
        assert_eq!(state.spawn_point(), Vec2::new(5.5, 5.5));
    }
}
