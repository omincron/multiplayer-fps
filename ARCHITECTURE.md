# Architecture — Multiplayer Maze FPS ("Maze Wars" clone)

This document is the technical reference for building this project. It is written
to be consumed by a coding agent as well as a human. Every section states
**contracts** (types, invariants, responsibilities) precisely enough that an
agent can implement against them without guessing, and precisely enough that
tests can be written against them independently of the implementation.

If you are an agent picking up this project: read this file fully before
writing code. Read `PLAN.md` for the order of work. Do not deviate from the
contracts in this file without updating it first — the contracts are what the
tests are written against.

---

## 1. Goals and non-negotiable constraints

These come directly from the assignment's audit checklist. Every architectural
decision below exists to satisfy one of these:

1. Server and client each **compile and run with zero warnings**
   (`cargo build` clean output, checked with `cargo clean && cargo build`).
2. Client startup flow, in order: prompt for server IP:port → prompt for
   username → connect → open GUI. This must work as a plain CLI prompt
   sequence, independent of any GUI-based launcher bonus feature.
3. Client renders: first-person view of the maze, a minimap showing the
   player's own position (updating live as they move), and a visible,
   numeric, live-updating FPS counter.
4. Sustained frame rate **> 50 fps** on the client, including while 10+
   clients are connected to one server.
5. Movement of self and other players must look and feel smooth,
   independent of the raw fps number — this is a distinct requirement
   from "the fps counter says a big number."
6. Transport is UDP. Server binds `0.0.0.0` (accepts arbitrary source IPs,
   not just loopback) even if grading is done over loopback.
7. Server supports at least 10 simultaneous client connections.
8. At least 3 maze difficulty levels, difficulty = harder navigation
   (bigger, fewer loops, more dead ends), not just cosmetic changes.

Everything below is designed to satisfy these constraints, not the other
way around.

---

## 2. Workspace layout

```
maze_wars/
├── Cargo.toml                 # workspace manifest
├── ARCHITECTURE.md
├── PLAN.md
├── common/                    # shared, pure, no I/O — the most heavily tested crate
│   ├── src/
│   │   ├── lib.rs
│   │   ├── protocol.rs        # wire message types + (de)serialization
│   │   ├── reliability.rs     # ack/retry layer for "important" UDP messages
│   │   ├── maze.rs            # maze data structure + generation algorithm
│   │   ├── sim.rs             # movement/collision/shooting resolution (pure fns)
│   │   ├── interp.rs          # snapshot interpolation/extrapolation math
│   │   └── types.rs           # PlayerState, Vec2, PlayerId, etc.
│   └── tests/                 # integration tests for common (see §8)
├── server/
│   ├── src/
│   │   ├── main.rs            # CLI arg parsing, socket bind, spawn loops
│   │   ├── net.rs             # recv loop, per-client address<->id table
│   │   ├── world.rs           # authoritative game state + tick loop
│   │   ├── bots.rs            # AI player driver (bonus)
│   │   └── levels.rs          # level progression / difficulty table
│   └── tests/
├── client/
│   ├── src/
│   │   ├── main.rs            # CLI prompts, connect handshake, hands off to app
│   │   ├── net.rs             # non-blocking recv thread -> channel, send helpers
│   │   ├── predict.rs         # client-side prediction + reconciliation
│   │   ├── render/
│   │   │   ├── raycast.rs     # first-person raycasting renderer
│   │   │   ├── minimap.rs
│   │   │   └── hud.rs         # fps counter, health, kill feed
│   │   ├── editor.rs          # maze editor (bonus)
│   │   └── launcher.rs        # GUI host-history launcher (bonus)
│   └── tests/
└── xtask/ (optional)          # helper binary: spawns N bot clients for load testing
```

**Rule:** `common` has zero dependencies on networking sockets, rendering, or
any I/O. Every function in `common` is a pure function of its inputs. This is
what makes it unit-testable without a server or a window. If you find
yourself wanting to test something in `common` by spinning up a socket, the
logic belongs in `server`/`client` instead, or you've mixed concerns.

---

## 3. Network protocol

### 3.1 Transport model

- UDP, one socket per process (server: one socket for all clients;
  client: one socket for the server).
- Server identifies clients by `SocketAddr` at the transport layer, mapped
  to a stable `PlayerId` (`u32`) at the application layer. `SocketAddr` can
  change (NAT rebinding) — do not use it as the long-term player identity
  anywhere except the demux table in `server::net`.
- Two message classes, handled differently:
  - **State messages** (position/orientation updates, world snapshots):
    unreliable, unordered, latest-wins. No retry. If lost, the next tick's
    message supersedes it.
  - **Event messages** (join, leave, hit/death, level change, chat): must
    arrive and must arrive at most once logically. These go through the
    reliability layer (§3.3).

### 3.2 Message types (`common::protocol`)

