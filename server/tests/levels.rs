//! Level progression integration tests (ARCHITECTURE.md §4.4, PLAN.md
//! Milestone 14). Real UDP sockets over loopback, same pattern as the
//! other `server/tests/*.rs` files.

mod support;

use common::maze::MazeSource;
use common::protocol::{ClientMsg, EventKind, ServerMsg};
use server::levels::LEVELS;
use support::{open_room, spawn_server_with_maze, test_config, FakeClient};

/// Reads messages until it finds the next `LevelChanged` event, acking
/// every event it sees along the way (same reasoning as the shooting
/// test: an unacked event gets retried by the reliability layer and a
/// retry could otherwise be misread as the next expected transition).
fn next_level_changed(client: &FakeClient) -> (u8, u32, MazeSource) {
    loop {
        if let ServerMsg::Event { event_id, kind } = client.recv() {
            client.send(&ClientMsg::Ack { event_id });
            if let EventKind::LevelChanged {
                level,
                level_epoch,
                maze,
            } = kind
            {
                return (level, level_epoch, maze);
            }
        }
    }
}

fn next_world_state(
    client: &FakeClient,
) -> (u32, Vec<common::protocol::PlayerSnapshot>) {
    loop {
        if let ServerMsg::WorldState {
            level_epoch,
            players,
            ..
        } = client.recv()
        {
            return (level_epoch, players);
        }
    }
}

#[test]
fn server_progresses_through_levels_on_the_configured_timer() {
    let config = server_test_config();
    let addr = spawn_server_with_maze(config, open_room(6, 6));
    let client = FakeClient::connect(addr);

    let ServerMsg::Welcome {
        level: initial_level,
        level_epoch: initial_epoch,
        player_id,
        ..
    } = client.join("player")
    else {
        panic!("expected Welcome")
    };
    assert_eq!(initial_level, 0);
    assert_eq!(initial_epoch, 0);

    // Two transitions: 0 -> 1 -> 2, both strictly harder (larger area,
    // lower braid_factor) — exercises the real table, not a wraparound.
    for expected_level in 1u8..=2 {
        let (level, level_epoch, maze) = next_level_changed(&client);
        assert_eq!(level, expected_level);
        assert_eq!(level_epoch, expected_level as u32);

        let MazeSource::Generated(spec) = maze else {
            panic!("expected a Generated maze source")
        };
        let expected = &LEVELS[expected_level as usize];
        assert_eq!(spec.width, expected.grid_w);
        assert_eq!(spec.height, expected.grid_h);
        assert_eq!(spec.braid_factor, expected.braid_factor);
        // "Genuinely different (larger/harder)", not just a different
        // seed of the same shape (PLAN.md's own wording) — check against
        // the *previous* level's area explicitly, not only against the
        // table (which could itself be wrong in a way this wouldn't catch
        // if compared only to itself).
        let previous = &LEVELS[expected_level as usize - 1];
        assert!(
            (expected.grid_w as u32 * expected.grid_h as u32)
                > (previous.grid_w as u32 * previous.grid_h as u32)
        );

        // The very next WorldState must carry the new epoch and place the
        // player back at a valid, in-bounds spawn point for the *new*
        // maze — not wherever they happened to be standing in the old one
        // (§4.4: "resets player positions to valid spawn cells"; this
        // matters for real once the table wraps a large maze back down to
        // a small one, but must hold on every transition).
        let (world_epoch, players) = next_world_state(&client);
        assert_eq!(world_epoch, level_epoch);
        let me = players
            .iter()
            .find(|p| p.id == player_id)
            .expect("self should be present in WorldState");
        assert!((0.0..expected.grid_w as f32).contains(&me.pos.x));
        assert!((0.0..expected.grid_h as f32).contains(&me.pos.y));
    }
}

fn server_test_config() -> common::config::Config {
    common::config::Config {
        level_duration_ms: 150,
        ..test_config(5, 5000)
    }
}
