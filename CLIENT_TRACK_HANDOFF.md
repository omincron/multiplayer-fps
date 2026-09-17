# client-track Handoff Briefing

Prepared 2026-09-17. Everything the second developer (and their AI agent)
needs before picking up `client-track` on this project.

## Setup

- `git fetch && git checkout client-track` — already branched off `develop`
  at the Milestone 5 + CI commit, and pushed to `origin`
  (`github.com/omincron/multiplayer-fps`).
- Read `ARCHITECTURE.md` in full first, then `PLAN.md` — especially the
  "Running log" at the bottom, which has detailed notes for every milestone
  completed so far: design decisions, gotchas, and exactly what was
  verified. Treat that log as required reading for their agent, not
  optional context.

## Where to start

Milestone 8 (CLI prompts + connect handshake + window). Milestones 0-5 —
the `common` crate: maze generation, wire protocol, reliability layer,
movement/collision, shooting — are done, gated (zero warnings, all tests
green), and frozen as the contract both tracks build against.

## The one hard rule

Don't modify anything under `common/` from `client-track`. If a Milestone
8+ need reveals that `common` has to change (a new field, a wrong
constant, etc.), that change goes on `develop` directly, gets logged in
`PLAN.md`'s running log, and `server-track` rebases onto it and reruns
`cargo test --workspace` — and the same applies in reverse if
`server-track` touches `common` first. This is the coordination point
agreed on when the two tracks were split.

## CI is live

Every push (any branch) and every PR into `develop`/`main` runs
`cargo build --workspace` + `cargo test --workspace` with
`RUSTFLAGS="-D warnings"` — a warning now fails the build outright, not
just a manual grep.

## Two things that aren't cleanly separable

- **Milestone 13** ("shooting, health, respawn, kill feed end-to-end") is
  joint: it needs server-side `raycast_hit` wiring *and* client-side HUD
  reaction together. Same for the level-progression parts of
  **Milestone 14** (`LevelChanged` handling). These two milestones need a
  short sync between tracks rather than being purely independent.
- Milestone 8's own manual gate ("connect a real client to a real server")
  needs `server-track` to have reached Milestone 7 first. The automated
  parts of Milestone 8 — CLI prompt logic, fps-averaging unit test,
  connect-handshake test against a scripted fake responder — don't need a
  real server at all, so `client-track` can build and gate most of
  Milestone 8 without waiting. Only the final manual check is blocked on
  server progress.

## Naming gotcha

`common::sim::raycast_hit` (Milestone 5, already built) is the
*server-authoritative* hit-detection raycast — it is not what
Milestone 9's `client::render::raycast` should reuse. The client's
raycaster is a separate DDA implementation for rendering (per-column view
distances for the whole screen), a different shape even though
conceptually similar (both are grid DDA raycasts). Don't let an agent try
to repurpose `sim::raycast_hit` for rendering.