```rust
type PlayerId = u32;
type Tick = u32;

enum ClientMsg {
    Join { name: String, protocol_version: u16, generator_version: u16 },
    // One Input == exactly one fixed simulation step of INPUT_DT_MS.
    // `move_dir` is a *direction*, not a displacement: the server clamps it
    // to unit length and applies its own speed (§4.3).
    Input { input_tick: Tick, move_dir: Vec2, facing: f32, shoot: bool },
    Ack { event_id: u32 },        // reliability layer ack
    Ping { nonce: u32 },
    Leave,
}

enum ServerMsg {
    Welcome {
        player_id: PlayerId,
        level: u8,
        level_epoch: u32,
        maze: MazeSource,         // descriptor, not the expanded grid — see §5.1
        tick_rate_hz: u16,
        server_tick: Tick,        // clock anchor for interpolation, §6.4
        server_time_ms: u64,
    },
    Rejected { reason: String },              // e.g. server full, name taken
    WorldState {
        tick: Tick,
        level_epoch: u32,         // lets a client discard snapshots for a maze it lacks
        players: Vec<PlayerSnapshot>,
    },
    Event { event_id: u32, kind: EventKind }, // reliable channel
    Pong { nonce: u32, server_tick: Tick, server_time_ms: u64 },
}

enum EventKind {
    PlayerJoined { id: PlayerId, name: String },
    PlayerLeft { id: PlayerId },
    Hit { shooter: PlayerId, target: PlayerId, target_hp: u8 },
    Killed { victim: PlayerId, killer: PlayerId },
    Respawned { id: PlayerId, pos: Vec2 },
    LevelChanged { level: u8, level_epoch: u32, maze: MazeSource },
}

// The maze never travels as an expanded grid unless it has to: see §5.1.
enum MazeSource {
    Generated(MazeSpec),   // ~24 bytes; the client regenerates it (§5.2 is deterministic)
    Custom(MazeGrid),      // editor-authored (§7.3); only if it fits the payload budget
}

struct PlayerSnapshot {
    id: PlayerId,
    pos: Vec2,
    facing: f32,
    hp: u8,
    // Highest `input_tick` from this player the server has consumed.
    // Only meaningful to that player; required for reconciliation (§6.3).
    last_input_tick: Tick,
}
```

Contract:
- `protocol_version` must be checked by the server on `Join`; mismatch ->
  `Rejected`. This exists so future changes fail loudly instead of silently
  desyncing.
- Serialization is `bincode` via `serde`. Every message type must round-trip
  through `serialize -> deserialize` byte-identically in a unit test — this
  is a real correctness requirement (see §8), not busywork: a subtle
  `#[repr]`/enum-ordering change is exactly the kind of bug this catches.
- `generator_version` is checked the same way as `protocol_version`.
  Because `MazeSource::Generated` sends only a seed and the client
  regenerates the grid locally, a generator change that both sides fail to
  notice would have them playing on silently different mazes — worse than
  a hard rejection.
- Max UDP payload assumed: **every** message stays under
  `MAX_PAYLOAD_BYTES` (~1200) to avoid fragmentation on typical MTUs.
  This is a per-variant budget, not a `WorldState`-only one: `Welcome` and
  `LevelChanged` used to carry the expanded maze grid, which is one byte
  per cell and therefore blows the budget past roughly 34x34 — while §4.4
  *requires* level area to grow. `LevelChanged` also rides the reliable
  channel, so an IP-fragmented multi-kilobyte datagram would be retried
  whole and lost entirely if any one fragment dropped. Hence `MazeSource`
  (§5.1). `WorldState` must additionally be sized against `MAX_PLAYERS`,
  not against 10 (see test list in §8).
- The deserializer must be configured with an explicit byte limit equal to
  `MAX_PAYLOAD_BYTES`, and the recv buffer sized to match. `bincode`'s
  default configuration reads a length prefix and pre-allocates, so on a
  socket bound to `0.0.0.0` (§1.6) a ~20-byte spoofed datagram declaring a
  multi-gigabyte `String` for `Join.name` would make the server's I/O
  thread attempt that allocation and abort. Nothing legitimate exceeds the
  budget, so the limit costs nothing.

### 3.3 Reliability layer (`common::reliability`)

For `Event` messages only:
- Sender keeps a table of `event_id -> (payload, last_sent_at, attempts)`.
- Resends unacked events every `RETRY_INTERVAL_MS` up to
  `MAX_RETRY_ATTEMPTS`, then gives up and logs (do not panic).
- **Because giving up is possible, no client state may depend on events
  alone.** Membership and per-player state are reconciled from
  `WorldState` every tick: a `PlayerId` present in a snapshot but unknown
  locally is created with a placeholder name (filled in if its
  `PlayerJoined` arrives later), and a locally-known player absent from
  snapshots for `GHOST_TIMEOUT_MS` is removed. Otherwise one permanently
  dropped `PlayerLeft` leaves a ghost on the minimap forever, and a
  dropped `PlayerJoined` leaves snapshots referencing an id with no local
  record — which naively becomes a panicking map lookup or an invisible
  opponent. Receiving a snapshot for an unknown id is a normal path, never
  an error. Events stay the source of truth only for *transient* facts
  (hit, kill, respawn, level change) that no snapshot can express.
- Receiver keeps a small ring/set of recently-seen `event_id`s and drops
  duplicates before handing them to game logic. This makes event handling
  idempotent at the call site — game logic never has to think about
  duplicate delivery.
- This layer must be tested with a **simulated lossy/duplicating channel**
  (an in-memory fake, not real sockets) — see §8.2.

### 3.4 Tick / rate model

- Server simulates at a fixed tick rate, `SERVER_TICK_HZ = 30` (configurable
  constant). One `WorldState` broadcast per tick.
