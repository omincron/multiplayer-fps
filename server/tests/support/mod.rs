//! Shared test helpers for `server/tests/*.rs` (ARCHITECTURE.md §8.3: real
//! loopback `UdpSocket`s standing in for clients, no mocking the network).
//! Not auto-discovered as its own test binary — lives under `tests/support/`
//! rather than directly in `tests/`, and each test file opts in with
//! `mod support;`.
//!
//! `#![allow(dead_code)]`: each `tests/*.rs` file is compiled as its own
//! separate binary and pulls in this whole module, so a helper only
//! `lifecycle.rs` uses looks unused from `tick_loop.rs`'s compilation and
//! vice versa — a per-binary false positive, not the "genuinely unused,
//! should stay a visible warning" case Milestone 2's no-`#[allow]` rule
//! was about (that was about a real gap in the protocol's variant
//! coverage; this is a known limitation of how `tests/` shares code).

#![allow(dead_code)]

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use common::config::{Config, GENERATOR_VERSION, MAX_PAYLOAD_BYTES, PROTOCOL_VERSION};
use common::maze::{MazeData, MazeGrid, MazeSource, MazeSpec, WALL_E, WALL_N, WALL_S, WALL_W};
use common::protocol::{self, ClientMsg, ServerMsg};

pub const DEFAULT_TEST_TIMEOUT: Duration = Duration::from_millis(300);

/// `127.0.0.1:0` (OS-assigned port), never `0.0.0.0:PORT` — production
/// binds the latter (§1.6), tests always the former (§8.3).
pub fn test_config(max_players: usize, client_timeout_ms: u64) -> Config {
    Config {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_players,
        client_timeout_ms,
        ..Config::default()
    }
}

pub fn spawn_server(config: Config) -> SocketAddr {
    server::net::spawn(config).expect("server failed to bind")
}

pub fn spawn_server_with_maze(config: Config, maze: MazeData) -> SocketAddr {
    server::net::spawn_with_maze(config, maze).expect("server failed to bind")
}

/// All walls closed everywhere; `open()` opens specific edges from this.
/// Same pattern as `common::sim`'s own test helpers (Milestone 4) —
/// duplicated here rather than shared, since those are private to
/// `common`'s own test module and this crate can't reach them.
pub fn walled_grid(width: u16, height: u16) -> MazeGrid {
    MazeGrid {
        width,
        height,
        walls: vec![WALL_N | WALL_E | WALL_S | WALL_W; width as usize * height as usize],
    }
}

pub fn open(grid: &mut MazeGrid, x1: u16, y1: u16, x2: u16, y2: u16) {
    let i1 = grid.index(x1, y1);
    let i2 = grid.index(x2, y2);
    if x2 == x1 + 1 && y2 == y1 {
        grid.walls[i1] &= !WALL_E;
        grid.walls[i2] &= !WALL_W;
    } else if x1 == x2 + 1 && y2 == y1 {
        grid.walls[i1] &= !WALL_W;
        grid.walls[i2] &= !WALL_E;
    } else if y2 == y1 + 1 && x2 == x1 {
        grid.walls[i1] &= !WALL_S;
        grid.walls[i2] &= !WALL_N;
    } else if y1 == y2 + 1 && x2 == x1 {
        grid.walls[i1] &= !WALL_N;
        grid.walls[i2] &= !WALL_S;
    } else {
        panic!("open() called on non-adjacent cells");
    }
}

/// Wraps a hand-built grid as `MazeData` for `spawn_server_with_maze`.
/// The embedded `MazeSpec` is metadata only (`seed: 0`) — regenerating
/// from it would NOT reproduce this hand-built grid, since determinism
/// only holds for grids that actually came out of `maze::generate`. Fine
/// for these tests: they only ever inspect `PlayerSnapshot` positions over
/// the wire, never ask a client to regenerate this maze.
pub fn maze_data_from_grid(grid: MazeGrid) -> MazeData {
    MazeData {
        source: MazeSource::Generated(MazeSpec {
            seed: 0,
            width: grid.width,
            height: grid.height,
            braid_factor: 0.0,
            generator_version: GENERATOR_VERSION,
        }),
        generator_name: "test-fixture (not reproducible from its embedded spec)".to_string(),
        grid,
    }
}

/// A fully open `width` x `height` room — every internal wall cleared.
/// `World`'s spawn point is the grid center, so with this maze any
/// movement direction from spawn is guaranteed unobstructed.
pub fn open_room(width: u16, height: u16) -> MazeData {
    let mut grid = walled_grid(width, height);
    for y in 0..height {
        for x in 0..width.saturating_sub(1) {
            open(&mut grid, x, y, x + 1, y);
        }
    }
    for x in 0..width {
        for y in 0..height.saturating_sub(1) {
            open(&mut grid, x, y, x, y + 1);
        }
    }
    maze_data_from_grid(grid)
}

/// A 4-wide, 1-tall corridor with a wall between cells 2 and 3. With
/// `World`'s spawn formula (`width/2 + 0.5`), spawn lands at x = 2.5 —
/// inside cell 2 — so moving east from spawn hits this wall.
pub fn corridor_with_wall_east_of_spawn() -> MazeData {
    let mut grid = walled_grid(4, 1);
    open(&mut grid, 0, 0, 1, 0);
    open(&mut grid, 1, 0, 2, 0);
    // Deliberately not opening 2<->3: that's the wall under test.
    maze_data_from_grid(grid)
}

/// A fake client: its own loopback socket with a bounded recv timeout, so
/// a test expecting "nothing else arrives" doesn't hang forever.
pub struct FakeClient {
    socket: UdpSocket,
    server_addr: SocketAddr,
}

impl FakeClient {
    pub fn connect(server_addr: SocketAddr) -> Self {
        Self::connect_with_timeout(server_addr, DEFAULT_TEST_TIMEOUT)
    }

    pub fn connect_with_timeout(server_addr: SocketAddr, timeout: Duration) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(timeout)).unwrap();
        FakeClient { socket, server_addr }
    }

    pub fn send(&self, msg: &ClientMsg) {
        let bytes = protocol::encode(msg).unwrap();
        self.socket.send_to(&bytes, self.server_addr).unwrap();
    }

    pub fn recv(&self) -> ServerMsg {
        let mut buf = [0u8; MAX_PAYLOAD_BYTES];
        let (len, _addr) = self
            .socket
            .recv_from(&mut buf)
            .expect("no response from server within the test timeout");
        protocol::decode(&buf[..len]).expect("malformed ServerMsg")
    }

    pub fn try_recv(&self) -> Option<ServerMsg> {
        let mut buf = [0u8; MAX_PAYLOAD_BYTES];
        let (len, _addr) = self.socket.recv_from(&mut buf).ok()?;
        Some(protocol::decode(&buf[..len]).expect("malformed ServerMsg"))
    }

    pub fn join(&self, name: &str) -> ServerMsg {
        self.send(&ClientMsg::Join {
            name: name.to_string(),
            protocol_version: PROTOCOL_VERSION,
            generator_version: GENERATOR_VERSION,
        });
        self.recv()
    }
}
