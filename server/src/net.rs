//! Socket I/O and connection lifecycle (ARCHITECTURE.md §4.1, §4.2).
//!
//! Milestone 6 scope only: players exist in the table, get a `Welcome`/
//! `Rejected`, and are dropped on idle timeout — but nothing moves yet.
//! The two-thread split (blocking socket I/O thread + a second thread that
//! owns all mutable state) is the same shape Milestone 7's real
//! fixed-timestep simulation thread will take over; this lifecycle loop is
//! its Milestone 6 stand-in, not a separate design.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::config::{Config, GENERATOR_VERSION, MAX_PAYLOAD_BYTES, PROTOCOL_VERSION};
use common::maze::{self, MazeData, MazeSpec};
use common::protocol::{self, ClientMsg, EventKind, ServerMsg};
use common::reliability;
use common::types::PlayerId;

/// How often the lifecycle loop wakes up even with no incoming message, to
/// check idle timeouts and due event retries. Server-internal timing, not
/// a cross-cutting protocol constant, so it lives here rather than in
/// `common::config`.
const LIFECYCLE_TICK_MS: u64 = 20;

/// Default maze generated at server startup. No `server::levels` table
/// yet (that's Milestone 14) — every joining player gets this same maze
/// until then.
const STARTUP_MAZE_WIDTH: u16 = 20;
const STARTUP_MAZE_HEIGHT: u16 = 20;
const STARTUP_MAZE_BRAID_FACTOR: f32 = 0.3;

/// Binds `config.bind_addr`, spawns the socket I/O thread and the
/// connection-lifecycle thread, and returns the address actually bound
/// (production already knows it; a test using `127.0.0.1:0` needs the
/// OS-assigned port handed back). Both threads run for the life of the
/// process — no graceful shutdown at this milestone, matching the "server
/// runs forever" production model; test binaries clean up their sockets
/// on process exit.
pub fn spawn(config: Config) -> std::io::Result<SocketAddr> {
    let socket = UdpSocket::bind(config.bind_addr)?;
    let local_addr = socket.local_addr()?;
    let recv_socket = socket.try_clone()?;

    let (tx, rx) = mpsc::channel::<(SocketAddr, ClientMsg)>();

    thread::spawn(move || io_loop(recv_socket, tx));
    thread::spawn(move || lifecycle_loop(socket, config, rx));

    Ok(local_addr)
}

/// Blocking recv loop. Deserializes and forwards `(SocketAddr, ClientMsg)`
/// to the lifecycle thread; never touches game state itself (§4.1 — a
/// slow/blocking socket call must never stall anything the other thread
/// is doing, and vice versa).
fn io_loop(socket: UdpSocket, tx: mpsc::Sender<(SocketAddr, ClientMsg)>) {
    let mut buf = [0u8; MAX_PAYLOAD_BYTES];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((len, addr)) => match protocol::decode::<ClientMsg>(&buf[..len]) {
                Ok(msg) => {
                    if tx.send((addr, msg)).is_err() {
                        return; // lifecycle thread is gone; nothing left to do
                    }
                }
                Err(_) => continue, // malformed/hostile datagram: drop silently
            },
            Err(_) => continue, // transient socket error: keep looping
        }
    }
}

struct PlayerRecord {
    id: PlayerId,
    addr: SocketAddr,
    name: String,
    last_seen_ms: u64,
    events: reliability::Sender<EventKind>,
}

struct Server {
    config: Config,
    players: HashMap<PlayerId, PlayerRecord>,
    addr_to_id: HashMap<SocketAddr, PlayerId>,
    next_player_id: PlayerId,
    next_event_id: u32,
    maze: MazeData,
    level: u8,
    level_epoch: u32,
}

impl Server {
    fn new(config: Config) -> Self {
        let spec = MazeSpec {
            seed: rand::random(),
            width: STARTUP_MAZE_WIDTH,
            height: STARTUP_MAZE_HEIGHT,
            braid_factor: STARTUP_MAZE_BRAID_FACTOR,
            generator_version: GENERATOR_VERSION,
        };
        Server {
            config,
            players: HashMap::new(),
            addr_to_id: HashMap::new(),
            next_player_id: 1,
            next_event_id: 1,
            maze: maze::generate(spec),
            level: 0,
            level_epoch: 0,
        }
    }

    fn handle_message(&mut self, socket: &UdpSocket, addr: SocketAddr, msg: ClientMsg) {
        match msg {
            ClientMsg::Join {
                name,
                protocol_version,
                generator_version,
            } => self.handle_join(socket, addr, name, protocol_version, generator_version),
            ClientMsg::Ping { nonce } => {
                self.touch(addr);
                send(
                    socket,
                    addr,
                    &ServerMsg::Pong {
                        nonce,
                        server_tick: 0, // no tick loop yet (Milestone 7)
                        server_time_ms: now_ms(),
                    },
                );
            }
            ClientMsg::Ack { event_id } => {
                if let Some(player) = self.player_at_mut(addr) {
                    player.events.ack(event_id);
                }
            }
            ClientMsg::Leave => self.remove_player_by_addr(socket, addr),
            // No simulation yet (Milestone 7), but an Input is still
            // evidence the player is alive — matches §4.2's "no
            // Input/Ping received" timeout wording exactly.
            ClientMsg::Input { .. } => self.touch(addr),
        }
    }

