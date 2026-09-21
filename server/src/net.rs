//! Socket I/O only (ARCHITECTURE.md §4.1, §2: "recv loop, per-client
//! address<->id table"). Binds the socket, runs the blocking recv loop,
//! and spawns `world::run` as the authoritative simulation thread. Owns
//! no player/game state itself — that's `crate::world::World` as of
//! Milestone 7 (Milestone 6's lifecycle loop lived here; it moved to
//! `world.rs` and grew into the real tick loop, per PLAN.md's Milestone 7
//! build note).

use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::thread;

use common::config::{Config, GENERATOR_VERSION, MAX_PAYLOAD_BYTES};
use common::maze::{self, MazeData, MazeSpec};
use common::protocol::{self, ClientMsg, ServerMsg};

use crate::levels::LEVELS;
use crate::world;

/// Binds `config.bind_addr`, spawns the socket I/O thread and the
/// authoritative simulation thread, and returns the address actually
/// bound (production already knows it; a test using `127.0.0.1:0` needs
/// the OS-assigned port handed back). Both threads run for the life of
/// the process — no graceful shutdown yet; test binaries clean up their
/// sockets on process exit.
pub fn spawn(config: Config) -> std::io::Result<SocketAddr> {
    let level = &LEVELS[0];
    let spec = MazeSpec {
        seed: rand::random(),
        width: level.grid_w,
        height: level.grid_h,
        braid_factor: level.braid_factor,
        generator_version: GENERATOR_VERSION,
    };
    spawn_with_maze(config, maze::generate(spec))
}

/// Same as [`spawn`], but with an explicit maze instead of a fresh random
/// one — this is what lets `server/tests/` pin a known, deterministic
/// layout (a real random maze makes assertions like "moving east is
/// unobstructed" or "there's a wall exactly here" unpredictable; see
/// `tests/support` for the hand-built test fixtures).
pub fn spawn_with_maze(config: Config, maze: MazeData) -> std::io::Result<SocketAddr> {
    let socket = UdpSocket::bind(config.bind_addr)?;
    let local_addr = socket.local_addr()?;
    let recv_socket = socket.try_clone()?;

    let (tx, rx) = mpsc::channel::<(SocketAddr, ClientMsg)>();

    thread::spawn(move || io_loop(recv_socket, tx));
    thread::spawn(move || world::run(socket, config, rx, maze));

    Ok(local_addr)
}

/// Blocking recv loop. Deserializes and forwards `(SocketAddr, ClientMsg)`
/// to the simulation thread; never touches game state itself (§4.1 — a
/// slow/blocking socket call must never stall the tick loop, and vice
/// versa).
fn io_loop(socket: UdpSocket, tx: mpsc::Sender<(SocketAddr, ClientMsg)>) {
    let mut buf = [0u8; MAX_PAYLOAD_BYTES];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((len, addr)) => match protocol::decode::<ClientMsg>(&buf[..len]) {
                Ok(msg) => {
                    if tx.send((addr, msg)).is_err() {
                        return; // simulation thread is gone; nothing left to do
                    }
                }
                Err(_) => continue, // malformed/hostile datagram: drop silently
            },
            Err(_) => continue, // transient socket error: keep looping
        }
    }
}

/// Send one message to one address. `pub(crate)` — `world.rs` uses this
/// for everything except the per-tick `WorldState` broadcast, which
/// encodes once and sends to every player itself rather than re-encoding
/// per recipient through here.
pub(crate) fn send(socket: &UdpSocket, addr: SocketAddr, msg: &ServerMsg) {
    match protocol::encode(msg) {
        Ok(bytes) => {
            let _ = socket.send_to(&bytes, addr); // best-effort; UDP has no delivery guarantee here
        }
        Err(e) => eprintln!("server: failed to encode {msg:?}: {e}"),
    }
}
