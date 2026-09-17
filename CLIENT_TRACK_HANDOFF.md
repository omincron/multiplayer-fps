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

## Rendezvous points

Three planned sync-ups with `server-track`, not a free-for-all merge
whenever convenient:

1. **Lightweight checkpoint, soon.** Once `server-track` finishes
   Milestone 7 (tick loop + movement, including the load test) and
   `client-track` finishes the automatable parts of Milestone 8 (CLI
   prompts, handshake retry logic, fps averaging — none of which need a
   live server), do a quick joint check: a real client actually connecting
   to a real server. That's Milestone 8's own manual gate. ~15 minutes,
   not a working session. (Status as of 2026-09-17: `server-track` has
   completed Milestone 6; Milestone 7 is next.)
2. **Mandatory integration sync, before Milestone 13.** `client-track`
   should independently finish Milestones 9–12 (raycasting render,
   prediction, minimap, interpolation) against the static/self-generated
   maze and fake data first — none of that needs the server further along.
   Milestones 13 (shooting/health/respawn/kill-feed) and 14 (level
   progression) are genuinely joint: server wires `raycast_hit` and the
   level table, client wires the HUD reaction and `LevelChanged` handling,
   and neither side can finish either milestone alone. Treat 13+14 as one
   combined integration session.
3. **Final rendezvous, everyone.** Milestone 15, the pre-submission
   load/soak dress rehearsal (10+ clients, 3 minutes, fps logged). Needs a
   fully working client and server together regardless of how the earlier
   work split up.

So: `server-track` completes through Milestone 7, `client-track` completes
through Milestone 12, before the mandatory Milestone 13/14 sync — that's
the one to actually plan a joint session around. After Milestone 15,
bonus features (Milestone 16+) can split again (bots → server, editor/
launcher → client, procedural levels already covered by 14) with only a
final joint smoke-test before submission.

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

## Naming gotcha

`common::sim::raycast_hit` (Milestone 5, already built) is the
*server-authoritative* hit-detection raycast — it is not what
Milestone 9's `client::render::raycast` should reuse. The client's
raycaster is a separate DDA implementation for rendering (per-column view
distances for the whole screen), a different shape even though
conceptually similar (both are grid DDA raycasts). Don't let an agent try
to repurpose `sim::raycast_hit` for rendering.
