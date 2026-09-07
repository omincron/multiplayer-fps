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
