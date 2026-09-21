//! Server-side shooting/health/respawn integration tests (ARCHITECTURE.md
//! §4.3, PLAN.md Milestone 13). Real UDP sockets over loopback, same
//! pattern as `tick_loop.rs`.

mod support;

use common::config::{HIT_DAMAGE, INPUT_DT_MS, MAX_HP, PLAYER_SPEED_UPS};
use common::protocol::{ClientMsg, EventKind, ServerMsg};
use common::types::Vec2;
use support::{open_room, spawn_server_with_maze, test_config, FakeClient};

const ROOM: u16 = 20;
/// Distance one `resolve_move` step covers in open space — same derivation
/// as `tick_loop.rs`'s `step_distance`, duplicated locally per this test
/// suite's own file (each `tests/*.rs` file is its own binary, no shared
/// non-`support` helpers across them).
fn step_distance() -> f32 {
    PLAYER_SPEED_UPS * (INPUT_DT_MS / 1000.0)
}

fn move_east(input_tick: u32) -> ClientMsg {
    ClientMsg::Input {
        input_tick,
        move_dir: Vec2::new(1.0, 0.0),
        facing: 0.0,
        shoot: false,
    }
}

fn shoot_east(input_tick: u32) -> ClientMsg {
    ClientMsg::Input {
        input_tick,
        move_dir: Vec2::ZERO,
        facing: 0.0, // FACING_EAST, matching common::sim's own test convention
        shoot: true,
    }
}

/// Reads messages until it finds the next `WorldState`, ignoring `Event`s —
/// same pattern as `tick_loop.rs`.
fn next_world_state(client: &FakeClient) -> Vec<common::protocol::PlayerSnapshot> {
    loop {
        if let ServerMsg::WorldState { players, .. } = client.recv() {
            return players;
        }
    }
}

/// Reads messages until it finds the next `Event`, ignoring `WorldState`,
/// and immediately acks it. Acking matters here specifically: this test
/// sends several shots in a row without a real client's usual ack-as-you-go
/// behavior, and an unacked `Hit` gets retried every `RETRY_INTERVAL_MS` —
/// a retry of shot 1's `Hit` arriving while the test is waiting for shot
/// 2's `Hit` would otherwise be misread as the wrong event.
fn next_event(client: &FakeClient) -> EventKind {
    loop {
        if let ServerMsg::Event { event_id, kind } = client.recv() {
            client.send(&ClientMsg::Ack { event_id });
            return kind;
        }
    }
}

fn snapshot_of(
    players: &[common::protocol::PlayerSnapshot],
    id: u32,
) -> Option<&common::protocol::PlayerSnapshot> {
    players.iter().find(|p| p.id == id)
}

#[test]
fn shooting_aimed_target_deals_damage_then_kills_and_respawns_them() {
    let addr = spawn_server_with_maze(test_config(5, 5000), open_room(ROOM, ROOM));
    let shooter = FakeClient::connect(addr);
    let target = FakeClient::connect(addr);

    let ServerMsg::Welcome {
        player_id: shooter_id,
        ..
    } = shooter.join("shooter")
    else {
        panic!("expected Welcome for shooter")
    };
    let ServerMsg::Welcome {
        player_id: target_id,
        ..
    } = target.join("target")
    else {
        panic!("expected Welcome for target")
    };

    // Move the target several cells east so there's real distance between
    // the two, then confirm it actually moved before trusting the shot
    // geometry below — a stub that never applies movement would otherwise
    // let this test "pass" by having shooter and target coincide.
    let desired_offset = step_distance() * 20.0; // ~2.0 units
    for tick in 1..=20u32 {
        target.send(&move_east(tick));
    }
    let (origin_x, target_x) = loop {
        let players = next_world_state(&shooter);
        let Some(t) = snapshot_of(&players, target_id) else {
            continue;
        };
        let Some(s) = snapshot_of(&players, shooter_id) else {
            continue;
        };
        if t.pos.x - s.pos.x >= desired_offset - 0.01 {
            break (s.pos.x, t.pos.x);
        }
    };
    assert!(
        target_x > origin_x + 0.5,
        "target did not actually move away from the shooter (shooter x={origin_x}, target x={target_x})"
    );

    // Enough aimed shots to go from MAX_HP to exactly 0 — confirms both the
    // per-hit damage amount and that death triggers at (not before/after) 0.
    let hits_to_kill = MAX_HP.div_ceil(HIT_DAMAGE);
    let mut expected_hp = MAX_HP;
    for shot in 0..hits_to_kill {
        shooter.send(&shoot_east(100 + shot as u32));
        let event = next_event(&shooter);
        let EventKind::Hit {
            shooter: hit_shooter,
            target: hit_target,
            target_hp,
        } = event
        else {
            panic!("expected a Hit event, got {event:?}")
        };
        assert_eq!(hit_shooter, shooter_id);
        assert_eq!(hit_target, target_id);
        expected_hp = expected_hp.saturating_sub(HIT_DAMAGE);
        assert_eq!(
            target_hp, expected_hp,
            "Hit event's target_hp didn't match the expected running total"
        );
    }
    assert_eq!(expected_hp, 0, "test setup should drive hp to exactly 0");

    let killed = next_event(&shooter);
    let EventKind::Killed { victim, killer } = killed else {
        panic!("expected a Killed event once hp reached 0, got {killed:?}")
    };
    assert_eq!(victim, target_id);
    assert_eq!(killer, shooter_id);

    let respawned = next_event(&shooter);
    let EventKind::Respawned { id, pos } = respawned else {
        panic!("expected a Respawned event right after Killed, got {respawned:?}")
    };
    assert_eq!(id, target_id);
    // "some position came back" is not the gate (PLAN.md Milestone 13) —
    // check it against the actual maze bounds. Every in-bounds cell in a
    // §5.3-valid maze is floor space (walls live on cell edges, not inside
    // cells), so in-bounds is the whole check.
    assert!(
        (0.0..ROOM as f32).contains(&pos.x) && (0.0..ROOM as f32).contains(&pos.y),
        "respawn position {pos:?} is outside the {ROOM}x{ROOM} maze"
    );

    // The respawn must also be reflected in the next authoritative
    // WorldState, at full hp — the Event stream and WorldState must agree.
    loop {
        let players = next_world_state(&shooter);
        let Some(t) = snapshot_of(&players, target_id) else {
            continue;
        };
        if t.hp == MAX_HP {
            assert_eq!(t.pos, pos, "WorldState position disagrees with the Respawned event");
            break;
        }
    }
}
