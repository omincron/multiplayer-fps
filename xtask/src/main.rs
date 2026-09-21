//! Bot-client load generator (ARCHITECTURE.md §2, §8.5; PLAN.md
//! Milestone 15). Spawns `bot_count` real, independent UDP clients against
//! a real running `server` process, each sending continuous randomized
//! movement (and occasional shots) — for the pre-submission "run a real
//! client alongside 9+ bots for 3+ minutes" manual soak test.
//!
//! Deliberately not a real AI (no pathfinding, no wall-following) — that's
//! the separate `server::bots` bonus feature from ARCHITECTURE.md §7.2,
//! implemented server-side as ordinary `PlayerState`s. This binary's only
//! job is to generate realistic network *load*: real UDP clients, real
//! join handshakes, real continuous input, indistinguishable from a human
//! mashing keys at the protocol level, so it exercises the exact same
//! server code path a real player does.
//!
//! Usage: `cargo run -p xtask -- <server_addr> <bot_count> [duration_secs]`
//! With no `duration_secs`, bots run until the process is killed (Ctrl+C).

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use common::config::{
    GENERATOR_VERSION, INPUT_DT_MS, JOIN_MAX_ATTEMPTS, JOIN_RETRY_BASE_MS, MAX_PAYLOAD_BYTES,
    PROTOCOL_VERSION,
};
use common::protocol::{ClientMsg, ServerMsg, decode, encode};
use common::types::Vec2;
use rand::Rng;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: {} <server_addr> <bot_count> [duration_secs]",
            args.first().map(String::as_str).unwrap_or("xtask")
        );
        std::process::exit(1);
    }
    let server_addr: SocketAddr = args[1].parse().unwrap_or_else(|error| {
        eprintln!("invalid server address {:?}: {error}", args[1]);
        std::process::exit(1);
    });
    let bot_count: usize = args[2].parse().unwrap_or_else(|error| {
        eprintln!("invalid bot count {:?}: {error}", args[2]);
        std::process::exit(1);
    });
    let duration = args
        .get(3)
        .map(|value| {
            let secs: u64 = value.parse().unwrap_or_else(|error| {
                eprintln!("invalid duration_secs {value:?}: {error}");
                std::process::exit(1);
            });
            Duration::from_secs(secs)
        })
        .unwrap_or(Duration::MAX);

    println!("xtask: spawning {bot_count} bots against {server_addr}");
    let handles: Vec<_> = (0..bot_count)
        .map(|i| thread::spawn(move || run_bot(i, server_addr, duration)))
        .collect();
    for handle in handles {
        let _ = handle.join();
    }
}

fn run_bot(index: usize, server_addr: SocketAddr, duration: Duration) {
    let name = format!("bot{index}");
    let socket = match connect(&name, server_addr) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("xtask: {name} failed to connect: {error}");
            return;
        }
    };
    println!("xtask: {name} connected");

    // No further reads after the handshake: a bot's job is to generate
    // load, not to react to the world, so it never needs `WorldState`/
    // `Event` traffic. Anything the server sends just accumulates in the
    // OS socket buffer and is dropped once full — harmless for a
    // short-lived load-test process, and simpler than draining a channel
    // this binary has no use for.
    let mut rng = rand::thread_rng();
    let input_dt = Duration::from_millis(INPUT_DT_MS as u64);
    let mut input_tick: u32 = 1;
    let mut move_dir = random_direction(&mut rng);
    let mut facing: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let mut next_direction_change = Instant::now() + Duration::from_secs(2);
    let start = Instant::now();

    while start.elapsed() < duration {
        let now = Instant::now();
        if now >= next_direction_change {
            move_dir = random_direction(&mut rng);
            facing = rng.gen_range(0.0..std::f32::consts::TAU);
            next_direction_change = now + Duration::from_millis(rng.gen_range(1000..3000));
        }
        let shoot = rng.gen_bool(0.05);
        let message = ClientMsg::Input {
            input_tick,
            move_dir,
            facing,
            shoot,
        };
        input_tick = input_tick.wrapping_add(1);
        if let Ok(bytes) = encode(&message) {
            let _ = socket.send(&bytes);
        }
        thread::sleep(input_dt);
    }

    let _ = socket.send(&encode(&ClientMsg::Leave).unwrap_or_default());
    println!("xtask: {name} finished its run");
}

fn random_direction(rng: &mut impl Rng) -> Vec2 {
    let angle: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    Vec2::new(angle.cos(), angle.sin())
}

/// Binds a socket and performs the same handshake `client::net::connect`
/// does (Milestone 8), independently reimplemented here since `xtask`
/// deliberately doesn't depend on the `client` crate (a load-generating
/// bot has no business pulling in `macroquad`/rendering).
fn connect(name: &str, server_addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let bind_addr = if server_addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let socket = UdpSocket::bind(bind_addr)?;
    socket.connect(server_addr)?;
    socket.set_read_timeout(Some(Duration::from_millis(JOIN_RETRY_BASE_MS)))?;

    let join = ClientMsg::Join {
        name: name.to_string(),
        protocol_version: PROTOCOL_VERSION,
        generator_version: GENERATOR_VERSION,
    };
    let payload = encode(&join).map_err(|e| std::io::Error::other(e.to_string()))?;

    let mut delay = Duration::from_millis(JOIN_RETRY_BASE_MS);
    let mut buf = [0u8; MAX_PAYLOAD_BYTES];
    for attempt in 1..=JOIN_MAX_ATTEMPTS {
        socket.send(&payload)?;
        match socket.recv(&mut buf) {
            Ok(len) => match decode::<ServerMsg>(&buf[..len]) {
                Ok(ServerMsg::Welcome { .. }) => return Ok(socket),
                Ok(ServerMsg::Rejected { reason }) => {
                    return Err(std::io::Error::other(format!("rejected: {reason}")));
                }
                _ => continue,
            },
            Err(error) if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut => {
                if attempt == JOIN_MAX_ATTEMPTS {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        format!("no response after {attempt} attempts"),
                    ));
                }
                thread::sleep(delay);
                delay *= 2;
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(ErrorKind::TimedOut, "join handshake exhausted"))
}
