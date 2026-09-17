//! Server tick-loop and movement integration tests (ARCHITECTURE.md §8.3,
//! PLAN.md Milestone 7). Real UDP sockets over loopback.

mod support;

use std::io::Write;
use std::time::{Duration, Instant};

use common::config::{MAX_INPUT_QUEUE, PLAYER_SPEED_UPS, SERVER_TICK_HZ};
use common::protocol::{ClientMsg, ServerMsg};
use common::types::Vec2;
use support::{corridor_with_wall_east_of_spawn, open_room, spawn_server_with_maze, test_config, FakeClient};

/// Distance one `resolve_move` step covers in open space: speed * dt,
/// where dt is one `INPUT_DT_MS` step (§4.3 — one `Input` == one fixed
/// step, independent of tick timing).
fn step_distance() -> f32 {
    PLAYER_SPEED_UPS * (common::config::INPUT_DT_MS / 1000.0)
}

fn input(input_tick: u32, dir: Vec2) -> ClientMsg {
    ClientMsg::Input {
        input_tick,
        move_dir: dir,
        facing: 0.0,
        shoot: false,
    }
}

/// Reads messages until it finds the next `WorldState`, ignoring `Event`s
/// interleaved with it (join broadcasts, etc.) — panics via `FakeClient`'s
/// own timeout if none arrives in time.
fn next_world_state(client: &FakeClient) -> (u32, Vec<common::protocol::PlayerSnapshot>) {
    loop {
        if let ServerMsg::WorldState { tick, players, .. } = client.recv() {
            return (tick, players);
        }
    }
}

fn snapshot_of<'a>(
    players: &'a [common::protocol::PlayerSnapshot],
    id: u32,
) -> Option<&'a common::protocol::PlayerSnapshot> {
    players.iter().find(|p| p.id == id)
}

#[test]
fn movement_is_reflected_in_another_clients_world_state_within_a_couple_ticks() {
    let addr = spawn_server_with_maze(test_config(20, 5000), open_room(20, 20));
    let a = FakeClient::connect(addr);
    let b = FakeClient::connect_with_timeout(addr, Duration::from_millis(500));

    let ServerMsg::Welcome { player_id: a_id, .. } = a.join("a") else {
        panic!("expected Welcome for a")
    };
    assert!(matches!(b.join("b"), ServerMsg::Welcome { .. }));

    a.send(&input(1, Vec2::new(1.0, 0.0)));

    // Not instantly — respect the tick boundary: poll for a bounded
    // number of ticks rather than asserting on the very first WorldState.
    let spawn_x = 20.0f32 / 2.0 + 0.5;
    let deadline = Instant::now() + Duration::from_millis(300); // several ticks at 30Hz
    loop {
        let (_, players) = next_world_state(&b);
        if let Some(snap) = snapshot_of(&players, a_id) {
            if snap.pos.x > spawn_x + 1e-3 {
                return; // moved — success
            }
        }
        assert!(
            Instant::now() < deadline,
            "a's movement was never reflected in b's WorldState within the deadline"
        );
    }
}

#[test]
fn input_that_walks_into_a_wall_stops_at_the_wall() {
    let addr = spawn_server_with_maze(test_config(20, 5000), corridor_with_wall_east_of_spawn());
    let client = FakeClient::connect_with_timeout(addr, Duration::from_millis(500));
    let ServerMsg::Welcome { player_id, .. } = client.join("solo") else {
        panic!("expected Welcome")
    };

    // Generous burst: more than enough steps to reach the wall (~0.5 units
    // away) regardless of exactly how many land in the first tick vs.
    // later ones. Extra steps queued past the wall are harmless — once
    // blocked, resolve_move produces zero further displacement.
    for i in 0..30u32 {
        client.send(&input(i, Vec2::new(1.0, 0.0)));
    }

    let expected_wall_x = 3.0 - 0.25; // wall at x=3.0 (cell 2|3 boundary), radius 0.25
    let deadline = Instant::now() + Duration::from_millis(1000);
    loop {
        let (_, players) = next_world_state(&client);
        let snap = snapshot_of(&players, player_id).expect("player missing from WorldState");
        if (snap.pos.x - expected_wall_x).abs() < 0.05 {
            return; // stopped at the wall — success
        }
        assert!(
            snap.pos.x <= expected_wall_x + 0.05,
            "moved past the wall: x = {}",
            snap.pos.x
        );
        assert!(
            Instant::now() < deadline,
            "never reached the wall within the deadline (stuck at x = {})",
            snap.pos.x
        );
    }
}

#[test]
fn two_inputs_in_one_tick_window_advance_two_steps() {
    let addr = spawn_server_with_maze(test_config(20, 5000), open_room(20, 20));
    let client = FakeClient::connect_with_timeout(addr, Duration::from_millis(500));
    let ServerMsg::Welcome { player_id, .. } = client.join("solo") else {
        panic!("expected Welcome")
    };
    let spawn_x = 20.0f32 / 2.0 + 0.5;

    // Both sent well within a single ~33ms tick interval.
    client.send(&input(1, Vec2::new(1.0, 0.0)));
    client.send(&input(2, Vec2::new(1.0, 0.0)));

    // First WorldState showing any movement at all is the one that
    // consumed both — nothing else could have moved the player in
    // between, since these are the only inputs sent.
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        let (_, players) = next_world_state(&client);
        let snap = snapshot_of(&players, player_id).expect("player missing from WorldState");
        let delta = snap.pos.x - spawn_x;
        if delta > 1e-3 {
            let expected = 2.0 * step_distance();
            assert!(
                (delta - expected).abs() < 0.02,
                "expected ~2 steps ({expected}), got delta {delta}"
            );
            return;
        }
        assert!(Instant::now() < deadline, "player never moved at all");
    }
}