    fn handle_join(
        &mut self,
        socket: &UdpSocket,
        addr: SocketAddr,
        name: String,
        protocol_version: u16,
        generator_version: u16,
    ) {
        if protocol_version != PROTOCOL_VERSION {
            send(
                socket,
                addr,
                &ServerMsg::Rejected {
                    reason: "protocol version mismatch".into(),
                },
            );
            return;
        }
        // Checked separately from protocol_version: a generator drift is
        // what makes `MazeSource::Generated` silently unsafe (both sides
        // would regenerate different mazes from the same seed), so it
        // gets its own reason string rather than folding into a generic
        // "version mismatch".
        if generator_version != GENERATOR_VERSION {
            send(
                socket,
                addr,
                &ServerMsg::Rejected {
                    reason: "generator version mismatch".into(),
                },
            );
            return;
        }
        if self.players.len() >= self.config.max_players {
            send(
                socket,
                addr,
                &ServerMsg::Rejected {
                    reason: "server full".into(),
                },
            );
            return;
        }
        if self.players.values().any(|p| p.name == name) {
            send(
                socket,
                addr,
                &ServerMsg::Rejected {
                    reason: "name already in use".into(),
                },
            );
            return;
        }

        let id = self.next_player_id;
        self.next_player_id += 1;
        let now = now_ms();
        self.players.insert(
            id,
            PlayerRecord {
                id,
                addr,
                name: name.clone(),
                last_seen_ms: now,
                events: reliability::Sender::new(
                    self.config.retry_interval_ms,
                    self.config.max_retry_attempts,
                ),
            },
        );
        self.addr_to_id.insert(addr, id);

        send(
            socket,
            addr,
            &ServerMsg::Welcome {
                player_id: id,
                level: self.level,
                level_epoch: self.level_epoch,
                maze: self.maze.source.clone(),
                tick_rate_hz: self.config.tick_hz,
                server_tick: 0,
                server_time_ms: now,
            },
        );

        // Only to everyone else (§4.2) — the new joiner already learned
        // their own id from Welcome. Known gap until Milestone 7 adds
        // WorldState: a newly joined player has no way to learn about
        // already-connected players yet, since that's what WorldState
        // snapshots are for, and this milestone doesn't broadcast those.
        self.broadcast_event(socket, EventKind::PlayerJoined { id, name }, Some(id));
    }

    fn broadcast_event(&mut self, socket: &UdpSocket, kind: EventKind, exclude: Option<PlayerId>) {
        let event_id = self.next_event_id;
        self.next_event_id += 1;
        let now = now_ms();
        for player in self.players.values_mut() {
            if Some(player.id) == exclude {
                continue;
            }
            player.events.send(event_id, kind.clone(), now);
            send(
                socket,
                player.addr,
                &ServerMsg::Event {
                    event_id,
                    kind: kind.clone(),
                },
            );
        }
    }

    fn check_idle_timeouts(&mut self, socket: &UdpSocket) {
        let now = now_ms();
        let timed_out: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, p)| now.saturating_sub(p.last_seen_ms) > self.config.client_timeout_ms)
            .map(|(&id, _)| id)
            .collect();
        for id in timed_out {
            self.remove_player(socket, id);
        }
    }

    fn retry_due_events(&mut self, socket: &UdpSocket) {
        let now = now_ms();
        for player in self.players.values_mut() {
            let tick = player.events.due_for_retry(now);
            for (event_id, kind) in tick.to_resend {
                send(socket, player.addr, &ServerMsg::Event { event_id, kind });
            }
            // §3.3: giving up is a real, logged outcome, not a panic.
            for event_id in tick.gave_up {
                eprintln!(
                    "server: gave up delivering event {event_id} to player {}",
                    player.id
                );
            }
        }
    }

    fn remove_player(&mut self, socket: &UdpSocket, id: PlayerId) {
        if let Some(player) = self.players.remove(&id) {
            self.addr_to_id.remove(&player.addr);
            self.broadcast_event(socket, EventKind::PlayerLeft { id }, None);
        }
    }

    fn remove_player_by_addr(&mut self, socket: &UdpSocket, addr: SocketAddr) {
        if let Some(&id) = self.addr_to_id.get(&addr) {
            self.remove_player(socket, id);
        }
    }

    fn player_at_mut(&mut self, addr: SocketAddr) -> Option<&mut PlayerRecord> {
        let id = *self.addr_to_id.get(&addr)?;
        self.players.get_mut(&id)
    }

    fn touch(&mut self, addr: SocketAddr) {
        if let Some(player) = self.player_at_mut(addr) {
            player.last_seen_ms = now_ms();
        }
    }
}

fn lifecycle_loop(socket: UdpSocket, config: Config, rx: mpsc::Receiver<(SocketAddr, ClientMsg)>) {
    let mut server = Server::new(config);
    loop {
        match rx.recv_timeout(Duration::from_millis(LIFECYCLE_TICK_MS)) {
            Ok((addr, msg)) => {
                server.handle_message(&socket, addr, msg);
                // Drain whatever else is already queued before doing the
                // periodic checks below, instead of waiting out a full
                // LIFECYCLE_TICK_MS between each queued message.
                while let Ok((addr, msg)) = rx.try_recv() {
                    server.handle_message(&socket, addr, msg);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return, // I/O thread gone
        }
        server.check_idle_timeouts(&socket);
        server.retry_due_events(&socket);
    }
}

fn send(socket: &UdpSocket, addr: SocketAddr, msg: &ServerMsg) {
    match protocol::encode(msg) {
        Ok(bytes) => {
            let _ = socket.send_to(&bytes, addr); // best-effort; UDP has no delivery guarantee here
        }
        Err(e) => eprintln!("server: failed to encode {msg:?}: {e}"),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