- Client sends `Input` at `CLIENT_INPUT_HZ = 30`: one message per fixed
  input step of `INPUT_DT_MS = 1000 / CLIENT_INPUT_HZ`. The client
  advances its own prediction by exactly one step per emitted input —
  never once per rendered frame — so both sides integrate the same number
  of steps per second whatever the frame rate (§6.3).
- Client renders at whatever rate the display loop achieves (target ≥60,
  hard floor 50). Rendering rate is decoupled from both of the above —
  this decoupling is what makes ">50fps while feeling smooth" achievable
  even though the network only updates 30 times/sec.

---

## 4. Server architecture

### 4.1 Threading model

- **Socket I/O thread**: blocking `recv_from` loop, deserializes, pushes
  `(SocketAddr, ClientMsg)` onto an MPSC channel. Never touches game state
  directly.
- **Simulation thread**: fixed-timestep loop (`SERVER_TICK_HZ`), drains the
  input channel, applies inputs to `World`, resolves collisions/shooting via
  `common::sim`, builds and sends `WorldState` to all known addresses.
- Rationale: a slow/blocking socket call must never stall the tick loop, and
  vice versa — this is what keeps tick rate (and therefore client fps
  perception) stable under load from many clients.

### 4.2 Connection lifecycle

- On `Join`: if under capacity (`config.max_players`, §9 — never a bare
  constant, because the tests in §8.3 must be able to lower it) and the
  name is not already in use, assign a new `PlayerId`, insert into
  `world`, reply `Welcome`, broadcast `PlayerJoined` to everyone else.
- **Bots occupy player slots** (§7.2): they are ordinary `PlayerState`s in
  the same table and appear in `WorldState` like anyone else, so they
  count against `max_players` and against the wire size budget. This is
  why `MAX_PLAYERS` is 20 and not 10 — 8 bots plus the 10 human clients
  the audit requires must both fit, and the size-budget test must be
  written against the same number.
- If at capacity: reply `Rejected { reason: "server full" }`. This must be
  a real, tested path (§8.3) — "accepts 10, cleanly rejects the 11th" is an
  explicit design requirement even though the audit only requires 10 to
  succeed.
- **Timeout**: if no `Input`/`Ping` received from a player for
  `config.client_timeout_ms`, drop them, broadcast `PlayerLeft`. Prevents
  dead connections from silently eating a capacity slot.

### 4.3 Authoritative simulation

- `World` owns all `PlayerState`s and the current `MazeData`. There are no
  projectile entities: shooting is hitscan (below) and is resolved inside
  the tick that requested it, so nothing in this design needs a travelling
  projectile and the wire format does not carry one.
- Movement: each player has an input **queue**, not a single latest input.
  Every tick the server drains that queue and applies each pending input
  as one `resolve_move` step of `INPUT_DT_MS`. At 30Hz tick and 30Hz input
  that is normally one step, but jitter delivers zero or two — keeping
  only the newest would silently shorten that player's movement and
  guarantee a reconciliation correction (§6.3) for a player who did
  nothing wrong.
- Two clamps make "authoritative" true rather than aspirational: cap the
  queue at `MAX_INPUT_QUEUE` steps consumed per tick (a client that floods
  inputs cannot buy extra speed, it just backs up), and normalise
  `move_dir` to at most unit length before use. `move_dir` is a direction;
  the server owns `PLAYER_SPEED_UPS`. Without the second clamp a client
  can put any magnitude it likes in the vector and teleport.
- Record the highest `input_tick` consumed per player and echo it back in
  that player's `PlayerSnapshot.last_input_tick` — §6.3 cannot reconcile
  without it.
- Shooting: instant-hit raycast along shooter facing, first collision
  (wall or player) wins, resolved by `common::sim::raycast_hit` (pure
  function).
- The server is the single source of truth for hit/damage/death. Clients
  never decide "I killed someone" — they only display what the server's
  `Event` stream tells them. This is what prevents cheating and, more
  importantly for this project, what prevents desync bugs.

### 4.4 Level/difficulty progression

- `server::levels` holds an ordered table:
  `Level { grid_w, grid_h, braid_factor, name }`, at least 3 entries,
  strictly increasing `grid_w*grid_h` and strictly decreasing
  `braid_factor` (fewer loops = more dead ends = harder — see §5.2 for why
  this specific parameter is the difficulty knob). Because both knobs move
  together, "harder" is only measurable as a *density* — dead ends per
  open cell — never as a raw count, which grows with area on its own
  (§5.3, invariant 5).
- Advancing a level increments `level_epoch`, regenerates the maze (new
  random seed), broadcasts `LevelChanged { level_epoch, .. }`, and resets
  player positions to valid spawn cells.
- `level_epoch` is stamped on every subsequent `WorldState`. A client that
  has not yet received the matching `LevelChanged` **must discard** those
  snapshots instead of rendering them. This is not a rare race:
  `LevelChanged` is reliable but retried on a 100ms timer, while snapshots
  arrive every 33ms, so one dropped datagram guarantees several snapshots
  for a maze the client does not have — drawn against the old geometry
  they put players inside walls and make local prediction collide with
  stale walls.

---

## 5. Maze representation and generation (`common::maze`)

### 5.1 Data structure

