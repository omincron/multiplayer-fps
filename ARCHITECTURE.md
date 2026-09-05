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
│   │   └── types.rs           # PlayerState, ProjectileState, Vec2, PlayerId, etc.
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
    Join { name: String, protocol_version: u16 },
    Input { client_tick: Tick, move_dir: Vec2, facing: f32, shoot: bool },
    Ack { event_id: u32 },        // reliability layer ack
    Ping { nonce: u32 },
    Leave,
}

enum ServerMsg {
    Welcome { player_id: PlayerId, level: u8, maze: MazeData, tick_rate_hz: u16 },
    Rejected { reason: String },              // e.g. server full, name taken
    WorldState {
        tick: Tick,
        players: Vec<PlayerSnapshot>,
        projectiles: Vec<ProjectileSnapshot>,
    },
    Event { event_id: u32, kind: EventKind }, // reliable channel
    Pong { nonce: u32 },
}

enum EventKind {
    PlayerJoined { id: PlayerId, name: String },
    PlayerLeft { id: PlayerId },
    Hit { shooter: PlayerId, target: PlayerId, target_hp: u8 },
    Killed { victim: PlayerId, killer: PlayerId },
    Respawned { id: PlayerId, pos: Vec2 },
    LevelChanged { level: u8, maze: MazeData },
}

struct PlayerSnapshot { id: PlayerId, pos: Vec2, facing: f32, hp: u8 }
struct ProjectileSnapshot { id: u32, pos: Vec2, dir: f32 }
```

Contract:
- `protocol_version` must be checked by the server on `Join`; mismatch ->
  `Rejected`. This exists so future changes fail loudly instead of silently
  desyncing.
- Serialization is `bincode` via `serde`. Every message type must round-trip
  through `serialize -> deserialize` byte-identically in a unit test — this
  is a real correctness requirement (see §8), not busywork: a subtle
  `#[repr]`/enum-ordering change is exactly the kind of bug this catches.
- Max UDP payload assumed: keep messages under ~1200 bytes to avoid
  fragmentation on typical MTUs. `WorldState` must be sized against player
  count — with 10 players this needs an actual byte-size check, not an
  assumption (see test list in §8).

### 3.3 Reliability layer (`common::reliability`)

For `Event` messages only:
- Sender keeps a table of `event_id -> (payload, last_sent_at, attempts)`.
- Resends unacked events every `RETRY_INTERVAL_MS` (e.g. 100ms) up to
  `MAX_ATTEMPTS` (e.g. 10), then gives up and logs (do not panic).
- Receiver keeps a small ring/set of recently-seen `event_id`s and drops
  duplicates before handing them to game logic. This makes event handling
  idempotent at the call site — game logic never has to think about
  duplicate delivery.
- This layer must be tested with a **simulated lossy/duplicating channel**
  (an in-memory fake, not real sockets) — see §8.2.

### 3.4 Tick / rate model

- Server simulates at a fixed tick rate, `SERVER_TICK_HZ = 30` (configurable
  constant). One `WorldState` broadcast per tick.
- Client sends `Input` at `CLIENT_INPUT_HZ = 30`.
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

- On `Join`: if under capacity (`MAX_PLAYERS = 10` minimum, make it a
  config constant so it can be raised) and name not already in use, assign a
  new `PlayerId`, insert into `world`, reply `Welcome`, broadcast
  `PlayerJoined` event to everyone else.
- If at capacity: reply `Rejected { reason: "server full" }`. This must be
  a real, tested path (§8.3) — "accepts 10, cleanly rejects the 11th" is an
  explicit design requirement even though the audit only requires 10 to
  succeed.
- **Timeout**: if no `Input`/`Ping` received from a player for
  `TIMEOUT_MS` (e.g. 5000ms), drop them, broadcast `PlayerLeft`. Prevents
  dead connections from silently eating a capacity slot.

### 4.3 Authoritative simulation

