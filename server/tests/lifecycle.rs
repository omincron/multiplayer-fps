//! Server integration tests (ARCHITECTURE.md §8.3, PLAN.md Milestone 6).
//! Real UDP sockets over loopback — no mocking the network, per §8.3's own
//! instruction that these drive the server with real `UdpSocket`s standing
//! in for clients.

mod support;

use std::time::Duration;

use common::config::{GENERATOR_VERSION, PROTOCOL_VERSION};
use common::maze::MazeSource;
use common::protocol::{ClientMsg, EventKind, ServerMsg};
use support::{spawn_server, test_config, FakeClient};

#[test]
fn single_client_connects_and_receives_welcome() {
    let addr = spawn_server(test_config(20, 5000));
    let client = FakeClient::connect(addr);

    match client.join("alice") {
        ServerMsg::Welcome {
            player_id, maze, ..
        } => {
            assert!(player_id > 0);
            match maze {
                MazeSource::Generated(spec) => {
                    assert!(spec.width > 0 && spec.height > 0);
                }
                MazeSource::Custom(_) => panic!("expected a generated maze, got a custom one"),
            }
        }
        other => panic!("expected Welcome, got {other:?}"),
    }
}

#[test]
fn capacity_rejects_beyond_max_and_existing_clients_unaffected() {
    let addr = spawn_server(test_config(2, 5000));
    let a = FakeClient::connect(addr);
    let b = FakeClient::connect(addr);
    let c = FakeClient::connect(addr);

    assert!(matches!(a.join("a"), ServerMsg::Welcome { .. }));
    assert!(matches!(b.join("b"), ServerMsg::Welcome { .. }));

    match c.join("c") {
        ServerMsg::Rejected { reason } => assert_eq!(reason, "server full"),
        other => panic!("expected Rejected(\"server full\"), got {other:?}"),
    }

    // The already-connected clients must still be responsive, not silently
    // affected by the capacity check that rejected `c`. `a` may also have
    // a `PlayerJoined` event about `b` (or now a `WorldState`) queued
    // ahead of the Pong (a legitimate race, not a bug), so skip past
    // anything that isn't it.
    a.send(&ClientMsg::Ping { nonce: 42 });
    loop {
        match a.recv() {
            ServerMsg::Pong { nonce, .. } => {
                assert_eq!(nonce, 42);
                break;
            }
            ServerMsg::Event { .. } | ServerMsg::WorldState { .. } => continue,
            other => panic!("expected Pong, got {other:?}"),
        }
    }
}

#[test]
fn duplicate_name_is_rejected() {
    let addr = spawn_server(test_config(20, 5000));
    let a = FakeClient::connect(addr);
    let b = FakeClient::connect(addr);

    assert!(matches!(a.join("dupe"), ServerMsg::Welcome { .. }));
    match b.join("dupe") {
        ServerMsg::Rejected { reason } => assert_eq!(reason, "name already in use"),
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn idle_timeout_broadcasts_player_left_to_a_still_connected_client() {
    // Short timeout so the test doesn't wait a real CLIENT_TIMEOUT_MS
    // (5000ms) — this is exactly why client_timeout_ms is a Config field
    // and not a bare constant (§9.1).
    let addr = spawn_server(test_config(20, 150));

    let a = FakeClient::connect(addr);
    let b = support::FakeClient::connect_with_timeout(addr, Duration::from_millis(20));

    let ServerMsg::Welcome { player_id: a_id, .. } = a.join("a") else {
        panic!("expected Welcome for a")
    };
    assert!(matches!(b.join("b"), ServerMsg::Welcome { .. }));

    // `a` goes silent from here. `b` keeps pinging so it doesn't also time
    // out, and polls for the PlayerLeft event between pings.
    let deadline = std::time::Instant::now() + Duration::from_millis(1000);
    loop {
        b.send(&ClientMsg::Ping { nonce: 1 });
        if let Some(ServerMsg::Event {
            kind: EventKind::PlayerLeft { id },
            ..
        }) = b.try_recv()
        {
            assert_eq!(id, a_id, "PlayerLeft fired for the wrong player");
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "never received PlayerLeft for the timed-out player within {:?}",
                deadline
            );
        }
    }
}

#[test]
fn protocol_version_mismatch_is_rejected() {
    let addr = spawn_server(test_config(20, 5000));
    let client = FakeClient::connect(addr);
    client.send(&ClientMsg::Join {
        name: "x".into(),
        protocol_version: PROTOCOL_VERSION + 1,
        generator_version: GENERATOR_VERSION,
    });
    match client.recv() {
        ServerMsg::Rejected { reason } => assert_eq!(reason, "protocol version mismatch"),
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn generator_version_mismatch_is_rejected() {
    let addr = spawn_server(test_config(20, 5000));
    let client = FakeClient::connect(addr);
    client.send(&ClientMsg::Join {
        name: "x".into(),
        protocol_version: PROTOCOL_VERSION,
        generator_version: GENERATOR_VERSION + 1,
    });
    match client.recv() {
        ServerMsg::Rejected { reason } => assert_eq!(reason, "generator version mismatch"),
        other => panic!("expected Rejected, got {other:?}"),
    }
}