```rust
// What travels on the wire for a generated maze (§3.2): ~24 bytes, whatever
// the maze size. The client calls `generate(spec)` to rebuild the grid.
struct MazeSpec {
    seed: u64,
    width: u16,
    height: u16,
    braid_factor: f32,
    generator_version: u16,
}

// The grid itself. `MazeSource::Custom` carries this directly, and only an
// editor-authored maze (§7.3) ever needs to — it has no seed that could
// reproduce it.
struct MazeGrid {
    width: u16,
    height: u16,
    // one u8 per cell, bitflags: N=1, E=2, S=4, W=8 => wall present on that side
    walls: Vec<u8>,
}

// What both sides hold in memory.
struct MazeData {
    grid: MazeGrid,
    source: MazeSource,
    generator_name: String, // e.g. "recursive-backtracker+braid", for audit visibility
}
```

This single structure is used for: server-side collision, client-side
rendering (raycast targets), and the minimap (draw walls directly from the
same bitflags). One representation, no duplicated/divergent geometry
*between subsystems*.

Two consequences worth stating explicitly, because both have bitten this
kind of design before:

- **The grid is not what goes on the wire.** `walls` is one byte per cell,
  so it exceeds the ~1200-byte datagram budget past roughly 34x34 — and
  §4.4 requires each level to be larger than the last. Sending the spec
  and regenerating client-side is possible only because §5.2 guarantees
  determinism, which is the main reason that guarantee exists.
  `MazeSource::Custom` is the escape hatch for editor mazes; the server
  must refuse to load a `--maze` file whose serialized grid exceeds
  `MAX_PAYLOAD_BYTES`, with a clear error, rather than emitting a datagram
  that will fragment.
- **Every interior wall is stored twice.** The wall between cell A and its
  eastern neighbour B is A's `E` bit *and* B's `W` bit. Nothing in the
  layout enforces that the two agree, so wall symmetry is an invariant to
  be tested (§5.3, invariant 4), not a property you get for free. Any code
  that mutates walls must mutate both sides.

### 5.2 Generation algorithm

- Base algorithm: randomized recursive backtracker (produces a "perfect"
  maze — exactly one path between any two cells, i.e. maximal dead ends,
  no loops).
- **Braiding pass**: after generating the perfect maze, collect every
  *dead-end cell* (exactly 3 walls) and visit them in a seeded random
  order. For each, with probability `braid_factor`, remove one of its
  walls — chosen among the walls not leading back the way the corridor
  came — joining that dead end into a loop. Removing a wall means clearing
  both stored copies of it (§5.1).
  `braid_factor = 0.0` → pure perfect maze (hardest: one route between any
  two cells, maximal dead ends). `braid_factor` near `1.0` → almost every
  dead end is opened into a loop (easier, many routes).
- Do **not** implement this as "iterate all interior walls and remove each
  with probability `braid_factor`". At `braid_factor = 0.6` that deletes
  roughly 60% of every wall in the grid and produces an open room with
  scattered pillars — it would fail PLAN Milestone 1's own "does this look
  like a maze" spot check. Iterating dead ends also makes the knob act
  directly on the quantity §5.3 measures, instead of correlating with it.
- No "don't isolate a cell" guard is needed or possible: removing a wall
  can only ever increase connectivity.
- This gives you difficulty as a single tunable float, which is exactly
  what the level table in §4.4 uses.
- Determinism requirement: `generate(spec)` must be a pure function —
  the same `MazeSpec` always produces byte-identical walls, on any machine
  and any build. This is required for testability (§8.1), and it is what
  makes `MazeSource::Generated` possible at all: the wire carries the spec
  and both sides regenerate, so a non-deterministic generator does not
  produce a flaky test, it produces two players walking around different
  mazes. Bump `GENERATOR_VERSION` on any change that alters output.

### 5.3 Invariants a generated maze must satisfy (tested, not assumed)

1. Fully connected: every open cell reachable from every other open cell
   (flood fill from any cell reaches all cells).
2. No isolated cells (a cell walled on all 4 sides is a bug, not a
   "dead end").
3. Border cells have their outer-facing walls set (players can't walk off
   the grid).
4. **Wall symmetry**: for every interior edge the two cells sharing it
   agree — for B east of A, A's `E` bit equals B's `W` bit, and likewise
   for N/S. This is the invariant that makes §5.1's "one representation"
   claim true. Violate it on one edge and the server's `resolve_move`
   (reading the cell being left) lets a player walk through a wall the
   client's raycaster (reading the neighbour) draws as solid — a bug that
   looks like desync or cheating and is neither.
5. **Difficulty is a density, not a count**: as `braid_factor` decreases
   across the level table, dead ends *per open cell* strictly increase.
   Asserting on the raw dead-end count is worthless here, because §4.4
   also grows the grid at every level and a larger maze has more dead ends
   regardless of what braiding did — such an assertion passes with the
   braiding pass deleted entirely, which is exactly the failure mode §8.6
   forbids. Test the knob with grid size held fixed, then separately
   assert the density ordering across the real level table.

---

## 6. Client architecture

### 6.1 Startup sequence (must match the audit exactly)

