# Plan — Multiplayer Maze FPS

This is an execution plan, not a design doc — see `ARCHITECTURE.md` for the
"what/why". This file is the "in what order, and how do I know each step
actually works" doc.

**How to use this file (agent instructions):**
- Work through milestones in order. Each milestone lists: what to build,
  which tests to write **first or alongside** (not after, and not skipped),
  and an explicit gate you must pass before starting the next milestone.
- "Gate" means: run the stated commands, confirm the stated outcome, and
  only then move on. Do not mark a milestone done because the code compiles
  — compiling is not the gate, the tests and the stated manual checks are.
- If a test would pass trivially against a stub/no-op implementation,
  it is not done — rewrite it so it can fail. Re-read §8.6 of
  `ARCHITECTURE.md` before writing any test.
- When a milestone's tests reveal a design problem, fix `ARCHITECTURE.md`
  first, then the code, then the test if needed — keep the doc and the
  code from drifting apart.
- Re-run `cargo clean && cargo build` (not just `cargo build`) and confirm
  **zero warnings** at the end of every milestone, not just at the end of
  the project. Warnings compound and get harder to untangle later.

---

## Milestone 0 — Workspace skeleton

**Build:**
- Create the workspace exactly as laid out in `ARCHITECTURE.md` §2:
  `common`, `server`, `client` crates, workspace `Cargo.toml`.
- `common`: add `serde`, `bincode` (or `postcard`), `proptest` (dev-dep).
- `server`: add `tokio` or plain `std::net` (pick one and note the choice
  in `ARCHITECTURE.md` if you deviate — don't mix async and blocking
  socket code in the same crate).
- `client`: add `macroquad` (or `ggez`), `egui`/`egui-macroquad` if using
  macroquad.
- Stub `main.rs` in both `server` and `client` that just prints a message
  and exits.

**Tests:**
- None yet — there is no logic to test. Do not write placeholder tests.

**Gate:**
- `cargo clean && cargo build --workspace` — zero warnings, zero errors.
- `cargo run -p server` and `cargo run -p client` both run and exit
  cleanly.

---

## Milestone 1 — Maze generation (pure, in `common`)

**Build:**
- `common::maze::{MazeSpec, MazeGrid, MazeData}` and
  `generate(spec: MazeSpec) -> MazeData` per Architecture §5.1.
- Braiding operates on **dead-end cells**, not on all interior walls
  (Architecture §5.2) — the all-walls version produces an open room with
  pillars at the braid factors this project actually uses, and would fail
  this milestone's own visual gate below.

**Tests (write these before you trust the implementation — they are the
actual spec):**
- `proptest`-based connectivity test over random `(seed, width, height,
  braid_factor)` inputs (Architecture §8.1, item 2). Run at least 256
  cases in CI, more locally while iterating.
- No-isolated-cell test.
- Border-wall test.
- Wall-symmetry property test (Architecture §5.3 invariant 4): every
  interior edge is stored identically in both cells that share it. Every
  wall lives in two places, so this is the invariant that keeps the server
  and the renderer agreeing about where the walls are.
- Determinism test (same spec twice -> identical output). This one carries
  more weight than it looks: `MazeSource::Generated` sends only the spec
  and has the client regenerate, so non-determinism means the two sides
  play on different mazes.
- Dead-end **density** monotonicity (Architecture §5.3 invariant 5), in
  two parts: (a) with grid size fixed, dead ends per open cell strictly
  increases as `braid_factor` drops across at least 3 values; (b) the same
  ordering holds across the real level table, where size varies too. Do
  not assert on raw dead-end counts across the real table — area grows
  every level, so that assertion passes even with the braiding pass
  deleted. Part (b) should be **red** until you've picked real level
  parameters, which is useful signal, not a problem to hide.

**Gate:**
- `cargo test -p common maze::` all green, including the proptest run.
- Manually print one generated maze as ASCII art to the terminal and
  visually sanity-check it looks like a maze (this is a spot check, not a
  substitute for the property tests above).

---

## Milestone 2 — Protocol types + serialization (`common::protocol`)

**Build:**
- All `ClientMsg`/`ServerMsg`/`EventKind`/`MazeSource` types from
  Architecture §3.2. Note what is deliberately absent: there is no
  projectile type, because shooting is hitscan (Architecture §4.3) — don't
  reintroduce one "for later".
- Configure the (de)serializer with an explicit `MAX_PAYLOAD_BYTES` read
  limit at the same time you define the types, not as a later hardening
  pass — the socket is public and the default config pre-allocates from an
  attacker-supplied length prefix.