- `World` owns all `PlayerState`s and `ProjectileState`s and the current
  `MazeData`.
- Movement: apply each player's latest received input, resolve against
  maze walls via `common::sim::resolve_move` (circle-vs-grid, pure
  function, heavily unit tested — see §8.1).
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
  this specific parameter is the difficulty knob).
- Advancing a level regenerates the maze (new random seed), broadcasts
  `LevelChanged`, and resets player positions to valid spawn cells.

---

## 5. Maze representation and generation (`common::maze`)

### 5.1 Data structure

```rust
struct MazeData {
    width: u16,
    height: u16,
    // one u8 per cell, bitflags: N=1, E=2, S=4, W=8 => wall present on that side
    walls: Vec<u8>,
    seed: u64,
    generator_name: String, // e.g. "recursive-backtracker+braid", for audit visibility
}
```

This single structure is used for: server-side collision, client-side
rendering (raycast targets), and the minimap (draw walls directly from the
same bitflags). One representation, no duplicated/divergent geometry.

### 5.2 Generation algorithm

- Base algorithm: randomized recursive backtracker (produces a "perfect"
  maze — exactly one path between any two cells, i.e. maximal dead ends,
  no loops).
- **Braiding pass**: after generating the perfect maze, iterate all
  interior walls; for each, with probability `braid_factor` knock it down
  (creating a loop) if doing so doesn't leave an isolated 1x1 pocket.
  `braid_factor = 0.0` → pure perfect maze (hardest, only one route, many
  dead ends). `braid_factor` closer to `1.0` → many loops (easier, multiple
  routes, fewer forced dead ends).
- This gives you difficulty as a single tunable float, which is exactly
  what the level table in §4.4 uses.
- Determinism requirement: `generate(seed, width, height, braid_factor)`
  must be a pure function — same seed always produces the same maze. This
  is required for testability (§8.1) and is also why we store `seed` in
  `MazeData` rather than only the resulting `walls`.

### 5.3 Invariants a generated maze must satisfy (tested, not assumed)

1. Fully connected: every open cell reachable from every other open cell
   (flood fill from any cell reaches all cells).
2. No isolated cells (a cell walled on all 4 sides is a bug, not a
   "dead end").
3. Border cells have their outer-facing walls set (players can't walk off
   the grid).
4. As `braid_factor` decreases across the level table, the *measured* dead
   end count (cells with exactly 3 walls) must strictly increase, and
   average solution-path branching must decrease. This is the actual test
   of "harder", not just "the number is smaller" (see §8.1).

---

## 6. Client architecture

### 6.1 Startup sequence (must match the audit exactly)