#[test]
fn flooding_input_caps_movement_at_max_input_queue_per_tick() {
    let addr = spawn_server_with_maze(test_config(20, 5000), open_room(20, 20));
    let client = FakeClient::connect_with_timeout(addr, Duration::from_millis(500));
    let ServerMsg::Welcome { player_id, .. } = client.join("solo") else {
        panic!("expected Welcome")
    };
    let spawn_x = 20.0f32 / 2.0 + 0.5;

    // MAX_INPUT_QUEUE + 5, all sent well within one tick window.
    let flood = MAX_INPUT_QUEUE as u32 + 5;
    for i in 1..=flood {
        client.send(&input(i, Vec2::new(1.0, 0.0)));
    }

    // The FIRST WorldState showing any movement reflects exactly one
    // tick's consumption — capped at MAX_INPUT_QUEUE, not the full flood.
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        let (_, players) = next_world_state(&client);
        let snap = snapshot_of(&players, player_id).expect("player missing from WorldState");
        let delta = snap.pos.x - spawn_x;
        if delta > 1e-3 {
            let capped = MAX_INPUT_QUEUE as f32 * step_distance();
            let uncapped = flood as f32 * step_distance();
            assert!(
                (delta - capped).abs() < 0.02,
                "expected exactly MAX_INPUT_QUEUE steps ({capped}) on the first tick, got {delta} \
                 (uncapped would have been {uncapped})"
            );
            return;
        }
        assert!(Instant::now() < deadline, "player never moved at all");
    }
}

#[test]
fn last_input_tick_echoes_the_highest_consumed_tick() {
    let addr = spawn_server_with_maze(test_config(20, 5000), open_room(20, 20));
    let client = FakeClient::connect_with_timeout(addr, Duration::from_millis(500));
    let ServerMsg::Welcome { player_id, .. } = client.join("solo") else {
        panic!("expected Welcome")
    };

    // 5 inputs, well under MAX_INPUT_QUEUE (6) — all consumed in one tick.
    for i in 1..=5u32 {
        client.send(&input(i, Vec2::new(1.0, 0.0)));
    }

    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        let (_, players) = next_world_state(&client);
        let snap = snapshot_of(&players, player_id).expect("player missing from WorldState");
        if snap.last_input_tick > 0 {
            assert_eq!(
                snap.last_input_tick, 5,
                "expected the highest *consumed* tick (5), not highest received or zero"
            );
            return;
        }
        assert!(Instant::now() < deadline, "last_input_tick never advanced");
    }
}

#[test]
fn tick_rate_holds_under_load_with_ten_clients() {
    let addr = spawn_server_with_maze(test_config(20, 5000), open_room(30, 30));

    let n_clients = 10;
    let mut clients = Vec::new();
    for i in 0..n_clients {
        let c = FakeClient::connect_with_timeout(addr, Duration::from_millis(50));
        let ServerMsg::Welcome { .. } = c.join(&format!("bot{i}")) else {
            panic!("expected Welcome for bot{i}")
        };
        clients.push(c);
    }

    // 10s is enough to catch a real regression in CI; the full 3-minute
    // run is the pre-submission manual soak test (ARCHITECTURE.md §8.5),
    // not this one.
    let test_duration = Duration::from_secs(10);
    let start = Instant::now();
    let mut input_tick = 0u32;
    let observer = &clients[0];
    let mut first_tick: Option<(u32, Instant)> = None;
    let mut last_tick: Option<(u32, Instant)> = None;

    while start.elapsed() < test_duration {
        input_tick += 1;
        for c in &clients {
            c.send(&input(input_tick, Vec2::new(1.0, 0.0)));
        }
        if let Some(ServerMsg::WorldState { tick, .. }) = observer.try_recv() {
            let now = Instant::now();
            if first_tick.is_none() {
                first_tick = Some((tick, now));
            }
            last_tick = Some((tick, now));
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let (t0, i0) = first_tick.expect("never received any WorldState during the load test");
    let (t1, i1) = last_tick.unwrap();
    let elapsed_s = (i1 - i0).as_secs_f64();
    let ticks_elapsed = (t1 - t0) as f64;
    let achieved_hz = if elapsed_s > 0.0 {
        ticks_elapsed / elapsed_s
    } else {
        0.0
    };
    let target_hz = SERVER_TICK_HZ as f64;

    log_tick_rate_result(n_clients, target_hz, achieved_hz, ticks_elapsed as u32, elapsed_s);

    // Generous tolerance for a CI runner under arbitrary load; the point
    // is catching a real regression (e.g. an O(n^2) broadcast, a lock
    // contention bug), not chasing single-digit-percent jitter.
    assert!(
        achieved_hz > target_hz * 0.8,
        "achieved tick rate {achieved_hz:.2}Hz is too far below target {target_hz}Hz with \
         {n_clients} clients"
    );
}

/// Persists the load-test result so it's not a one-time terminal glance
/// (PLAN.md Milestone 7 gate's own instruction) — appended to a log file
/// under the workspace's `target/` (gitignored, but present locally across
/// runs) in addition to the usual captured test output.
fn log_tick_rate_result(n_clients: usize, target_hz: f64, achieved_hz: f64, ticks: u32, secs: f64) {
    let line = format!(
        "{} clients={n_clients} target_hz={target_hz} achieved_hz={achieved_hz:.2} ticks={ticks} secs={secs:.2}\n",
        chrono_like_timestamp(),
    );
    eprint!("{line}"); // visible in CI's captured-on-failure output too

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("tick_rate_log.txt");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// A timestamp without pulling in a chrono dependency for one log line.
fn chrono_like_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
