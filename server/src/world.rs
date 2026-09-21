//! Authoritative game state and the fixed-timestep simulation tick loop
//! (ARCHITECTURE.md §4.1, §4.2, §4.3). Milestone 6's connection-lifecycle
//! thread lived in `net.rs`; it moved here and grew into the real tick
//! loop for Milestone 7 — same shape (own all mutable state, drain the
//! input channel, act), now with real movement and a `WorldState`
//! broadcast every tick.

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::Rng;

use common::config::{
    Config, GENERATOR_VERSION, HIT_DAMAGE, INPUT_DT_MS, MAX_HP, MAX_INPUT_QUEUE, MAX_SHOT_RANGE,
    PLAYER_SPEED_UPS, PROTOCOL_VERSION,
};
use common::maze::MazeData;
use common::protocol::{self, ClientMsg, EventKind, PlayerSnapshot, ServerMsg};
use common::reliability;
use common::sim::{self, RaycastHit};
use common::types::{PlayerId, Tick, Vec2};

use crate::net::send;

/// Player collision radius passed to `resolve_move`. Well under the 0.5
/// ceiling `common::sim::circle_fits` requires — see that function's own
/// doc comment and Milestone 4's PLAN.md log entry, which flagged this as
/// a constraint for whichever milestone picked a real radius. This one.
const PLAYER_RADIUS: f32 = 0.25;

struct QueuedInput {
    input_tick: Tick,
    move_dir: Vec2,
    facing: f32,
    shoot: bool,
}

/// A shot to resolve, captured at the position/facing the shooter actually
/// had *at that input step* — not wherever they end up after every queued
/// input this tick finishes applying. Resolution itself is deferred to a
/// separate pass (`resolve_shots`) because it needs read access to every
/// other player's position while `apply_queued_input` is still mutably
/// borrowing the shooter.
struct ShotRequest {
    shooter: PlayerId,
    origin: Vec2,
    facing: f32,
}

struct PlayerState {
    id: PlayerId,
    addr: SocketAddr,
    name: String,
    pos: Vec2,
    facing: f32,
    hp: u8,
    last_seen_ms: u64,
    input_queue: VecDeque<QueuedInput>,
    last_input_tick: Tick,
    events: reliability::Sender<EventKind>,
}

pub struct World {
    config: Config,
    players: HashMap<PlayerId, PlayerState>,
    addr_to_id: HashMap<SocketAddr, PlayerId>,
    next_player_id: PlayerId,
    next_event_id: u32,
    maze: MazeData,
    /// No `server::levels` table yet (Milestone 14) — fixed at 0 for now.
    level: u8,
    level_epoch: u32,
    tick: Tick,
    /// Fixed spawn point: the maze's center cell. Always valid floor space
    /// regardless of that cell's walls (no cell is ever fully isolated,
    /// §5.3 invariant 2) — not a "pick an open, unoccupied cell" spawn
    /// algorithm, which is Milestone 13's respawn logic, not join's.
    spawn_pos: Vec2,
}

impl World {
    fn new(config: Config, maze: MazeData) -> Self {
        // Integer cell index, not `dimension as f32 / 2.0 + 0.5` — that
        // float formula looks equivalent but isn't: for a degenerate
        // height-1 (or width-1) grid it evaluates to exactly `height`
        // itself (1.0/2.0 + 0.5 == 1.0), landing one full unit outside
        // the grid and making every movement attempt fail immediately at
        // `circle_fits`'s bounds check. Center *cell*, then +0.5, is
        // correct for every grid size including that one.
        let spawn_pos = Vec2::new(
            (maze.grid.width / 2) as f32 + 0.5,
            (maze.grid.height / 2) as f32 + 0.5,
        );
        World {
            config,
            players: HashMap::new(),
            addr_to_id: HashMap::new(),
            next_player_id: 1,
            next_event_id: 1,
            maze,
            level: 0,
            level_epoch: 0,
            tick: 0,
            spawn_pos,
        }
    }