**Tests:**
- Round-trip test for every enum variant (loop over a hand-built list of
  one instance per variant — if you add a variant later and forget to add
  it to this list, that's a real gap, so structure the test so a missing
  variant is obvious, e.g. exhaustive `match` with no wildcard arm in the
  test's variant-list constructor).
- A size-budget test covering **every** message variant, not just
  `WorldState`: assert each serializes to under `MAX_PAYLOAD_BYTES`
  (Architecture §3.2). Build `WorldState` with `MAX_PLAYERS` players, and
  build `Welcome`/`LevelChanged` for the *largest* maze in the level
  table — those two are the ones that blow the budget the moment anyone
  puts an expanded grid back on the wire, and a `WorldState`-only test
  exempts exactly them. It should fail loudly (not silently truncate) if
  someone bumps `MAX_PLAYERS` or grows a level without reconsidering the
  wire format.
- A hostile-payload test: hand-build a short buffer declaring a huge
  `String` length for `Join.name`, assert the deserializer errors promptly
  instead of attempting the allocation.

**Gate:**
- `cargo test -p common protocol::` green.
- No `#[allow(dead_code)]` anywhere to silence unused-variant warnings —
  if a variant is genuinely unused so far, that's a warning worth seeing,
  not suppressing.

---

## Milestone 3 — Reliability layer (`common::reliability`)

**Build:**
- Sender-side retry table, receiver-side dedupe set, per Architecture §3.3.
- A small in-memory "lossy channel" test harness (seeded RNG-driven drop/
  duplicate/reorder) — this harness is test infrastructure, lives under
  `common/tests/` or a `#[cfg(test)]` module, not shipped code.

**Tests:**
- Delivery-under-loss test (Architecture §8.2, item 1). Phrase it as
  "delivered within `MAX_RETRY_ATTEMPTS * RETRY_INTERVAL_MS` for each of
  these fixed seeds", not as an unbounded "eventually" — the layer gives
  up by design, so an absolute claim is either flaky or false.
- Exactly-once-under-duplication test (§8.2 item 2) — this is the one
  most likely to catch a real bug, don't skip it.
- Ordering/staleness test under simulated reordering (§8.2 item 3).

**Gate:**
- `cargo test -p common reliability::` green across at least 3 different
  seeds for the lossy channel (run the test body in a loop over seeds, or
  parametrize it) — a single lucky seed passing is not sufficient
  evidence.

---

## Milestone 4 — Movement & collision (`common::sim`)

**Build:**
- `resolve_move(maze, pos, move_dir, speed, dt_s, radius) -> pos'` — one
  fixed simulation step of circle-vs-grid collision with wall sliding (not
  full-stop). `move_dir` is normalised to at most unit length inside the
  function; the caller supplies `speed`, never the client. The `dt_s`
  parameter is what keeps client prediction and server simulation
  integrating the same distance per second (Architecture §3.4) — a
  signature without it silently ties movement speed to frame rate.

**Tests:**
- Straight corridor: moving into an end wall stops at the wall, not past
  it, not short of it (assert within epsilon of the wall's plane).
- Diagonal-into-corner: moving diagonally into a wall corner still allows
  sliding progress along the open axis (this is the test that fails on a
  naive "cancel all movement on any collision" implementation — write it
  deliberately to catch that).
- Moving through a doorway exactly at the collision radius boundary
  (edge case most likely to reveal an off-by-epsilon bug).
- Same direction, twice the `dt_s`: assert twice the displacement (in open
  space). This is the test that fails if someone drops `dt_s` and hardcodes
  a per-call step.
- `move_dir` with magnitude 100: assert the displacement equals the
  unit-direction case. Without this, a client controls its own speed and
  "the server is authoritative" is not true (Architecture §4.3).

**Gate:**
- `cargo test -p common sim::resolve_move` green.
- No test in this file passes against a stub that just returns `pos`
  unchanged — verify this by temporarily breaking the implementation and
  confirming tests fail (a quick manual sanity check, not a permanent
  test).

---

## Milestone 5 — Shooting resolution (`common::sim::raycast_hit`)

**Build:**
- Instant-hit raycast against maze walls + player positions.

**Tests:**
- Wall-only scene: ray hits the correct wall segment at the correct
  distance.
- Player-closer-than-wall: player is reported as the hit, with correct id.
- Wall-closer-than-player: wall wins, player behind it is *not* reported
  hit (this is the occlusion-correctness test — easy to get subtly wrong).
- Clean miss (open corridor longer than any obstacle): no hit reported.

**Gate:**
- `cargo test -p common sim::raycast_hit` green, all four cases present
  and each individually failing if you comment out the relevant logic
  (spot-check at least one by breaking it on purpose).

---

## Milestone 6 — Server: socket I/O + connection lifecycle (no game loop yet)

**Build:**
- `common::config::Config` (Architecture §9.1) **first** — `bind_addr`,
  `tick_hz`, `max_players`, `client_timeout_ms`, retry values, with
  `Default` built from the §9 constants. Every use site reads the config,
  not the constant. Without this the tests below cannot be written at all:
  they need `127.0.0.1:0`, a small player cap, and a sub-second timeout.
- `server::net`: bind `config.bind_addr` (`0.0.0.0:PORT` in production,
  per audit item §1.6), recv loop, `SocketAddr <-> PlayerId` table,
  `Join`/`Welcome`/`Rejected` handling, capacity limit, duplicate name
  rejection, idle timeout, `protocol_version`/`generator_version` check.
- No movement/shooting simulation yet — players exist but don't move.

**Tests (integration, real loopback sockets, `server/tests/`):**
- Single client connects, receives `Welcome` with a valid `player_id` and
  the expected `maze`.
- Capacity test: fill `config.max_players` (set it low for the test),
  assert the next connection gets `Rejected` with a specific reason
  string, and assert the already-connected clients are unaffected.
- Duplicate-name rejection test.
- Idle-timeout test: connect, go silent, assert a still-connected second
  client receives `PlayerLeft` for the timed-out player within a bounded
  window after the configured timeout (use a short timeout constant
  override for the test, don't wait 5 real seconds if avoidable).
- Protocol-version and generator-version mismatch rejection tests
  (separate cases — the second is what stops client and server silently
  regenerating different mazes from the same seed).

**Gate:**
- `cargo test -p server` green.
- Manually run the server, connect with `netcat`-style raw UDP or a tiny
  scratch script sending a hand-crafted `Join` bincode payload, confirm a
  `Welcome` comes back — this is a real end-to-end sanity check
  independent of your own test harness's assumptions.

---

## Milestone 7 — Server: authoritative tick loop + movement

**Build:**
- `server::world::World`, fixed-timestep sim thread wired to `net` per
  Architecture §4.1, broadcasting `WorldState` each tick.
- Per-player input **queue**, drained each tick, one `resolve_move` step
  per queued input, capped at `MAX_INPUT_QUEUE` (Architecture §4.3).
  Keeping only the newest input is the tempting shortcut and it is wrong:
  network jitter routinely delivers two inputs in one tick window, and
  discarding one shortens that player's movement and triggers a
  reconciliation correction for a player who did nothing unusual.
- Echo the highest consumed `input_tick` per player as
  `PlayerSnapshot.last_input_tick` — Milestone 10 cannot reconcile
  without it, so it is not optional plumbing.

**Tests:**
- Two fake clients, one sends movement input, assert the *other* client's
  received `WorldState` reflects the moved position within one or two
  ticks (not instantly — respect the tick boundary in the assertion).
- A client sending input that would walk through a wall: assert the
  broadcast position stops at the wall (this re-validates `resolve_move`
  is actually wired in, not just unit-tested in isolation).
- Two inputs delivered inside one tick window: assert the player advances
  two steps, not one. Then `MAX_INPUT_QUEUE + 5` at once: assert movement
  is capped rather than scaling with the flood.
- `last_input_tick` echo test: send inputs 1..5, assert the snapshot
  reports the highest tick actually consumed (not the highest received,
  and not zero) — a stubbed-out field here silently disables Milestone
  10's reconciliation without failing any of its own tests.
- **Tick-rate-under-load test** (Architecture §8.3, the big one): spin up
  10+ fake clients sending input continuously for a fixed duration
  (10-30 seconds is enough for CI; save the full 3-minute run for
  pre-submission), measure achieved tick rate from `WorldState.tick`
  timestamps, assert within tolerance of `SERVER_TICK_HZ`. Treat any
  failure here as a P0 bug, not a "tune it later" item — it's the direct
  automated proxy for the highest-stakes manual audit item.

**Gate:**
- `cargo test -p server` green including the load test.
- Load test result logged (tick rate achieved vs target) somewhere you
  can point to later — don't let this be a one-time terminal glance.

---

## Milestone 8 — Client: connect flow + raw rendering window (no networking-driven movement yet)

**Build:**
- CLI prompts exactly per Architecture §6.1 (`Enter IP Address:`,
  `Enter Name:`, `Starting...`), connect handshake retrying on
  `JOIN_RETRY_BASE_MS` with doubling backoff up to `JOIN_MAX_ATTEMPTS`
  (§9 — these are the handshake's own constants; the reliability layer's
  retry constants belong to the event channel and are not reusable here).
  A `Rejected` is final: print the reason, exit non-zero, do not retry.
- Open a macroquad/ggez window on success, render *something* (even just
  a blank frame + fps counter) to prove the pipeline works end to end.
- fps counter using the rolling-average function from Architecture §6.5 —
  build and unit-test this function now, even though it has nothing
  interesting to show yet.

**Tests:**
- Unit test the fps-averaging function per Architecture §8.4: feed a
  synthetic frame-time sequence with a deliberate spike, assert the
  reported value is the rolling average, not the last instantaneous
  value.
- Integration test the connect handshake against a scripted fake
  responder (per Architecture §8.4): assert the retry count and backoff
  timing against `JOIN_MAX_ATTEMPTS` / `JOIN_RETRY_BASE_MS` (import them;
  do not write the numbers into the test), a clear failure message and
  non-zero exit when attempts are exhausted, the success path when a
  `Welcome` arrives after 1-2 dropped attempts, and immediate exit without
  further retries on `Rejected`.

**Gate:**
- `cargo test -p client` green.
- Manual run: `cargo run -p client`, follow the prompts against a running
  `server` from Milestone 7, confirm connection succeeds and a window
  opens showing a live-updating fps number.

---

## Milestone 9 — Client: raycasting renderer against a static maze

**Build:**
- `client::render::raycast`: DDA raycasting against the `MazeData` the
  client builds from the `MazeSource` in `Welcome` (regenerating it
  locally for `Generated`, using the grid directly for `Custom`), one
  vertical strip per screen column, flat-shaded walls.
- Assert the §5.3 invariants on the maze the client just built, at build
  time. It is cheap, and it turns "the two sides generated different
  mazes" into an immediate loud failure instead of a confusing rendering
  bug two milestones later. No other players yet, no movement yet — just look around a
  static maze from a fixed or keyboard-controlled camera.

**Tests:**
- Unit test the raycast distance/DDA math directly (Architecture §8.4):
  hand-build a tiny maze (e.g. 3x3 with one known wall), compute expected
  hit distance for a few specific ray angles by hand, assert the function
  matches within epsilon. Do this before trusting what's on screen — a
  raycaster can look "plausible" while being subtly wrong (e.g. fisheye
  distortion not corrected, or off-by-one cell indexing), and a visual
  glance will not reliably catch that.

**Gate:**
- `cargo test -p client render::raycast` green.
- Manual: walk/look around the maze with keyboard/mouse, visually confirm
  walls render at sensible distances/angles and there's no obvious
  fisheye warping or misaligned geometry.

---

## Milestone 10 — Client-side prediction + camera movement

**Build:**
- Accumulate elapsed time and emit exactly one `Input` per whole
  `INPUT_DT_MS`; for each emitted input, send it *and* advance local
  prediction by that same one `resolve_move` step. Do **not** step
  prediction once per rendered frame — at 120fps that integrates four
  times the distance the 30Hz server does, and every reconciliation snaps
  the player backwards (Architecture §6.2).
- Keep a `(input_tick, input, predicted_pos_after)` history buffer sized
  `CLIENT_INPUT_HZ * MAX_RTT_S`.
- Reconcile against `PlayerSnapshot.last_input_tick`: compare the server's
  position to the *matching* history entry, and on divergence reset and
  replay the newer buffered inputs (Architecture §6.3).

**Tests:**
- Drive synthetic input through the client's prediction path against a
  mock network layer that never responds, and assert the predicted
  position updates on the same frame as the input, with no round trip.
- Frame-rate independence: run the same one second of held input at a
  simulated 30fps and at 120fps, assert the predicted displacement matches
  within epsilon. This is the direct test for the
  step-per-frame-vs-step-per-input bug above, and it fails loudly on it.
- Reconciliation unit test (Architecture §8.1): feed a snapshot whose
  `last_input_tick` is several inputs behind the newest prediction, with
  the server agreeing about that older position. Assert the correction is
  zero after replay. An implementation that compares against the newest
  prediction instead produces a non-zero correction every tick here —
  that is the rubber-banding bug, caught without a live server.

**Gate:**
- Manual: with a running server, move around — camera must respond
  instantly to input with no visible lag, even if you artificially add
  latency (see Milestone 13's latency injection tool, or just note this
  as a re-check after that tool exists).

---

## Milestone 11 — Minimap + multi-client visibility

**Build:**
- `client::render::minimap`: top-down draw of `MazeData` + self marker +
  remote player markers.
- Wire `WorldState` reception into a per-remote-player snapshot buffer
  (raw, no interpolation yet — that's Milestone 12).

**Tests:**
- Minimap coordinate-mapping unit test: given a known player world
  position and known maze dimensions, assert the computed minimap pixel/
  screen coordinate matches a hand-calculated expected value. (Catches
  flipped axes / off-by-half-cell errors that are easy to miss visually
  since "roughly in the right area" looks fine at a glance.)

**Gate:**
- Manual, two local clients: both appear on each other's minimap; moving
  one visibly moves its dot on both clients' minimaps.
- This is also the first point where "Does the client... minimap... does
  moving update it" from the audit is genuinely checkable end to end —
  run through that exact audit wording now, not just at the end.

---

## Milestone 12 — Remote player interpolation

**Build:**
- Server-clock offset estimation from `Welcome`/`Pong`
  `(server_tick, server_time_ms)` plus measured RTT, smoothed — not the
  latest raw sample, and never anchored to "the first tick I received"
  (Architecture §6.4). The render target is a local wall-clock time and
  snapshots carry a tick counter; without this mapping there is nothing to
  interpolate against.
- Per-remote-player snapshot buffer of `SNAPSHOT_BUFFER_LEN` (5), not 2:
  at 30Hz, rendering 100ms behind needs the 4th and 5th most recent
  snapshots. A 2-entry buffer spans 33ms, so every frame silently falls
  through to extrapolation and you get the exact choppiness this milestone
  is meant to remove.
- Replace raw-snapshot rendering of remote players with the interpolation/
  bounded-extrapolation scheme from Architecture §6.4.

**Tests:**
- Unit test the interpolation function directly per Architecture §8.1:
  three cases — query time between two snapshots (linear interp expected
  value), query time after the latest snapshot within the extrapolation
  window (extrapolated expected value), query time beyond the
  extrapolation cap (held at last-known position, not runaway
  extrapolation).
- Buffer-length test: fill a buffer at `SERVER_TICK_HZ` for a second,
  query at `now - INTERP_DELAY_MS`, and assert the **interpolated** branch
  was taken. Without this, a too-short buffer degrades every query to
  extrapolation and the three cases above still pass, because they hand
  the function timestamps chosen to bracket.
- Clock-offset test: feed `Pong`s with a known server time and a simulated
  RTT plus one badly delayed outlier sample, assert the smoothed offset
  ignores the outlier rather than tracking it.

**Gate:**
- `cargo test -p common interp::` green (or wherever this lives).
- Manual, two local clients: with the other client moving steadily,
  confirm the remote player's on-screen movement looks smooth, not
  stepped/choppy, at normal network conditions.
- Manual, artificially throttle one client's network read rate or add
  a small sleep in its recv path to simulate a slow link, confirm
  degraded-but-still-reasonable (not violently jumpy) remote movement.

---

## Milestone 13 — Shooting, health, respawn, kill feed end-to-end

**Build:**
- Wire input `shoot: true` through to server's `raycast_hit`, damage
  application, `Hit`/`Killed`/`Respawned` events, client-side HUD
  reaction (health display, kill feed from events).

**Tests:**
- Server integration test: two fake clients, one shoots when aimed at the
  other, assert the target's hp decreases via the `Hit` event and, at 0
  hp, a `Killed` event fires and a subsequent `Respawned` event places
  the player at a valid open (non-wall) cell (check against `MazeData`,
  don't just check "some position came back").

**Gate:**
- `cargo test -p server` (extended) green.
- Manual, two local clients: shoot each other, confirm hp/kill feed
  updates correctly on both ends and respawn lands somewhere walkable.

---

## Milestone 14 — Level progression / difficulty

**Build:**
- `server::levels` table (≥3 entries) wired to actually trigger
  `LevelChanged` (pick a trigger condition — e.g. time-based or
  score-based — and document the choice), client handling of
  `LevelChanged` (regenerate `MazeData` from the `MazeSource`, reset local
  prediction state and all snapshot buffers).
- `level_epoch` incremented per change, stamped on every `WorldState`, and
  **checked on the client**: snapshots whose epoch doesn't match the maze
  currently held are dropped, not rendered (Architecture §4.4).

**Tests:**
- Reuses Milestone 1's dead-end-density test against the *actual*
  configured level table (part (b), left red/pending earlier — it must be
  green now with real numbers).
- Epoch-mismatch test: deliver a `WorldState` with a `level_epoch` the
  client has no maze for and assert it is discarded rather than rendered.
  Simulate the real race by dropping the `LevelChanged` datagram once —
  it is retried at 100ms while snapshots arrive every 33ms, so several
  mismatched snapshots always arrive first; rendering them puts players
  inside walls.
- Integration test: server progresses through levels on the configured
  trigger, connected fake client receives `LevelChanged` with a
  genuinely different (larger/harder) maze each time.

**Gate:**
- `cargo test --workspace` green.
- Manual: play through a level transition on a live client, confirm the
  maze visibly changes and the player isn't left stuck inside geometry
  from the old maze.

---

## Milestone 15 — Load & soak validation (the pre-submission dress rehearsal)

**Build:**
- If not already built as part of Milestone 7's load test, build a small
  standalone bot-client binary (`xtask` or `server --bots N`) that can
  drive many simulated players against a real server process.

**Tests / checks:**
- Automated: extend Milestone 7's tick-rate test to the full 10+ clients,
  3-minute duration, logging tick rate and any dropped/timed-out
  connections to a file.
- Manual, exactly mirroring the audit wording: run a real client
  alongside 9+ bots (or additional local client processes) for 3+
  minutes, watch the client's own fps counter stay above 50 throughout,
  and separately judge "does it feel smooth" as its own explicit check —
  do not let a good fps number stand in for this judgment.

**Gate:**
- Logged evidence file showing sustained tick rate and client fps over
  the full 3-minute run.
- `cargo clean && cargo build --workspace` — zero warnings, one final
  time, from a clean checkout.

---

## Milestone 16+ — Bonus features (only after Milestone 15 gate passes)

Do these in this order (highest audit-value / lowest-risk first, per
Architecture §7):

1. **AI bots as a real feature** (if not already fully productized from
   the load-testing bot client): expose as a documented server flag,
   confirm bots path through identical `common::sim` logic as real
   players (write one test asserting a bot cannot be observed walking
   through a wall, exactly like a real player can't).
2. **Procedural levels** — likely already satisfied by Milestone 14; just
   confirm the generator name/seed is visibly logged or shown, per
   Architecture §7.1, so it's unambiguous during grading.
3. **Maze editor** — build only after core game is fully gated; export/
   import format must round-trip through the same `MazeGrid` serde types
   used by the wire protocol (add a round-trip test for the file format,
   same pattern as Milestone 2). Two non-negotiables: toggling an edge
   writes **both** cells' wall bits (Milestone 1's symmetry test exists
   for this — run it over editor output too), and a loaded `--maze` file
   is validated against the full §5.3 invariant list *and* the
   `MAX_PAYLOAD_BYTES` limit before it is served to clients, since a
   custom maze travels as an expanded grid rather than a seed.
4. **Host history / GUI launcher** — lowest priority, build last, must not
   replace or break the CLI prompt flow from Milestone 8.

**Gate for each bonus item:** it must pass its own equivalent "yes/no"
audit question fully — a half-working editor or bot scores nothing per
the audit's phrasing, so don't spread effort across all four if time is
short. Pick as many as you can *fully* finish, in the priority order
above.

---

## Running log (agent: append entries here as you complete milestones)

Keep a short dated log at the bottom of this file as work progresses,
e.g.:

```
2026-XX-XX: Milestone 1 complete. proptest connectivity check ran 500
cases, all green. Dead-end monotonicity test initially red with
braid_factor = [0.5, 0.5, 0.5] (placeholder) — fixed level table to
[0.6, 0.3, 0.05], now green.
```

This gives the next agent/session picking this up a trail of what was
actually verified, not just what was written.

---

2026-09-07: Milestone 0 complete. Workspace created with `common` (lib),
`server` and `client` (bins), edition 2024, resolver 3, shared versions via
`[workspace.dependencies]`. Server deps: `common` only — picked plain
`std::net`/threads over tokio, recorded in ARCHITECTURE.md §4.1. Client:
`macroquad` 0.4 (added, not yet used — no window until milestone 8).
`common`: `serde` 1 + `bincode` 2 (chosen over postcard because
`config::standard().with_limit()` gives the MAX_PAYLOAD_BYTES read cap
milestone 2 requires), `proptest` 1 as a dev-dep. No tests written, per the
milestone. Gate: `cargo clean && cargo build --workspace` finished with zero
warnings (grepped the raw output, 0 lines matching `warning`), and both
binaries run and exit 0.

2026-09-17: Milestone 1 complete (`common::maze`, ARCHITECTURE.md §5).
`MazeSpec`/`MazeGrid`/`MazeSource`/`MazeData` per §5.1, `generate(spec) ->
MazeData` per §5.2: randomized recursive backtracker (iterative, explicit
stack — no recursion depth risk on large mazes) for the spanning tree, then
a braiding pass over dead-end cells only (candidates restricted to walls
with an in-bounds neighbour, so border walls — §5.3 invariant 3 — are never
touched). `MazeSource` and `MazeData` live in `common::maze` even though
ARCHITECTURE.md §3.2 discusses `MazeSource` under the protocol section;
`common::protocol` (Milestone 2) should `pub use maze::MazeSource` rather
than redefining it.

RNG: added `rand` + `rand_chacha` as workspace deps (not in the original
Cargo.toml), seeded from `MazeSpec.seed` via `ChaCha8Rng::seed_from_u64`.
ChaCha8 is a pure software algorithm with no OS-entropy dependency, so
output is reproducible across machines given the same crate versions —
Cargo.lock is committed, so that holds. **Gotcha for whoever touches this
next**: edition 2024 makes `gen` a reserved keyword, so `Rng::gen` must be
called as `rng.r#gen::<f32>()`, not `rng.gen::<f32>()` (the latter is a
syntax error, not a warning).

Tests (`common/src/maze.rs`, bottom `mod tests`): determinism, different-seed
divergence, `braid_factor = 0.0` reproduces the bare spanning tree's
dead-end count exactly, a `proptest` property test over random `(seed,
width, height, braid_factor)` asserting all four §5.3 structural invariants
(connectivity, no isolated cells, closed borders, wall symmetry), and two
dead-end-*density* monotonicity tests for §5.3 invariant 5 — one with grid
size held fixed, one across a placeholder 3-entry table
`(20,20,0.6),(30,30,0.3),(40,40,0.05)` standing in for `server::levels`
until Milestone 14 builds the real one (that milestone should replace the
placeholder table in a *new* test with the real one, per PLAN.md's own
instruction — don't just edit this test's numbers). Both density tests
average over 40 seeds per data point because the metric is stochastic per
seed; a single-seed trial is not reliable evidence of monotonicity either
way, in either direction. All passed on the first parameter choice, no
tuning needed.

Visibility decision worth knowing before Milestone 16 (maze editor, §7.3):
the four §5.3 invariant-check helpers (`is_fully_connected`,
`has_no_isolated_cell`, `borders_are_closed`, `walls_are_symmetric`) are
`#[cfg(test)] pub(crate)` — nothing outside tests calls them yet, and
Milestone 2's "no `#[allow(dead_code)]`" instruction implies the converse
too: don't pre-empt a real dead-code warning by adding unused-but-`pub`
surface area before something real needs it. When the editor's `--maze`
loader needs to validate a file against the full invariant list, promote
these to real `pub` items called from that load path instead of
reimplementing the flood fill etc. there. `dead_end_density`, by contrast,
is already plain `pub` (no `#[cfg(test)]`) because Milestone 14's reuse of
it is already committed in this file, not speculative.

Manually ASCII-printed one generated maze (20x12, seed 99, braid_factor 0.3)
per this milestone's own gate instruction: connected corridors with
scattered loops, no open-room-with-pillars degenerate case — looks like a
real maze. Done with a temporary `#[ignore]`'d test, removed before
committing; it was never shipped code.

Gate: `cargo clean && cargo build --workspace` — zero warnings (grepped, 0
matches). `cargo test -p common maze::` — 7/7 green, including the proptest
run (default ~256 cases).

2026-09-17: Milestone 2 complete (`common::protocol`, ARCHITECTURE.md §3.2).
`ClientMsg`/`ServerMsg`/`EventKind`/`PlayerSnapshot` as specified. Two new
supporting modules this milestone needed that weren't built yet:
- `common::config` — the §9 "single source of truth" constants (not the
  §9.1 runtime `Config` struct, which is still Milestone 6's job). Moved
  `GENERATOR_VERSION` here from `common::maze` (Milestone 1 had defined it
  locally since `common::config` didn't exist yet); `maze.rs` now does
  `pub use crate::config::GENERATOR_VERSION` so nothing calling
  `maze::GENERATOR_VERSION` broke. **If you add a new §9 constant, put it
  in `config.rs`, not next to whatever module first needed it** — this is
  exactly the drift the architecture doc warns about.
- `common::types` — `PlayerId`/`Tick` type aliases and `Vec2` (minimal:
  just the struct + `new`/`ZERO`; arithmetic ops deliberately not added
  yet since nothing needs them until Milestone 4's `sim.rs`).

`MazeSource` stays defined in `common::maze` (Milestone 1's call, logged
there) — `protocol.rs` imports it rather than redefining it, matching
ARCHITECTURE.md §3.2's usage even though the type itself lives in the maze
module.

Serialization: `bincode::config::standard().with_limit::<MAX_PAYLOAD_BYTES>()`
via `bincode::serde::{encode_to_vec, decode_from_slice}`, wrapped as
`protocol::encode`/`protocol::decode` so every call site gets the byte
limit automatically — no call site builds its own config.

Tests (`common/src/protocol.rs`, bottom `mod tests`): round-trip tests for
every `ClientMsg`/`ServerMsg`/`EventKind` variant, each backed by an
exhaustive `match` with no wildcard arm (adding a variant without updating
the match is a compile error — the nudge to also add a sample instance,
since nothing *forces* that half); a payload-budget test covering every
variant plus a `WorldState` built with `MAX_PLAYERS` (20) players, not 10;
and a hostile-payload test.

The hostile-payload test is hand-built, not guessed: I read bincode
2.0.1's actual source (`varint/mod.rs`, `features/serde/ser.rs`,
`features/impl_alloc.rs`) rather than assume the wire format, then verified
empirically before trusting it — encoded a real `Join{name:"george",..}`
and confirmed the byte prefix is `[0x00 (variant 0), 0x06 (name len), ...]`
matching what the source predicts, and confirmed the malicious buffer
`[0x00, 253, 0xFF*8]` (declares a `u64::MAX`-length name) decodes to
`Err(LimitExceeded)` specifically — not some other error that would pass
the test for the wrong reason. Also confirmed from source that
`Vec<T>`/`String` decode calls `decoder.claim_container_read(len)` *before*
allocating, which is why a limited config rejects the huge declared length
instead of attempting the allocation the attack is going for. Both
verification snippets were temporary `#[ignore]`/debug-`eprintln!` tests,
removed before committing.

No `#[allow(dead_code)]` anywhere (grepped `common/src/`, confirmed empty)
— per Milestone 2's own instruction.

Gate: `cargo clean && cargo build --workspace` — zero warnings. `cargo test
-p common` — 12/12 green (7 maze + 5 protocol).

2026-09-17: Milestone 3 complete (`common::reliability`, ARCHITECTURE.md
§3.3). `Sender<T>` (generic over payload — doesn't need to know about
`protocol::EventKind`) tracks a retry table and, on `due_for_retry(now_ms)`,
returns both what to resend and what just gave up (the caller logs
give-ups; this module does no I/O/logging itself, per `common`'s
zero-I/O contract — including no real clock reads, which is why every
function takes `now_ms: u64` as a parameter instead of reading a clock).
`Receiver` is a bounded-ring dedupe set (`accept(event_id) -> bool`, true
only the first time).

Give-up timing note for whoever wires this into the real server: an event
is dropped as given-up on the retry check *after* it reaches
`max_attempts`, not the moment it reaches it — so worst-case time-to-give-up
is `max_attempts * retry_interval_ms`, matching §8.2's own test wording
exactly, but if you were expecting `(max_attempts - 1) * retry_interval_ms`
this is why the numbers look one interval too long. Deliberate, not a bug.

Test harness (`LossyChannel`, inside `#[cfg(test)] mod tests` — never
shipped, per this milestone's own instruction): seeded `ChaCha8Rng`,
`transmit()` drops/duplicates one message, `reordered_batch()` shuffles a
whole batch. All three §8.2 tests run across 8 fixed seeds (gate asks for
"at least 3").

Scope call worth flagging for whoever picks this up next: §8.2's third
test ("reordering... a stale position doesn't overwrite a newer one") is
about *state* messages, not the `Event` channel this module actually
implements — `WorldState` is explicitly unordered/latest-wins/no-retry
(§3.1) and has no consumer built yet (that's Milestone 7+ server-side,
Milestone 12 client-side interpolation). I wrote it as a self-contained
test against a minimal `LatestTickWins` reducer defined *inside the test
module only* — it validates the general pattern the real `WorldState`
consumer will need, using the same shared fake channel, without adding
speculative production API to `common::reliability` for a consumer that
doesn't exist yet. When Milestone 7/12 build the real thing, this test
doesn't need to move — it already proves the pattern; the real consumer
gets its own tests against its own types.

Verified both to-be-caught bugs are actually caught before trusting the
suite: temporarily short-circuited `due_for_retry` to never resend
(`delivery_under_loss_within_bound` failed, "event 8 never delivered") and
`Receiver::accept` to never dedupe
(`receiver_fires_exactly_once_under_duplication` failed, "left: 2, right:
1"), then reverted both. Neither survived in the committed code.

Gate: `cargo clean && cargo build --workspace` — zero warnings. `cargo test
-p common` — 17/17 green (7 maze + 5 protocol + 5 reliability).

2026-09-17: Milestone 4 complete (`common::sim::resolve_move`,
ARCHITECTURE.md §4.3). Circle-vs-grid collision, axis-separated (resolve X
displacement, then Y from the possibly-X-blocked result) — this is what
produces wall sliding "for free": a diagonal move blocked on one axis still
applies the other, no separate sliding logic needed. Each axis moves in
`MAX_SUBSTEP = 0.02`-unit increments, stopping at the first substep that
would cross a wall (`circle_fits`), so it never tunnels through a wall
regardless of how large `dt_s` is.

**Design constraint worth knowing before Milestone 7/13 pick a real player
radius**: `circle_fits` only checks the *current* cell's own wall bits, not
a neighbouring cell's mirrored bit. That's sufficient — and cheap — only
because `radius` is assumed well under half a cell (0.5); a radius at or
above that could reach into a cell the check never looks at. Milestone 7's
`server::world` should pick a radius comfortably under 0.5 (something like
0.2–0.3) and not treat this as a general-purpose circle-vs-polygon
collider.

Tests (`common/src/sim.rs`, bottom `mod tests`): end-wall stop (within
`EPS = 0.03`, one substep of slack), diagonal-into-corner sliding (asserts
the *unblocked* axis actually advanced by the expected clamped-diagonal
amount, not just "not exactly zero"), a doorway boundary test with two
sub-cases (radius 0.49 passes straight through a 1-wide corridor, radius
0.51 is blocked immediately — same corridor, only the radius changes),
double-dt-doubles-displacement, and the magnitude-100 clamp test.

Per this milestone's own gate instruction ("verify by temporarily breaking
the implementation"), stubbed `resolve_move` to `return pos` unchanged and
reran: 3 of 5 tests failed as expected, but **2 passed anyway** —
`double_dt_doubles_displacement_in_open_space` and
`oversized_move_dir_is_clamped_to_unit_length` only asserted a *relative*
property (0 == 2×0, and equal-to-itself), which a no-op stub satisfies
vacuously. Fixed both by adding an explicit "real movement happened"
assertion (displacement > 0.1) before the relative check, reran the stub —
all 5 failed — then reverted the stub. This is exactly the failure mode
§8.6 warns about, worth remembering if you write a "these two calls should
produce related outputs" test anywhere else in this project: always assert
*a value*, not just the relationship between two values, or a stub can
satisfy the relationship trivially.

`walled_grid`/`open()` test helpers hand-build a `MazeGrid` directly rather
than going through `maze::generate` — deliberate, so these tests have exact
control over wall placement instead of depending on a seed producing the
right shape.

Gate: `cargo clean && cargo build --workspace` — zero warnings. `cargo test
-p common` — 22/22 green (7 maze + 5 protocol + 5 reliability + 5 sim).

2026-09-17: Milestone 5 complete (`common::sim::raycast_hit`,
ARCHITECTURE.md §4.3). Standard DDA grid raycast (`raycast_wall_distance`)
for the nearest wall crossing, plus analytic ray-circle intersection
(`ray_circle_distance`) per candidate player; `raycast_hit` takes the
smaller of the two, so "first collision wins" falls out of a plain
distance comparison rather than needing separate occlusion logic. `players`
is the *candidate* list — the caller (future `server::world`) is
responsible for excluding the shooter before calling this; the function
itself has no concept of "self."

Tests (`common/src/sim.rs`, appended to the same `mod tests`): a 10-cell
corridor helper (`corridor(blocked_after)`) reused across all four cases —
wall-only hit at the exact expected distance, player-closer-than-wall
(distance computed by hand: near edge of a radius-0.3 circle at x=5.0,
`(5.0-0.3)-0.5 = 4.2`), wall-closer-than-player (a player sits beyond a
mid-corridor wall and must never be reported), and a clean miss when
`max_range` is shorter than the corridor. All four passed against the
first implementation — no distance math needed correcting.

Per the gate's "spot-check at least one by breaking it on purpose"
instruction, removed the occlusion guard (`if dist >= wall_distance {
continue }`) and reran just the wall-closer-than-player test: it failed,
reporting the occluded player as hit at distance 6.2 instead of the wall
at 3.5 — confirming that test doesn't pass by accident. Reverted before
committing.

This completes `common` (Milestones 1–5): maze generation, wire protocol,
reliability layer, movement/collision, and shooting are all built and
tested. Per the fork-point discussion earlier in this project's history —
`common`'s public contract (types, constants, `resolve_move`,
`raycast_hit`, `generate`) is now the frozen interface both `server` and
`client` build against. This is the intended split point for two
devs/agents to work `server` (Milestones 6–7, 13–14) and `client`
(Milestones 8–13) in parallel: branch `server-track`/`client-track` off
`develop` from here, keep `common` changes on `develop` directly (logged in
this file, both tracks rebase), and gate every merge on
`cargo test --workspace` staying green.

Gate: `cargo clean && cargo build --workspace` — zero warnings. `cargo test
-p common` — 26/26 green (7 maze + 5 protocol + 5 reliability + 9 sim).

2026-09-17: Fork-point infrastructure set up (see the note above). Added
`.github/workflows/ci.yml` on `develop`: builds + tests the whole workspace
on every push (any branch) and on PRs into `develop`/`main`, with
`RUSTFLAGS="-D warnings"` — the CI equivalent of the manual
"grep the build output for `warning:`" gate every milestone above was held
to, turning any warning into a hard failure instead of a thing a human has
to remember to check. Verified locally with
`RUSTFLAGS="-D warnings" cargo build --workspace` and
`... cargo test --workspace` before pushing — both clean.

Cut `server-track` and `client-track` from this commit on `develop` (repo
is `origin` = `github.com/omincron/multiplayer-fps`, both branches pushed).
Convention going forward, per the earlier handoff discussion: `common::`
changes land on `develop` directly (never on a track branch), get logged
here, and whichever track didn't make the change rebases onto `develop`
and reruns `cargo test --workspace` before continuing. Server work resumes
at Milestone 6 on `server-track`; client work resumes at Milestone 8 on
`client-track` (Milestone 6/7 gate the server enough that Milestone 8's
manual "connect a real client to a real server" check has something to
connect to, but the two tracks otherwise don't block each other).

2026-09-17: `common::config::Config` added (§9.1) — on `develop` directly,
per the fork-point convention above, since Milestone 6 (`server-track`)
needs it but it's a `common` change. `bind_addr`, `tick_hz`, `max_players`,
`client_timeout_ms`, `retry_interval_ms`, `max_retry_attempts`; `Default`
built from the existing §9 constants plus a new `DEFAULT_PORT = 7777`
(arbitrary choice, not in the architecture doc's constant table — tests
never reference it, they always override `bind_addr` to `127.0.0.1:0`).
One test: `Default` actually matches every constant it's supposed to be
built from, and binds `0.0.0.0` (not loopback) by default. `server-track`
should rebase onto this commit before continuing Milestone 6; `client-track`
doesn't need it yet but was fast-forwarded too since it hadn't diverged.

Gate: `cargo clean && cargo build --workspace` — zero warnings. `cargo test
-p common` — 27/27 green (7 maze + 5 protocol + 5 reliability + 9 sim + 1
config).

2026-09-17: Milestone 6 complete (`server::net`, ARCHITECTURE.md §4.1/§4.2)
on `server-track` (rebased onto the `common::config::Config` commit above
first). Socket I/O thread (blocking `recv_from`, deserializes, forwards
`(SocketAddr, ClientMsg)` over an `mpsc` channel, never touches game state)
plus a connection-lifecycle thread that owns all mutable state — same
two-thread shape §4.1 describes for the real simulation thread, just with
no movement yet. The lifecycle thread wakes at least every
`LIFECYCLE_TICK_MS = 20` (`recv_timeout`, not a busy spin) even with no
incoming message, so idle-timeout checks and event retries happen on a
steady cadence independent of traffic.

**Structural change**: `server` was bin-only; added `server/src/lib.rs`
(`pub mod net;`) because `server/tests/` integration tests can't link a
bin-only crate. `main.rs` now does `use server::net;` instead of
`mod net;`. Anyone adding another server module later should add it to
`lib.rs`, not declare it again in `main.rs`.

Connection lifecycle: `Join` checked in order — protocol version, then
generator version (separate rejection reasons: a generator mismatch is
what stops two sides silently regenerating different mazes from the same
seed, so it's diagnostically distinct from a protocol mismatch), then
capacity, then duplicate name — assign `PlayerId`, reply `Welcome` with a
maze generated once at server startup (no `server::levels` table yet,
that's Milestone 14; every joiner gets the same maze until then), broadcast
`PlayerJoined` to everyone else. Idle timeout and `Leave` both funnel
through one `remove_player`, which broadcasts `PlayerLeft`. `Ping`/`Input`
both count as the keep-alive §4.2 describes ("no Input/Ping received");
`Ack` deliberately does not extend it, matching that wording exactly.

Reliability layer (Milestone 3) is wired in for real here, not just
tested in isolation: **one `reliability::Sender<EventKind>` per connected
player** (not one global sender) — necessary because a broadcast event
like `PlayerJoined` has multiple independent recipients, each of whom
might ack at a different time or not at all, and the Milestone 3 `Sender`
only models one recipient's retry state per instance. The lifecycle loop
calls `due_for_retry` on every player's sender every tick and
retransmits/logs give-ups accordingly.

**Known gap, expected until Milestone 7**: a newly-joined player currently
has no way to learn about already-connected players — `PlayerJoined` only
goes to "everyone else" (per §4.2's own wording), and there's no
`WorldState` broadcast yet for a new joiner to learn the roster from.
Not a bug to fix now, just don't be surprised by it.

Tests (`server/tests/lifecycle.rs`, real loopback `UdpSocket`s, no
mocking): single-client Welcome, capacity (reject the 3rd of a 2-cap
server with `"server full"`, verify the first two are still responsive via
Ping/Pong), duplicate name, idle timeout (150ms configured timeout, second
client polls for the `PlayerLeft` event while pinging to stay alive
itself), and the two version-mismatch cases. One real bug caught and fixed
during development, not a hypothetical: the capacity test's first version
assumed the very next message after a `Ping` would be its `Pong`, but
client `a` can have a `PlayerJoined` event (about `b`'s join) queued ahead
of it — a genuine race, not a server defect. Fixed by looping past `Event`
messages until the `Pong` arrives. Ran the suite 5x in a row before
trusting it (timing-sensitive tests: idle-timeout polling, capacity race).

Per the gate's manual-check instruction, ran the real `server` binary as
its own OS process and hit it with a standalone scratch client (a separate
tiny Cargo project in the scratch directory depending on `common` by path,
not sharing any code with the test harness) sending a hand-built `Join` —
got `Welcome { player_id: 1, ... }` back over real loopback UDP,
independent of any assumption baked into `server/tests/`. Scratch project
deleted after; nothing from it was committed.

Gate: `cargo clean && cargo build --workspace` — zero warnings. `cargo test
--workspace` green (27 common + 6 server). Manual end-to-end check passed.

2026-09-17: Milestone 7 complete (`server::world`, ARCHITECTURE.md §4.1/
§4.3) on `server-track`.

**Refactor**: per PLAN.md's own Milestone 7 build note ("`server::world::
World`... wired to `net`"), moved Milestone 6's connection-lifecycle
`Server`/`lifecycle_loop` out of `net.rs` and into a new `world.rs` as
`World`/`run`, extended with real movement. `net.rs` is now thin: bind,
`io_loop` (blocking recv/decode/forward), a `send()` helper, and `spawn`/
`spawn_with_maze` (the latter added so tests can inject a deterministic
maze — see below). This matches ARCHITECTURE.md §2's own file-layout
description (`net.rs` = "recv loop, per-client address<->id table",
`world.rs` = "authoritative game state + tick loop") more closely than
Milestone 6's single-file version did; nothing about Milestone 6's tests
needed to change beyond the import path.

Tick loop (`world::run`): fixed `1000/config.tick_hz` ms timestep via
`Instant`-based deadlines, not a plain sleep loop — `rx.recv_timeout`
blocks until either a message arrives or the next tick deadline, so idle
periods don't busy-spin and traffic doesn't delay the tick. Falls behind
(debugger pause, sustained overload) → resyncs to now rather than bursting
through a catch-up backlog, which would only compound the problem under
real load.

Movement: per-player `VecDeque<QueuedInput>`, drained oldest-first, capped
at `MAX_INPUT_QUEUE` (6) *consumed per tick* — **the queue itself is
unbounded on insertion**, only consumption is capped. §4.3 says a flooding
client's excess inputs "just back up" (i.e. get consumed on a later tick),
not that they're discarded; capping insertion instead would silently drop
input; the flood test below is specifically what confirms this
distinction, not the cap in general. `last_input_tick` is the highest
*consumed* tick (`.max()`), guarded even though FIFO draining should
already guarantee monotonicity — cheap insurance if that ever changes.
`WorldState` is encoded once per tick and sent to every player from the
same byte buffer, not re-encoded per recipient.

**Real bug caught by the tests, not hypothetical**: the initial spawn-point
formula was `dimension as f32 / 2.0 + 0.5`, which looks like "grid center"
but for a degenerate `height = 1` grid evaluates to exactly `1.0` —
*outside* the valid `[0, 1)` range, so `circle_fits`'s bounds check
rejected every single movement attempt from spawn, silently. The
`input_that_walks_into_a_wall_stops_at_the_wall` test (using a 4x1
corridor fixture) caught this immediately: position never left spawn at
all. Fixed by computing the center **cell index** via integer division
first (`width / 2`, `height / 2`), then `+ 0.5` — correct for every grid
size including degenerate ones. Even-dimension mazes (the 20x20 and 30x30
used by the other tests) were unaffected either way, which is exactly why
this only showed up once a test used an odd/degenerate dimension
deliberately.

**Determinism for movement tests**: added `net::spawn_with_maze(config,
maze: MazeData)` alongside `spawn` — production still generates a real
random maze, but tests inject a hand-built one (`server/tests/support/
mod.rs`: `walled_grid`/`open`, same pattern as `common::sim`'s Milestone 4
test helpers, duplicated locally since those are private to `common`'s own
test module). `open_room(w, h)` for movement/flood/tick-rate tests (every
direction unobstructed from spawn) and `corridor_with_wall_east_of_spawn()`
for the wall-stop test (relies on knowing `World`'s exact spawn formula, so
the two are coupled by design — a spawn-formula change should re-derive
this fixture's expected wall position, not just re-run the test and see).

**Test-sharing wrinkle**: moved `FakeClient` etc. into `server/tests/
support/mod.rs` (the standard `mod support;`-per-file pattern, since each
top-level file under `tests/` compiles as its own separate binary and
can't otherwise share code). Side effect: each binary only uses a subset
of the shared helpers, so rustc's dead-code lint fires per-binary false
positives (a function `lifecycle.rs` doesn't call looks unused from ITS
compilation even though `tick_loop.rs` uses it, and vice versa). Added
`#![allow(dead_code)]` to `support/mod.rs` specifically, with a comment
distinguishing this from Milestone 2's no-`#[allow(dead_code)]` rule —
that rule was about a real gap (an unused protocol variant) staying
visible; this is a known Rust tooling limitation for shared test code, not
a gap to hide. Confirmed this doesn't mask anything by running under
`RUSTFLAGS="-D warnings" cargo test --no-run` before and after adding it.

Tests (`server/tests/tick_loop.rs`, 6 total): movement reflected in
another client's `WorldState` within a bounded few-tick window (not
instantly); wall-stop (re-validates `resolve_move` is actually wired in,
not just unit-tested in isolation, per PLAN's own framing); two inputs in
one tick window advance two steps; `MAX_INPUT_QUEUE + 5` flood caps at
exactly `MAX_INPUT_QUEUE` steps on the first tick — the first
`WorldState` showing *any* movement is what's checked, since later ticks
would also reflect the backed-up remainder and defeat the assertion if
checked too late; `last_input_tick` echoes 5 (highest consumed) for 5
sent inputs, not 0 and not something else; tick-rate-under-load with 10
clients sending input continuously for 10s (full 3-minute run is the
pre-submission manual soak test, §8.5, not this one) — achieved ~30.76Hz
against a 30Hz target, comfortably inside the 0.8x tolerance. All 12
server tests (6 lifecycle + 6 tick_loop) run 5x in a row, zero flakiness.

Load-test result **persisted**, not a one-time terminal glance (this
milestone's own gate instruction): appended to `target/tick_rate_log.txt`
(gitignored, local) on every run, plus `eprintln!` for CI visibility.
Latest: `clients=10 target_hz=30 achieved_hz=30.76 ticks=302 secs=9.82`.

Gate: `cargo clean && cargo build --workspace` — zero warnings, including
under `RUSTFLAGS="-D warnings"`. `cargo test --workspace` — 39/39 green
(27 common + 6 lifecycle + 6 tick_loop).

2026-09-21: Milestone 8 automated portion complete on `client-track`.
Implemented the required terminal startup sequence as independently testable
prompt functions: invalid socket addresses, empty names, and names over the
24-character client limit produce a clear explanation and re-prompt rather
than panicking. The executable prints `Starting...` only after both inputs are
valid.

Added the UDP join handshake with the shared `PROTOCOL_VERSION`,
`GENERATOR_VERSION`, `JOIN_RETRY_BASE_MS`, and `JOIN_MAX_ATTEMPTS` constants.
The client retains the same connected `UdpSocket` after `Welcome` so later
gameplay traffic keeps the transport identity the server assigned. Tests use
a real loopback scripted responder and cover success after two dropped joins,
immediate final `Rejected` handling, exhausted-attempt diagnostics, and the
production exponential schedule `[250, 500, 1000, 2000, 4000, 8000]` ms.
Test-only options shorten the real waits without changing production policy.

Added a bounded rolling-average FPS meter (60-frame production window from
`FPS_AVG_WINDOW_FRAMES`) and a post-handshake Macroquad window showing the
connected player id and live FPS. Macroquad is started explicitly after the
handshake instead of through its usual entry-point attribute, because that
attribute would create the GUI before the required CLI/connect sequence.

TDD evidence: each FPS, handshake, and prompt slice was first observed red
against its missing implementation, then made green. `cargo test --workspace`
passes 36 tests total (9 client + 27 common); `RUSTFLAGS='-D warnings' cargo
build --workspace` is clean. Client-only `rustfmt --check` and `git diff
--check` pass. Workspace-wide formatting was deliberately not applied because
it would rewrite frozen `common/` files on the client-owned branch.

2026-09-21: Rendezvous point 1 (CLIENT_TRACK_HANDOFF.md) confirmed, doubling
as Milestone 8's own manual gate. `client-track` at `6df5201` (Milestone 8:
CLI prompts, handshake retry/backoff, fps window) connected a real
`cargo run -p client` on a macOS machine to a real `cargo run -p server`
(`server-track`, Milestone 7 HEAD) on a separate Ubuntu machine, both on the
same wifi network — a genuine two-machine test, not loopback. Server bound
`0.0.0.0:7777`; client pointed at the Ubuntu box's LAN IP (found via
`hostname -I`, filtering out the `172.17.0.1`/`172.18.0.1` Docker bridge
addresses also listed — only the real wifi-interface address is reachable
from another machine on the network). Result: window opened showing
`Connected as <name> (player <id>)` and a live-updating FPS counter, i.e. a
real `Welcome` round-tripped over UDP across two machines.

**Known merge hazard for whoever does the client-track/server-track merge**:
`client-track` forked from the `common::config::Config` commit (before
Milestones 6/7's log entries existed) and appended its own Milestone 8 entry
right after that point — so `PLAN.md`'s running log has diverged structure
between the two branches and will produce a real merge conflict on this
file (not just a formatting nuisance) the first time they're merged. Resolve
by keeping both branches' entries, ordered by milestone/date, not by
picking one side.

Nothing in `common/` changed on either side for this checkpoint, so no
rebase was required beyond having both branches reasonably current.
2026-09-21: Milestone 8 complete. The team ran the real `client-track` client
against the Milestone 7 `server-track` binary and confirmed the end-to-end
handshake succeeds, the GUI opens after `Welcome`, and the live FPS display
updates. This closes the manual rendezvous gate. Milestone 9 (client DDA
raycasting against the maze received in `Welcome`) is now the next ready item.

2026-09-21: Milestone 9 automated portion complete on `client-track`. Added
`client::maze::build_and_validate`: generated sources are deterministically
rebuilt from their `MazeSpec`, custom grids are retained directly, and both
paths validate dimensions, known wall bits, closed borders, mirrored interior
walls, no isolated cells, and full connectivity before the GUI opens. Invalid
geometry now fails with a specific client error instead of reaching rendering.

Added `client::render::raycast`, a pure grid-DDA raycaster. It advances from
cell boundary to cell boundary, checking the current cell's directional wall
bit, and returns wall distance plus vertical/horizontal face orientation.
Tests use hand-built 3x3 grids and hand-calculated east, west, and north hit
distances, a bounded-range miss, and a 30-degree projection case proving the
perpendicular-distance correction removes fisheye distortion.

The Macroquad client now casts one ray per screen column against the validated
maze received in `Welcome`, projects corrected depth into flat-shaded vertical
wall strips, shades horizontal and vertical faces differently, and permits
left/right viewing with arrow keys or A/D. Position remains fixed by design;
movement and prediction begin in Milestone 10.

TDD evidence: the DDA API and fisheye correction were each observed failing
before implementation. `cargo test --workspace` passes 45 tests total (18
client + 27 common). Client-only `rustfmt --check`, client Clippy with warnings
denied, `RUSTFLAGS='-D warnings' cargo build --workspace`, and `git diff
--check` all pass.

2026-09-21: Milestone 9 complete. The team ran the renderer against the real
server maze, confirmed walls were visible and the camera rotated correctly
with A/D, and accepted the visual rendering gate. Traversal was intentionally
not part of this milestone; fixed-step movement and collision begin in
Milestone 10 so rendering and movement failures remain independently testable.

2026-09-21: Milestone 10 automated portion complete on `client-track`. Added a
pure `Predictor` that accumulates rendered-frame time and emits exactly one
numbered input for each whole `INPUT_DT_MS`. Every emitted input immediately
advances local position through the shared `common::sim::resolve_move` using
the server-owned speed, then enters a bounded history sized from
`CLIENT_INPUT_HZ * MAX_RTT_S`.

Reconciliation looks up the history entry matching
`PlayerSnapshot.last_input_tick`, never the newest prediction. If that older
prediction agrees with the authoritative position, acknowledged entries are
discarded without moving the current prediction. On divergence, prediction
resets to the authoritative position and replays every newer input. The GUI
maintains a separately decaying correction offset so an actual correction is
visually blended while normal locally predicted movement remains immediate.

W/S or Up/Down now produce forward/backward movement relative to facing;
A/D or Left/Right rotate. The camera and server input both advance on the same
fixed steps, and rendering reads the predicted position. The UDP connection is
split by cloning the already-connected socket: the original sends inputs while
a dedicated blocking receive thread decodes server messages into a channel
that the render loop drains with `try_recv`. Snapshots from a different
`level_epoch` are ignored.

TDD evidence: immediate local movement, 30-vs-120-fps equivalence over one
second, and zero correction when an older acknowledged prediction matches were
written red first. The exact-step test exposed an f32/f64 boundary mismatch
that initially emitted zero inputs and was fixed by computing the fixed step
with the shared f32 expression before promotion. A real loopback UDP test also
proves sending and receiving through the cloned socket preserve one connected
transport identity.

Verification: `cargo test --workspace` passes 49 tests total (22 client + 27
common). Client-only `rustfmt --check`, client Clippy with warnings denied,
`RUSTFLAGS='-D warnings' cargo build --workspace`, and `git diff --check` pass.

Coordination note: client prediction uses radius `0.25`, matching
`server::world::PLAYER_RADIUS`. That radius is server-local rather than part of
the frozen `common` API; promote it to the shared contract on `develop` if the
value ever changes so the two tracks cannot drift.

2026-09-21: Milestone 10 complete. The team tested the real client/server
build and confirmed forward/backward movement and rotation work, movement is
immediate, collision and wall sliding behave correctly, the camera shows no
obvious reconciliation snapping, and FPS remains stable. Artificial-latency
behavior remains an explicit re-check once Milestone 13's latency tool exists.

2026-09-21: Milestone 11 automated portion complete on `client-track`. Added
`client::render::minimap` with a pure world-to-screen mapping tested against
hand-calculated coordinates and orientation endpoints. The live minimap draws
walls directly from the same `MazeGrid` bitflags as collision and raycasting,
preserves the maze aspect ratio, shows self as a yellow marker with facing
direction, and shows remote snapshot positions as red markers.

Added `RemotePlayers`, which treats each `WorldState` as authoritative
membership reconciliation rather than depending on reliable events alone. An
unknown snapshot id is created with a `Player <id>` placeholder, a late
`PlayerJoined` event replaces that name, `PlayerLeft` removes it immediately,
and absence beyond `GHOST_TIMEOUT_MS` removes a ghost even if the leave event
never arrives. Self is deliberately excluded from the remote registry.
Processed `ServerMsg::Event` messages are now acknowledged through the same
connected UDP socket so the server's reliability layer stops retrying them.

TDD evidence: coordinate mapping/orientation and the missing-join/missing-leave
membership paths were written red before implementation. `cargo test
--workspace` passes 54 tests total (27 client + 27 common). Client-only
`rustfmt --check`, client Clippy with warnings denied, `RUSTFLAGS='-D warnings'
cargo build --workspace`, and `git diff --check` pass.

2026-09-21: Milestone 11 complete. The team ran the two-client audit check and
confirmed each client shows itself and the other player on the minimap, remote
markers update as either player moves, and disconnected players disappear.

2026-09-21: Milestone 12 automated portion complete on `client-track`. Added a
bounded, time-sorted `SnapshotBuffer` per remote player. It interpolates
position and shortest-arc facing between bracketing snapshots, extrapolates
velocity only through `MAX_EXTRAPOLATION_MS`, then holds the latest known state
instead of guessing indefinitely. The buffer retains `SNAPSHOT_BUFFER_LEN`
(5) entries, which tests prove brackets the 100 ms delayed render target at the
30 Hz server rate.

Added `ClockSync` driven by one-second `Ping`/matching `Pong` samples. Offset is
selected from the recent sample with the lowest RTT so a badly delayed Pong
cannot drag the render timeline away from buffered snapshots. `Welcome`
provides the initial zero-RTT clock anchor before the first Pong. Snapshot ticks
are converted relative to the latest `(server_tick, server_time_ms)` anchor;
time arithmetic uses `f64` because epoch-sized millisecond values lose usable
precision in `f32`.

Remote minimap markers now render at estimated server time minus
`INTERP_DELAY_MS`, rather than at their latest raw 30 Hz snapshot. Snapshot
insertion tolerates UDP reordering. A red test for a snapshot tick slightly
older than the newest Pong anchor exposed unsigned wraparound (billions of
ticks into the future); the conversion now interprets the wrapping delta as
signed, and both forward and backward anchor cases pass.

TDD evidence covers interpolation, bounded extrapolation then hold, configured
buffer length taking the interpolation branch, shortest-path angle wrapping,
clock outlier rejection, and tick-to-server-time anchoring. `cargo test
--workspace` passes 60 tests total (33 client + 27 common). Client-only
`rustfmt --check`, client Clippy with warnings denied, `RUSTFLAGS='-D warnings'
cargo build --workspace`, and `git diff --check` pass.

Remaining Milestone 12 gates: with two real clients, confirm the moving remote
marker is smooth rather than stepping at 30 Hz; then artificially delay one
client's receive path and confirm motion degrades reasonably without violent
jumps. Do not mark Milestone 12 complete until both visual checks pass.

2026-09-21: `client-track` (through Milestone 12) and `server-track`
(through Milestone 7) merged into `develop` ahead of the mandatory
Milestones 13/14 integration sync (`CLIENT_TRACK_HANDOFF.md` rendezvous
point 2). Merge was clean except for this file, whose running log had
diverged in structure since the Milestone 8 checkpoint (see the "known
merge hazard" note above) — resolved by interleaving both branches'
entries in milestone/chronological order rather than picking one side;
no entry was dropped.

**Open item carried over, not yet closed**: `client-track`'s own last
log entry above says explicitly not to mark Milestone 12 complete until
two visual checks both pass — remote-player smoothness with real
clients, AND a degraded/artificially-delayed connection still moving
reasonably rather than jumpily. Only the first was actually verified
(multiple real clients, including a third connected as `patou` from a
worktree, all visible on each other's minimaps with smooth movement).
The artificial-latency/degraded-connection check was never done. Do
this before treating Milestone 12 as fully closed — it's cheap (throttle
one client's socket read rate or add a small sleep in its recv path per
Architecture §6.4's own suggestion) and it's the one case most likely to
reveal a bug the happy-path check can't.

2026-09-21: Milestone 13, server half complete (`server::world`,
ARCHITECTURE.md §4.3), on `develop` directly — no more separate tracks for
this milestone since it's genuinely joint. Client half (HUD health display,
kill feed from events) is still open, owned by `client-track`'s dev.

**New shared constants** (`common::config`, not in the original §9 table):
`MAX_HP = 100`, `HIT_DAMAGE = 25` (4 hits to kill), `MAX_SHOT_RANGE = 100.0`
(comfortably past the largest configured maze's ~56.6-unit diagonal, so a
shot's outcome is always a wall or a player, never an arbitrary cutoff).
`handle_join`'s hardcoded `hp: 100` was promoted to `MAX_HP` at the same
time, per this file's own "hardcoding one of these values inline instead of
importing the constant is a bug waiting to happen" rule.

**Design**: `apply_queued_input` now also returns a `Vec<ShotRequest>`
(shooter id + the position/facing *at that specific queued input step*, not
wherever the player ends up after every queued input this tick finishes) —
resolution is deferred to a separate `resolve_shots` pass over the returned
list, called right after, because `raycast_hit` needs read access to every
*other* player's position while `apply_queued_input` still holds a mutable
borrow on the shooter. `resolve_shots` re-reads the candidate list fresh
for each shot in the list (not once up front), so two shots landing in the
same tick that both connect see a consistent, up-to-date world — e.g. a
target respawned by an earlier shot this tick isn't shot again at its
stale pre-respawn position. Damage/kill/respawn is one path
(`apply_hit` → `respawn`): hp drops, `Hit` broadcasts, and at exactly 0 hp
`Killed` broadcasts followed immediately by `Respawned` — no death/waiting
period, matching the milestone's own wording ("a subsequent Respawned event
places the player"). All three events broadcast to everyone (`exclude:
None`), same as existing events, since kill feed needs everyone informed,
not just the two players involved.

**Respawn placement**: uniformly random `(x, y)` within the current maze's
grid bounds. No "is this cell walled on all sides" check is needed or
added: walls live on cell *edges* (§5.1), never inside a cell, and §5.3
invariant 2 already guarantees no cell is fully isolated — so every
in-bounds grid cell is valid floor space. This is a materially different
spawn rule from `World::new`'s fixed grid-center `spawn_pos` (join still
uses that, respawn does not) — worth remembering if a future milestone
wants to unify them.

Tests (`server/tests/shooting.rs`, real loopback sockets, same pattern as
`tick_loop.rs`): two fake clients, target moved several cells away from a
stationary shooter (with an explicit assertion that it actually moved,
not just "some position came back"), then exactly `MAX_HP / HIT_DAMAGE`
aimed shots — asserting `Hit`'s `target_hp` matches the expected running
total after *each* one, not just the last — followed by `Killed` (right
shooter/victim ids) and `Respawned` (position checked against the actual
maze bounds, not just "a position came back," per this milestone's own
phrasing), and finally that the *next* `WorldState` agrees with the
`Respawned` event's position and full hp — the event stream and
`WorldState` must not disagree. The test acks every event it receives
(`ClientMsg::Ack`) as a real client would; without that, an unacked `Hit`
retried by the reliability layer mid-test could be misread as a later
expected event — caught while writing the test, not left as a latent
flake.

Per this project's own testing standard (§8.6 / PLAN.md's repeated
"verify the test can actually fail" instruction): temporarily commented
out the damage line in `apply_hit`, reran — the test failed with `left:
100, right: 75` on the first `Hit` assertion, exactly as expected — then
reverted before committing.

Gate (server half only — full Milestone 13 gate needs the client HUD/kill
feed piece too): `cargo clean && cargo build --workspace` — zero warnings
under `RUSTFLAGS="-D warnings"`. `cargo test --workspace` — 73/73 green
(33 client + 27 common + 6 lifecycle + 1 shooting + 6 tick_loop).

**Open for the joint session**: client needs to send `shoot: true` on
fire input (already part of the wire format since Milestone 2 — nothing
new needed there), react to `Hit`/`Killed`/`Respawned` events for the
health display and kill feed, and the two of you should do the manual
"shoot each other, confirm hp/kill feed update on both ends, respawn
lands somewhere walkable" check together once that's wired up.

2026-09-21: Milestone 13, client half complete (`client::main`, plus new
`client::killfeed`), also on `develop` directly. Both halves of Milestone
13 are now in.

**Shoot input**: edge-triggered (`is_key_pressed(Space) ||
is_mouse_button_pressed(Left)`), not held-down — a shot is a discrete
hitscan event (§4.3), not a rate to sustain like movement, so semi-auto
(one shot per press) is the correct mapping, not full-auto-while-held.
Passed into `Predictor::advance_frame`'s existing (previously always-
`false`) `shoot` parameter — that parameter already existed from
Milestone 10 and only needed a real value plumbed in, not a signature
change.

**Health HUD**: `own_hp` is read *only* from `WorldState`'s own snapshot,
deliberately not also patched from `Hit` events — the server is already
the single source of truth (§4.3) and `WorldState` arrives every tick
(33ms), so a second, separate update path from events would just be a
redundant place for the two to disagree, for no perceptible latency win.

**Kill feed** (new `client::killfeed::KillFeed`): a small bounded,
expiring list, deliberately pure and dumb — it takes pre-formatted
`String`s and a timestamp, and knows nothing about players, ids, or "you"
vs. a name. Message formatting (which needs `RemotePlayers` + the local
player's own name) lives in `main.rs`'s new `player_label` helper instead,
keeping the module testable without a `RemotePlayers` fixture.

**Respawn handling, two separate paths because `RemotePlayers` never
tracks self** (it filters `self_id` out of every snapshot on the way in,
per Milestone 11's design):
- Self: `Predictor::respawn_to(pos)` (new method) snaps prediction to the
  server-assigned spawn point and clears buffered input history, since
  every buffered entry predicted a position from *before* the teleport.
  `camera_correction` is also reset to zero in the same event handler —
  without that, a stale in-flight correction from just before death would
  keep decaying visibly across the teleport.
- Remote: `RemotePlayers::apply_event`'s `Respawned` arm now sets the
  player's `pos` directly *and* calls a new `SnapshotBuffer::clear()`
  (`client::interp`). Clearing matters, not just updating `pos`: without
  it, the interpolator would keep bracketing between the stale pre-death
  snapshots and the new position and render a visible slide across the
  map instead of an instant pop. `render_state`'s existing empty-buffer
  fallback (`unwrap_or((self.pos, self.facing))`) is what makes the
  now-current `pos` render immediately with no special-casing needed in
  the render path itself.

Tests: `killfeed.rs` (newest-first ordering, expiry window, capacity
eviction — 3 tests), `predict.rs` (`respawn_to` snaps position and empties
history — 1 test), `players.rs` (a `Respawned` event on a player with
real prior snapshot history renders at the new position immediately,
not something interpolated from the stale ones — 1 test, and this one
would fail without the `clear()` call, not just without the `pos` update).

`cargo test --workspace`: 78/78 green (38 client + 27 common + 6
lifecycle + 1 shooting + 6 tick_loop). `cargo clean && cargo build
--workspace` — zero warnings under `RUSTFLAGS="-D warnings"`.

**What was and wasn't verified here**: a real server + two real client
processes were run together for ~10 seconds (connect, idle) to confirm
nothing panics with the new code paths active — process-alive and
log-based, not a visual check. The actual gameplay gate from PLAN.md
("shoot each other, confirm hp/kill feed update on both ends, respawn
lands somewhere walkable") still needs a human at the keyboard — aiming
and firing can't be driven or observed from here. That manual check is
the next step, not yet done.