1. CLI prompt: `Enter IP Address:` → parse as `SocketAddr` (reject and
   re-prompt on parse failure, don't panic).
2. CLI prompt: `Enter Name:` → non-empty, reasonable length cap.
3. Print `Starting...`, attempt connect handshake: send `Join`, wait for
   `Welcome` or `Rejected`, resending after `JOIN_RETRY_BASE_MS` and
   doubling that wait each time, up to `JOIN_MAX_ATTEMPTS` (§9 — the
   reliability-layer retry constants belong to the event channel and are
   not these). A `Rejected` is final: print its reason and exit non-zero
   without retrying. Exhausting the attempts prints a clear failure
   message naming the address tried, and exits non-zero.
4. On `Welcome`, open the GUI window and enter the main loop.

This flow must work standalone via plain stdin prompts — the GUI launcher
(bonus, §7.4) is an alternative entry point, not a replacement for this one.

### 6.2 Main loop responsibilities, per frame

1. Poll local input (keyboard/mouse). Accumulate elapsed time and, for
   every whole `INPUT_DT_MS` elapsed, emit exactly one `Input`: send it to
   the server *and* advance local prediction by that same single step
   (§6.3). Facing follows the mouse every frame — it is local-only and
   costs nothing — but position advances only on input steps. Stepping
   prediction once per rendered frame instead would integrate 120 steps
   per second against the server's 30, so the predicted position would run
   roughly 4x ahead and every reconciliation would snap the player back.
2. Drain any newly-arrived `WorldState`/`Event` messages from the network
   thread's channel (non-blocking).
3. Update interpolation buffers for other players (§6.4).
4. Render: raycast view, minimap, HUD/fps (§6.5).
5. Compute and record frame time for the fps counter.

Network receive must never block this loop. The socket read happens on a
dedicated thread that only pushes parsed messages into a channel; the main
loop only ever does non-blocking channel `try_recv`.

### 6.3 Client-side prediction (own player)

- On each local input, immediately move the locally-predicted player
  position using the *same* `common::sim::resolve_move` function the
  server uses, and render from that predicted position. Do not wait for
  the server round-trip to move the camera — this is what makes "camera
  updates when moving" and "feels smooth" pass.
- Keep a history buffer of `(input_tick, input, predicted_pos_after)`,
  one entry per emitted input, sized for at least
  `CLIENT_INPUT_HZ * MAX_RTT_S` entries.
- Reconciliation: a `WorldState` reports `last_input_tick` for self — the
  newest input the server had actually consumed when it built that
  snapshot. Look up *that* entry in the history buffer and compare its
  stored prediction against the authoritative position. Comparing against
  the newest prediction instead is the classic failure: the newest
  prediction includes inputs the server has not seen yet, so a player
  simply holding a direction is "corrected" toward a position one RTT
  stale on every snapshot and rubber-bands permanently. This is also why
  `PlayerSnapshot` carries `last_input_tick` at all — without it the
  client cannot tell which prediction a correction refers to.
- On divergence beyond epsilon: reset to the authoritative position, then
  **replay** every buffered input newer than `last_input_tick` through
  `resolve_move` to recompute the present position, and blend the camera
  toward that corrected result over a few frames to avoid a visible pop.
  Discard history entries at or below `last_input_tick`.

### 6.4 Remote player interpolation

- Never render other players at their most-recently-received raw
  position (that is choppy at 30Hz network rate rendered at 60+fps).
- **Establish a clock mapping first.** Snapshots are stamped with a server
  `tick`, but the render target is a local wall-clock time, so you need
  the server's clock expressed locally. `Welcome` and every `Pong` carry
  `(server_tick, server_time_ms)`; with the RTT measured from the matching
  `Ping`, estimate `offset = server_time_ms + rtt/2 - local_now_ms` and
  keep a smoothed value (e.g. the minimum over recent samples, slewed
  toward changes) rather than the latest raw one, which would make the
  render target jitter. Convert ticks as `tick * 1000 / SERVER_TICK_HZ`.
  Do **not** anchor to "the first tick I received": the queueing delay on
  that single packet becomes a permanent constant error, and drift then
  slides the render target out of the buffer entirely.
- Keep a snapshot buffer per remote player long enough to actually bracket
  the render time: at least
  `ceil(INTERP_DELAY_MS / (1000 / SERVER_TICK_HZ)) + 2` entries —
  `SNAPSHOT_BUFFER_LEN = 5` with the §9 values, not 2. At 30Hz snapshots
  are 33ms apart, so rendering 100ms behind needs the 4th and 5th most
  recent; a 2-entry buffer spans only 33ms, its oldest entry is always
  newer than the render target, and every frame falls through to
  extrapolation — producing exactly the choppiness this section exists to
  prevent.
- Render at `now - INTERP_DELAY_MS`, linearly interpolating position and
  (angle-wrapped) facing between the two buffered snapshots bracketing
  that time.
- If no newer snapshot has arrived when needed, extrapolate briefly from
  velocity, capped to a max extrapolation window, then hold last-known
  position rather than guessing indefinitely.
- This is the single most important piece for the "feels smooth
  independent of the displayed fps" audit item — get this working before
  polishing visuals.

### 6.5 Rendering

- **First-person view**: raycasting against `MazeData.grid.walls` (DDA
  algorithm), one ray per screen column, vertical strip height inverse to
  distance, flat-shaded/wireframe style to match the original's vector
  look (no textures needed). Depth buffer per column reused to occlude
  player sprites correctly.
- **Player sprites**: simple flat polygon/wireframe billboards, scaled by
  distance, drawn only for players within line-of-sight per the same
  depth buffer.
- **Minimap**: separate small orthographic top-down viewport, drawn every
  frame from the same `MazeData`, with a marker for self (distinct color)
  and one marker per known remote player at their interpolated position.
- **HUD**: numeric FPS (rolling average over last 30–60 frames, not
  instantaneous 1/dt — instantaneous readings are noisy and look
  unconvincing on stream/demo), health, kill feed from `Event`s.

---

## 7. Bonus features — architecture notes

Keep these decoupled enough that skipping any one of them doesn't block the
core game.

### 7.1 Procedural levels
Already covered by §5 — no extra architecture, just make sure the
generator name/seed is surfaced somewhere visible (log line or HUD) so it's
unambiguous during grading that generation is algorithmic.

### 7.2 AI bots (`server::bots`)
- A bot is implemented server-side as a `PlayerState` whose `Input` each
  tick is produced by a simple controller instead of a network message:
  wall-following or A* toward the nearest visible human player.
- Bots should go through the *exact same* `common::sim::resolve_move` /
  `raycast_hit` path as real players — no special-cased "bot movement"
  function. This guarantees bots can't diverge from real physics and
  doubles as a stress-test tool for the 10-connection/3-minute audit item.
- Toggle via server CLI flag, e.g. `--bots 8`. Bots hold real player slots
  (§4.2) and appear in `WorldState` like anyone else, so a `--bots N` with
  `N` at or above `config.max_players` must be refused at startup rather
  than quietly leaving no room for the human clients the audit needs.

### 7.3 Maze editor (`client::editor`)
- Separate mode: a grid painter GUI operating directly on a `MazeGrid`
  in memory (toggle walls per cell edge with clicks), exports/imports as
  RON or JSON via `serde`. The server should be able to load a grid from a
  file path at startup (`--maze path.ron`) instead of generating one, as
  the integration point between editor output and the game; it travels as
  `MazeSource::Custom` (§3.2).
- Two rules the editor must not break, both of them the reason the
  invariants in §5.3 exist: a click that toggles an edge writes **both**
  cells' bits (invariant 4), and a loaded file is validated against the
  full §5.3 invariant list — plus the `MAX_PAYLOAD_BYTES` limit from §5.1
  — and rejected with a clear message rather than served to clients.

### 7.4 Host history / GUI launcher (`client::launcher`)
- A small egui screen shown before the connect handshake, backed by a
  local `~/.maze_wars/hosts.json`: list of `{alias, ip, port}`, add/edit/
  connect. This is additive to, not a replacement for, the CLI prompt flow
  in §6.1 (audit explicitly tests the CLI flow).

---

## 8. Testing strategy

**Principle governing every test in this project:** a test must be able to
*fail* on a plausible buggy implementation. If you can't describe a bug the
test would catch, delete the test — it's decoration, not verification.
Concretely, avoid: asserting a struct has the field you just set (tests the
compiler, not you), asserting a function returns `Ok` without checking the
value, snapshot tests with no explanation of what invariant the snapshot
encodes.

### 8.1 `common` — pure unit/property tests (fastest, most numerous, run on every change)

- **Protocol round-trip**: for every `ClientMsg`/`ServerMsg` variant,
  `deserialize(serialize(x)) == x`. Catches enum layout / serde attribute
  regressions.
- **Payload budget, per variant**: serialize one instance of every message
  type — including `Welcome` and `LevelChanged`, which are the two that
  historically carried a whole maze — and assert each is under
  `MAX_PAYLOAD_BYTES`. Build `WorldState` with `MAX_PLAYERS` players, not
  10. A budget test that covers only `WorldState` exempts precisely the
  messages most likely to blow it.
- **Hostile payload**: feed the deserializer a hand-built buffer declaring
  an enormous `String`/`Vec` length, and assert it returns an error
  promptly without a large allocation. The socket accepts arbitrary source
  IPs (§1.6), so this is a reachable path, not a hypothetical.
- **Maze connectivity** (property test, e.g. via `proptest`, over random
  seeds/sizes/braid factors): flood fill reaches every open cell; no cell
  has all 4 walls set; border walls are always present. This is the test
  most likely to catch a real generation bug (an off-by-one in the
  backtracker leaving a pocket unreachable).
- **Wall symmetry** (same property test, §5.3 invariant 4): for every
  interior edge, both cells agree about it. Cheap to check, and the only
  thing standing between a one-sided wall edit and a player walking
  through a wall that is drawn solid.
- **Maze difficulty ordering** (§5.3 invariant 5): with grid size held
  **fixed**, generate across the level table's `braid_factor` values and
  assert dead ends per open cell strictly increases as the factor drops;
  then assert the same density ordering across the real level table, where
  size varies too. Asserting on raw dead-end counts across the real table
  would pass with the braiding pass deleted, since area grows every level
  — see §8.6.
- **Determinism**: same seed + params → byte-identical `MazeData` across
  two calls. Catches accidental use of a non-seeded RNG source.
- **Collision resolution** (`resolve_move`): test with hand-built tiny
  mazes (e.g. a single walled corridor, a corner) and known start
  position + movement vector; assert the resolved position does not
  cross a wall and does not get stuck ("sliding" along a wall when moving
  diagonally into it should still produce forward progress, not a full
  stop — write a test that would fail if someone implements naive
  full-stop-on-any-collision).
- **Raycast hit resolution**: construct scenes with a wall closer than a
  player, a player closer than a wall, and a clean miss; assert the
  correct target/kind is returned and the distance matches expectation
  within epsilon.
- **Interpolation math**: given a snapshot buffer and a query time
  between/after/far-after the buffered range, assert correct interpolated,
  extrapolated (bounded), and held-last-known outputs respectively —
  three distinct test cases, not one happy path. Drive it with a buffer
  filled at `SERVER_TICK_HZ` and queried at `now - INTERP_DELAY_MS`, and
  assert the *interpolated* branch is the one taken: with a too-short
  buffer every query silently degrades to extrapolation, which no
  three-case test written against hand-picked timestamps would notice.
- **Reconciliation**: build a prediction history, feed a snapshot whose
  `last_input_tick` is several inputs behind the newest prediction, and
  assert (a) the comparison is made against the matching history entry,
  and (b) after replaying the newer inputs the corrected position equals
  the pre-correction prediction when the server agreed. A reconciliation
  that compares against the newest prediction fails (b) with a non-zero
  correction every tick — that is the rubber-banding bug, and this is the
  test that catches it without a live server.

### 8.2 Reliability layer — simulated network tests (no real sockets)

- Build an in-memory fake channel that can be configured to drop N%,
  duplicate N%, and reorder messages, driven by a seeded RNG for
  reproducibility.
- Test: under 30% simulated loss, every sent event is delivered at least
  once within `MAX_RETRY_ATTEMPTS * RETRY_INTERVAL_MS`. Note this is a
  probabilistic claim, not an absolute one — §3.3 gives up after
  `MAX_RETRY_ATTEMPTS` — so drive it from a fixed set of seeds and assert
  full delivery for each. Do not write it as an unbounded "eventually":
  that test is either flaky or lying. The complementary behaviour (a
  permanently dropped event must not corrupt client state) is covered by
  the membership-reconciliation path in §3.3, tested in §8.4.
- Test: under simulated duplication, the receiver-side application
  callback fires **exactly once** per logical event (this is the test
  that actually catches a missing dedupe-by-`event_id` bug).
- Test: under reordering, out-of-order state messages don't cause a
  stale position to overwrite a newer one (assert on `tick` ordering
  logic specifically, not just "no crash").

### 8.3 Server integration tests (real UDP sockets over loopback, `server/tests/`)

- Spin up a real server instance in a test process with a `Config`
  (§9) whose `bind_addr` is `127.0.0.1:0` (OS-assigned port), a small
  `max_players`, and a short `client_timeout_ms`; drive it with real
  `UdpSocket`s standing in for clients. Production binds `0.0.0.0:PORT`
  (§1.6) — that difference is exactly why these are config fields and not
  constants.
- **Capacity test**: connect `config.max_players` clients successfully,
  assert the next one gets `Rejected`, and assert the already-connected
  clients keep receiving `WorldState` throughout. This test is worthless
  if it only checks "connect() didn't error" — assert on the actual
  message content received.
- **Level-epoch test**: force a level change, then assert the client
  discards `WorldState`s stamped with an epoch it has no maze for.
  Simulate the real race by dropping the `LevelChanged` datagram once and
  confirming no snapshot is applied against the old maze before the retry
  lands.
- **Timeout test**: connect a client, stop sending anything, assert a
  `PlayerLeft` event is broadcast to a second, still-connected client
  within a bounded time window after the configured timeout.
- **Duplicate-name / bad protocol version**: assert `Rejected` with the
  expected reason, not just "not `Welcome`".
- **Tick-rate-under-load test**: connect 10+ fake clients (can reuse the
  bot input generator), run for a fixed wall-clock duration, measure
  actual achieved server tick rate from timestamps in outgoing
  `WorldState` messages, assert it stays within tolerance of
  `SERVER_TICK_HZ`. This is the automated proxy for the manual "10
  clients for 3 minutes, fps stays >50" audit item — run it in CI on
  every change, don't discover a regression only during a live demo.
- **Input-queue test**: deliver two inputs for one player inside a single
  server tick and assert the player moves two steps, not one. Then deliver
  `MAX_INPUT_QUEUE + 5` at once and assert movement is capped. Then send
  an input whose `move_dir` has magnitude 100 and assert displacement
  matches a unit direction. Each of these is a distinct plausible bug:
  dropping queued inputs, unbounded catch-up, and client-controlled speed.

### 8.4 Client tests

- Rendering itself (raycasting output, GUI) is not meaningfully unit
  testable — don't write fake tests around pixel output. Instead:
  - Unit test the **raycast math** (`render::raycast`'s distance/DDA
    computation) as a pure function against hand-computed expected
    distances for known ray angles into a known tiny maze — this is
    exactly the kind of thing that silently renders "plausible but
    wrong" without a test.
  - Unit test the **fps averaging** function directly: feed it a
    synthetic sequence of frame times (including a deliberate spike)
    and assert the reported value is the rolling average, not the
    instantaneous last value — this test would fail on the naive
    `1.0/dt` implementation.
  - Integration test client `net` against a minimal fake server (a
    test-only UDP echo/scripted responder) to verify the connect
    handshake retry/backoff/failure-reporting behaviour without needing
    the real server binary. Assert against the §9 constants
    (`JOIN_RETRY_BASE_MS` doubling, `JOIN_MAX_ATTEMPTS`), not against
    numbers written into the test.
  - Test the **membership-reconciliation** path from §3.3 directly: feed
    the client a `WorldState` containing a `PlayerId` it never saw a
    `PlayerJoined` for, and assert it renders that player with a
    placeholder name instead of panicking or dropping them; then stop
    including a known player and assert they are removed after
    `GHOST_TIMEOUT_MS` even though no `PlayerLeft` ever arrives.

### 8.5 Manual / demo-day checklist (not automatable, but scripted so it's repeatable)

Keep a `scripts/manual_check.md` (or reuse the audit list itself) and run
it, in full, before submission:
- Fresh clone, `cargo clean && cargo build` on both binaries — confirm
  zero warnings in the raw output (grep for `warning:` and assert none).
- Full CLI-prompt-to-GUI flow on one machine.
- Second local client joins, both visible to each other, minimap and
  camera update live for both.
- Bot-assisted 10+-connection, 3-minute soak test with FPS logged to a
  file the whole time (don't rely on eyeballing a live counter for your
  own evidence trail).

### 8.6 What NOT to do

- Do not write a test that asserts a getter returns what a setter just
  set.
- Do not write a test that mocks out the exact function under test and
  then asserts the mock was called — that tests the test, not the code.
- Do not skip the maze-connectivity property test in favor of "eyeballing
  a printed maze" — this is precisely the kind of bug (rare seed producing
  an isolated pocket) that only shows up under many random trials.