    fn handle_message(&mut self, socket: &UdpSocket, addr: SocketAddr, msg: ClientMsg) {
        match msg {
            ClientMsg::Join {
                name,
                protocol_version,
                generator_version,
            } => self.handle_join(socket, addr, name, protocol_version, generator_version),
            ClientMsg::Input {
                input_tick,
                move_dir,
                facing,
                shoot,
            } => {
                if let Some(player) = self.player_at_mut(addr) {
                    player.last_seen_ms = now_ms();
                    // No cap on the queue itself — only on how many of it
                    // get *consumed* per tick (§4.3: "it just backs up").
                    // Capping insertion instead would silently discard
                    // inputs rather than delaying them.
                    player.input_queue.push_back(QueuedInput {
                        input_tick,
                        move_dir,
                        facing,
                        shoot,
                    });
                }
            }
            ClientMsg::Ping { nonce } => {
                self.touch(addr);
                send(
                    socket,
                    addr,
                    &ServerMsg::Pong {
                        nonce,
                        server_tick: self.tick,
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
            PlayerState {
                id,
                addr,
                name: name.clone(),
                pos: self.spawn_pos,
                facing: 0.0,
                hp: MAX_HP,
                last_seen_ms: now,
                input_queue: VecDeque::new(),
                last_input_tick: 0,
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
                server_tick: self.tick,
                server_time_ms: now,
            },
        );

        // Only to everyone else (§4.2) — the new joiner already learned
        // their own id from Welcome, and now also gets the roster via the
        // very next WorldState broadcast (the Milestone 6 gap — no way
        // for a new joiner to learn about existing players — is closed as
        // of this milestone).
        self.broadcast_event(socket, EventKind::PlayerJoined { id, name }, Some(id));
    }

    /// One fixed-timestep tick: apply queued input, check idle timeouts,
    /// retry due events, broadcast `WorldState`.
    fn tick(&mut self, socket: &UdpSocket) {
        self.tick += 1;
        let shots = self.apply_queued_input();
        self.resolve_shots(socket, shots);
        self.check_idle_timeouts(socket);
        self.retry_due_events(socket);
        self.broadcast_world_state(socket);
    }

    /// One `resolve_move` step per queued input, oldest first, capped at
    /// `MAX_INPUT_QUEUE` consumed *this tick* (§4.3) — never just the
    /// newest: network jitter routinely delivers two inputs in one tick
    /// window, and keeping only the latest would silently shorten that
    /// player's movement and force an unearned reconciliation correction
    /// (Milestone 10) for a player who did nothing wrong. Extra queued
    /// inputs beyond the cap simply wait for the next tick ("it just backs
    /// up") rather than being dropped.
    fn apply_queued_input(&mut self) -> Vec<ShotRequest> {
        let dt_s = INPUT_DT_MS / 1000.0;
        let mut shots = Vec::new();
        for player in self.players.values_mut() {
            for _ in 0..MAX_INPUT_QUEUE {
                let Some(input) = player.input_queue.pop_front() else {
                    break;
                };
                player.pos = sim::resolve_move(
                    &self.maze.grid,
                    player.pos,
                    input.move_dir,
                    PLAYER_SPEED_UPS,
                    dt_s,
                    PLAYER_RADIUS,
                );
                player.facing = input.facing;
                // Highest *consumed* tick, not highest *received* — a
                // player's queue can (briefly) hold inputs out of order
                // relative to arrival, though never out of order relative
                // to each other since UDP reordering only affects delivery
                // order into the queue, not FIFO draining from it. Guard
                // with max() anyway: cheap, and correct if that ever
                // changes.
                player.last_input_tick = player.last_input_tick.max(input.input_tick);
                if input.shoot {
                    shots.push(ShotRequest {
                        shooter: player.id,
                        origin: player.pos,
                        facing: player.facing,
                    });
                }
            }
        }
        shots
    }

    /// Resolves each shot queued this tick against the *current* positions
    /// of every other player (re-read fresh per shot, so two separate shots
    /// this tick that both connect see a consistent, up-to-date world —
    /// e.g. a target respawned by an earlier shot isn't shot again at its
    /// old position) and the maze's walls. §4.3: hitscan, resolved inside
    /// the tick that requested it — there is no projectile entity anywhere
    /// in `World` or on the wire.
    fn resolve_shots(&mut self, socket: &UdpSocket, shots: Vec<ShotRequest>) {
        for shot in shots {
            // The shooter may have timed out/left earlier this same tick.
            if !self.players.contains_key(&shot.shooter) {
                continue;
            }
            let candidates: Vec<(PlayerId, Vec2)> = self
                .players
                .values()
                .filter(|p| p.id != shot.shooter)
                .map(|p| (p.id, p.pos))
                .collect();
            let hit = sim::raycast_hit(
                &self.maze.grid,
                shot.origin,
                shot.facing,
                &candidates,
                PLAYER_RADIUS,
                MAX_SHOT_RANGE,
            );
            if let Some(RaycastHit::Player { id: target, .. }) = hit {
                self.apply_hit(socket, shot.shooter, target);
            }
            // A wall hit or a clean miss has no gameplay effect to report —
            // no `Event` variant exists for "shot and missed" (§3.2), and
            // none is needed.
        }
    }

    /// Applies one hit's damage, broadcasts `Hit`, and — at 0 hp —
    /// broadcasts `Killed` and immediately respawns the victim. No death/
    /// waiting period: the victim's `PlayerState` goes from 0 hp straight
    /// to a fresh spawn within this same tick.
    fn apply_hit(&mut self, socket: &UdpSocket, shooter: PlayerId, target: PlayerId) {
        let Some(player) = self.players.get_mut(&target) else {
            return; // target left/timed out between the shot and resolution
        };
        player.hp = player.hp.saturating_sub(HIT_DAMAGE);
        let target_hp = player.hp;
        self.broadcast_event(
            socket,
            EventKind::Hit {
                shooter,
                target,
                target_hp,
            },
            None,
        );

        if target_hp == 0 {
            self.broadcast_event(
                socket,
                EventKind::Killed {
                    victim: target,
                    killer: shooter,
                },
                None,
            );
            self.respawn(socket, target);
        }
    }

    /// Picks a random cell within the current maze's bounds and moves the
    /// player there at full hp. No "is this cell walled on all sides"
    /// check is needed: walls live on cell *edges* (§5.1), never inside a
    /// cell, and §5.3 invariant 2 guarantees no cell is isolated anyway —
    /// so every in-bounds grid cell is valid floor space to respawn onto.
    fn respawn(&mut self, socket: &UdpSocket, id: PlayerId) {
        let pos = self.random_open_cell();
        if let Some(player) = self.players.get_mut(&id) {
            player.pos = pos;
            player.hp = MAX_HP;
        }
        self.broadcast_event(socket, EventKind::Respawned { id, pos }, None);
    }

    fn random_open_cell(&self) -> Vec2 {
        let mut rng = rand::thread_rng();
        let x = rng.gen_range(0..self.maze.grid.width);
        let y = rng.gen_range(0..self.maze.grid.height);
        Vec2::new(x as f32 + 0.5, y as f32 + 0.5)
    }

    /// Unreliable, unordered, latest-wins (§3.1) — no reliability wrapper,
    /// straight socket sends, encoded once for every recipient rather than
    /// once per recipient.
    fn broadcast_world_state(&self, socket: &UdpSocket) {
        let players: Vec<PlayerSnapshot> = self
            .players
            .values()
            .map(|p| PlayerSnapshot {
                id: p.id,
                pos: p.pos,
                facing: p.facing,
                hp: p.hp,
                last_input_tick: p.last_input_tick,
            })
            .collect();
        let msg = ServerMsg::WorldState {
            tick: self.tick,
            level_epoch: self.level_epoch,
            players,
        };
        match protocol::encode(&msg) {
            Ok(bytes) => {
                for player in self.players.values() {
                    let _ = socket.send_to(&bytes, player.addr);
                }
            }
            Err(e) => eprintln!("server: failed to encode WorldState: {e}"),
        }
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

    fn player_at_mut(&mut self, addr: SocketAddr) -> Option<&mut PlayerState> {
        let id = *self.addr_to_id.get(&addr)?;
        self.players.get_mut(&id)
    }

    fn touch(&mut self, addr: SocketAddr) {
        if let Some(player) = self.player_at_mut(addr) {
            player.last_seen_ms = now_ms();
        }
    }
}

/// The simulation thread's entry point (ARCHITECTURE.md §4.1): fixed
/// `config.tick_hz` timestep. Drains whatever's arrived on `rx` before
/// each tick deadline, but always wakes at the deadline even with nothing
/// queued, so the tick rate holds independent of traffic.
pub fn run(socket: UdpSocket, config: Config, rx: mpsc::Receiver<(SocketAddr, ClientMsg)>, maze: MazeData) {
    let tick_interval = Duration::from_millis(1000 / config.tick_hz as u64);
    let mut world = World::new(config, maze);
    let mut next_tick_at = Instant::now() + tick_interval;

    loop {
        let wait = next_tick_at.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok((addr, msg)) => {
                world.handle_message(&socket, addr, msg);
                while let Ok((addr, msg)) = rx.try_recv() {
                    world.handle_message(&socket, addr, msg);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return, // I/O thread gone
        }

        if Instant::now() >= next_tick_at {
            world.tick(&socket);
            next_tick_at += tick_interval;
            // Fell far behind (e.g. a debugger pause, or sustained
            // overload): resync to now instead of bursting through a
            // catch-up backlog of ticks, which would just make things
            // worse under real load.
            if next_tick_at < Instant::now() {
                next_tick_at = Instant::now() + tick_interval;
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