1. CLI prompt: `Enter IP Address:` → parse as `SocketAddr` (reject and
   re-prompt on parse failure, don't panic).
2. CLI prompt: `Enter Name:` → non-empty, reasonable length cap.
3. Print `Starting...`, attempt connect handshake (`Join` → wait for
   `Welcome` or `Rejected`, with retry/backoff and a hard timeout →
   print a clear failure message and exit non-zero if it never connects).
4. On `Welcome`, open the GUI window and enter the main loop.

This flow must work standalone via plain stdin prompts — the GUI launcher
(bonus, §7.4) is an alternative entry point, not a replacement for this one.

### 6.2 Main loop responsibilities, per frame

1. Poll local input (keyboard/mouse) → update locally-predicted own
   position immediately (§6.3) → send `Input` to server at
   `CLIENT_INPUT_HZ` (not necessarily every render frame).
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
- Reconciliation: when a `WorldState` arrives with the server's
  authoritative position for `self`, compare against the predicted
  position at that tick (keep a short history buffer keyed by
  `client_tick`). If they diverge beyond a small epsilon, snap/blend
  toward the authoritative position rather than teleporting instantly
  (blend over a few frames to avoid visible pops).

### 6.4 Remote player interpolation

- Never render other players at their most-recently-received raw
  position (that is choppy at 30Hz network rate rendered at 60+fps).
- Keep the last 2 (or more) received snapshots per remote player with
  their tick timestamps. Render at `now - INTERP_DELAY_MS` (e.g. 100ms
  behind), linearly interpolating position/facing between the two
  snapshots that bracket that render time.
- If no newer snapshot has arrived when needed, extrapolate briefly from
  velocity, capped to a max extrapolation window, then hold last-known
  position rather than guessing indefinitely.
- This is the single most important piece for the "feels smooth
  independent of the displayed fps" audit item — get this working before
  polishing visuals.

### 6.5 Rendering

- **First-person view**: raycasting against `MazeData.walls` (DDA
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
- Toggle via server CLI flag, e.g. `--bots 8`.

### 7.3 Maze editor (`client::editor`)
- Separate mode: a grid painter GUI operating directly on a `MazeData`
  in memory (toggle walls per cell edge with clicks), exports/imports as
  RON or JSON via `serde`. The server should be able to load a
  `MazeData` from a file path at startup (`--maze path.ron`) instead of
  generating one, as the integration point between editor output and the
  game.

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
- **Maze connectivity** (property test, e.g. via `proptest`, over random
  seeds/sizes/braid factors): flood fill reaches every open cell; no cell
  has all 4 walls set; border walls are always present. This is the test
  most likely to catch a real generation bug (an off-by-one in the
  backtracker leaving a pocket unreachable).
- **Maze difficulty ordering**: generate one maze per level-table entry
  with the actual configured parameters; assert dead-end count is
  monotonically non-decreasing and average branching factor is
  monotonically non-increasing across levels. This directly tests the
  audit's "increasing difficulty" requirement instead of trusting the
  parameter choice.
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
- **Interpolation math**: given two timestamped snapshots and a query
  time between/before/after them, assert correct interpolated,
  extrapolated (bounded), and held-last-known outputs respectively —
  three distinct test cases, not one happy path.

### 8.2 Reliability layer — simulated network tests (no real sockets)

- Build an in-memory fake channel that can be configured to drop N%,
  duplicate N%, and reorder messages, driven by a seeded RNG for
  reproducibility.
- Test: under 30% simulated loss, every sent event is eventually
  delivered at least once within `MAX_ATTEMPTS * RETRY_INTERVAL_MS`.
- Test: under simulated duplication, the receiver-side application
  callback fires **exactly once** per logical event (this is the test
  that actually catches a missing dedupe-by-`event_id` bug).
- Test: under reordering, out-of-order state messages don't cause a
  stale position to overwrite a newer one (assert on `tick` ordering
  logic specifically, not just "no crash").

### 8.3 Server integration tests (real UDP sockets over loopback, `server/tests/`)

- Spin up a real server instance bound to `127.0.0.1:0` (OS-assigned
  port) in a test process, drive it with real `UdpSocket`s standing in
  for clients.
- **Capacity test**: connect `MAX_PLAYERS` clients successfully, assert
  the `MAX_PLAYERS + 1`th gets `Rejected`. This test is worthless if it
  only checks "connect() didn't error" — assert on the actual message
  content received.
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
    handshake retry/timeout/failure-reporting behavior without needing
    the real server binary.

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
pub const SERVER_TICK_HZ: u16 = 30;
pub const CLIENT_INPUT_HZ: u16 = 30;
pub const MAX_PLAYERS: usize = 16;      // >= 10 required; headroom on top
pub const CLIENT_TIMEOUT_MS: u64 = 5000;
pub const RETRY_INTERVAL_MS: u64 = 100;
pub const MAX_RETRY_ATTEMPTS: u32 = 10;
pub const INTERP_DELAY_MS: f32 = 100.0;
pub const MAX_EXTRAPOLATION_MS: f32 = 250.0;
pub const FPS_AVG_WINDOW_FRAMES: usize = 60;
```

Any implementation or test that hardcodes one of these values inline
instead of importing the constant is a bug waiting to happen when the
value is tuned later.