- Do not consider the tick-rate-under-load test optional — it is the only
  automated stand-in for the single riskiest audit item (fps under
  10-client load), and it is cheap to run in CI once bots exist.

---

## 9. Configuration constants (single source of truth)

Put these in `common::config` so server and client cannot drift apart:

```rust
pub const PROTOCOL_VERSION: u16 = 1;
pub const GENERATOR_VERSION: u16 = 1;   // bump on ANY change to maze generation

pub const SERVER_TICK_HZ: u16 = 30;
pub const CLIENT_INPUT_HZ: u16 = 30;
pub const INPUT_DT_MS: f32 = 1000.0 / CLIENT_INPUT_HZ as f32;
pub const PLAYER_SPEED_UPS: f32 = 3.0;  // cells/sec; the SERVER owns speed, not the client
pub const MAX_INPUT_QUEUE: usize = 6;   // input steps consumed per player per tick (§4.3)
pub const MAX_PLAYERS: usize = 20;      // 10 humans + 8 bots + headroom; bots take slots (§4.2)
pub const CLIENT_TIMEOUT_MS: u64 = 5000;
pub const GHOST_TIMEOUT_MS: u64 = 2000; // drop a known player absent from snapshots (§3.3)
pub const RETRY_INTERVAL_MS: u64 = 100; // event channel only
pub const MAX_RETRY_ATTEMPTS: u32 = 10; // event channel only
pub const JOIN_RETRY_BASE_MS: u64 = 250; // handshake only (§6.1); doubles each attempt
pub const JOIN_MAX_ATTEMPTS: u32 = 6;    // ~15s total before giving up
pub const INTERP_DELAY_MS: f32 = 100.0;
pub const MAX_EXTRAPOLATION_MS: f32 = 250.0;
pub const SNAPSHOT_BUFFER_LEN: usize = 5; // >= ceil(INTERP_DELAY_MS / tick_ms) + 2 (§6.4)
pub const MAX_RTT_S: f32 = 1.0;           // sizes the prediction history buffer (§6.3)
pub const FPS_AVG_WINDOW_FRAMES: usize = 60;
pub const MAX_PAYLOAD_BYTES: usize = 1200; // recv buffer AND the deserializer's read limit
```

