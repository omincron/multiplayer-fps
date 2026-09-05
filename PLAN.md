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
- `common::maze::MazeData` struct and `generate(seed, width, height,
  braid_factor) -> MazeData` per Architecture §5.

**Tests (write these before you trust the implementation — they are the
actual spec):**
- `proptest`-based connectivity test over random `(seed, width, height,
  braid_factor)` inputs (Architecture §8.1, item 2). Run at least 256
  cases in CI, more locally while iterating.
- No-isolated-cell test.
- Border-wall test.
- Determinism test (same inputs twice -> identical output).
- Dead-end-count monotonicity test across at least 3 `braid_factor`
  values, using the actual constants you intend to put in the level
  table — this test should currently be **red** until you've picked real
  level parameters, which is useful signal, not a problem to hide.

**Gate:**
- `cargo test -p common maze::` all green, including the proptest run.
- Manually print one generated maze as ASCII art to the terminal and
  visually sanity-check it looks like a maze (this is a spot check, not a
  substitute for the property tests above).

---

## Milestone 2 — Protocol types + serialization (`common::protocol`)

**Build:**
- All `ClientMsg`/`ServerMsg`/`EventKind` types from Architecture §3.2.

**Tests:**
- Round-trip test for every enum variant (loop over a hand-built list of
  one instance per variant — if you add a variant later and forget to add
  it to this list, that's a real gap, so structure the test so a missing
  variant is obvious, e.g. exhaustive `match` with no wildcard arm in the
  test's variant-list constructor).
- A size-budget test: serialize a `WorldState` with `MAX_PLAYERS` players
  and a handful of projectiles, assert the byte length stays under your
  chosen UDP payload budget (Architecture §3.2). This test should fail
  loudly (not silently truncate) if someone bumps `MAX_PLAYERS` without
  reconsidering the wire format.

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
- Delivery-under-loss test (Architecture §8.2, item 1).
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
- `resolve_move(maze, pos, move_dir, radius) -> pos'` — circle-vs-grid
  collision with wall sliding (not full-stop).

**Tests:**
- Straight corridor: moving into an end wall stops at the wall, not past
  it, not short of it (assert within epsilon of the wall's plane).
- Diagonal-into-corner: moving diagonally into a wall corner still allows
  sliding progress along the open axis (this is the test that fails on a
  naive "cancel all movement on any collision" implementation — write it
  deliberately to catch that).
- Moving through a doorway exactly at the collision radius boundary
  (edge case most likely to reveal an off-by-epsilon bug).

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
- `server::net`: bind `0.0.0.0:PORT`, recv loop, `SocketAddr <-> PlayerId`
  table, `Join`/`Welcome`/`Rejected` handling, capacity limit, duplicate
  name rejection, idle timeout.
- No movement/shooting simulation yet — players exist but don't move.

**Tests (integration, real loopback sockets, `server/tests/`):**
- Single client connects, receives `Welcome` with a valid `player_id` and
  the expected `maze`.
- `MAX_PLAYERS`-plus-one test: fill capacity, assert the next connection
  gets `Rejected` with a specific reason string, and assert the
  already-connected clients are unaffected.
- Duplicate-name rejection test.
- Idle-timeout test: connect, go silent, assert a still-connected second
  client receives `PlayerLeft` for the timed-out player within a bounded
  window after the configured timeout (use a short timeout constant
  override for the test, don't wait 5 real seconds if avoidable).
- Protocol-version-mismatch rejection test.

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
  Architecture §4.1, applying `common::sim::resolve_move` to player
  inputs, broadcasting `WorldState` each tick.

**Tests:**
- Two fake clients, one sends movement input, assert the *other* client's
  received `WorldState` reflects the moved position within one or two
  ticks (not instantly — respect the tick boundary in the assertion).
- A client sending input that would walk through a wall: assert the
  broadcast position stops at the wall (this re-validates `resolve_move`
  is actually wired in, not just unit-tested in isolation).
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
  `Enter Name:`, `Starting...`), connect handshake with retry/timeout.
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
  responder (per Architecture §8.4): assert correct retry count and
  backoff timing on no response, and a clear failure message/exit code
  when retries are exhausted, and correct success path when a `Welcome`
  arrives after 1-2 dropped attempts.

**Gate:**
- `cargo test -p client` green.
- Manual run: `cargo run -p client`, follow the prompts against a running
  `server` from Milestone 7, confirm connection succeeds and a window
  opens showing a live-updating fps number.

---

## Milestone 9 — Client: raycasting renderer against a static maze

**Build:**
- `client::render::raycast`: DDA raycasting against the `MazeData`
  received in `Welcome`, one vertical strip per screen column, flat-shaded
  walls. No other players yet, no movement yet — just look around a
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
- Wire local input -> immediate local `resolve_move` call -> camera moves
  instantly (Architecture §6.3), independent of server round-trip.
- Send `Input` to server at `CLIENT_INPUT_HZ`.

**Tests:**
- This is largely a wiring milestone; the underlying `resolve_move` logic
  is already tested in Milestone 4. Add one integration-style test: drive
  synthetic input through the client's prediction path and assert the
  rendered/predicted position updates on the same frame as the input,
  without waiting for a network round trip (can be tested by asserting
  against a mock/no-op network layer that never responds).

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
- Replace raw-snapshot rendering of remote players with the interpolation/
  bounded-extrapolation scheme from Architecture §6.4.

**Tests:**
- Unit test the interpolation function directly per Architecture §8.1:
  three cases — query time between two snapshots (linear interp expected
  value), query time after the latest snapshot within the extrapolation
  window (extrapolated expected value), query time beyond the
  extrapolation cap (held at last-known position, not runaway
  extrapolation).

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
  `LevelChanged` (swap `MazeData`, reset local prediction state).

**Tests:**
- Reuses Milestone 1's monotonic-difficulty test against the *actual*
  configured level table (if that test was left red/pending earlier,
  it must be green now with real numbers).
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
   import format must round-trip through the same `MazeData` serde types
   used by the wire protocol (add a round-trip test for the file format,
   same pattern as Milestone 2).
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