Use these names exactly as written wherever the doc refers to them — an
earlier draft of this file said `TIMEOUT_MS` and `MAX_ATTEMPTS` in prose
while declaring `CLIENT_TIMEOUT_MS` and `MAX_RETRY_ATTEMPTS` here, which
is how a "single source of truth" quietly becomes two.

Any implementation or test that hardcodes one of these values inline
instead of importing the constant is a bug waiting to happen when the
value is tuned later.

### 9.1 What must be runtime-configurable

Several of the above cannot be `pub const` at the point of use, because
the tests in §8.3 need to lower them: an idle-timeout test that waits a
real 5 seconds, a capacity test that must open 20 sockets, and a bind
address that is `0.0.0.0:PORT` in production but `127.0.0.1:0` under test.
A `const` cannot be overridden per test, so those live in a struct whose
`Default` is built from the constants above, and the server reads the
struct — never the constant — at every use site:

```rust
pub struct Config {
    pub bind_addr: SocketAddr,      // 0.0.0.0:PORT prod, 127.0.0.1:0 in tests
    pub tick_hz: u16,
    pub max_players: usize,
    pub client_timeout_ms: u64,
    pub retry_interval_ms: u64,
    pub max_retry_attempts: u32,
}
```

The purely client-side tuning values (`INTERP_DELAY_MS`,
`SNAPSHOT_BUFFER_LEN`, `FPS_AVG_WINDOW_FRAMES`, …) can stay constants —
nothing needs to vary them per test.
