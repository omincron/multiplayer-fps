# Visual polish TODO (`player-sprites` branch)

Not part of the audit checklist — this branch is cosmetic polish on top
of the complete, gated core game on `develop`. Read this before picking
the work back up; it's written so you don't have to reconstruct context.

## Where things stand

**Done:**
- First-person player sprites (`client::render::sprites` +
  `main.rs`'s `draw_player_sprites`). Remote players render as
  billboards in the 3D view, not just the minimap, correctly occluded
  behind nearer walls using the same per-column depth buffer the wall
  renderer builds. Currently a **flat colored rectangle**, not a real
  image — visually confirmed working (correct occlusion, correct
  distance scaling) but plain-looking.
- CC0 art assets downloaded and committed under `client/assets/`
  (sources + license in `client/assets/ATTRIBUTION.md`):
  - `sprites/enemy.png` — 62x94px, RGBA with real transparency, a single
    cropped front-facing frame. Not wired into rendering yet.
  - `sprites/soldier_sheet.png` — the full source sheet `enemy.png` was
    cropped from. Multi-pose, multi-angle, two color variants (purple
    and orange/pink), packed in an irregular grid (not a clean uniform
    NxM layout — see "extracting more frames" below if you want more
    than the one static pose).
  - `textures/brick_001.png` (red brick), `textures/brick_002.png`
    (light gray brick), `textures/stone_wall.jpg` (dark rough stone) —
    all 512x512, seamless/tileable. None used yet; walls still render
    as flat shaded colors.

**Not done:** actually drawing any of the above. Both pieces below are
independent of each other — do either first, or just one.

## 1. Swap the sprite rectangle for `enemy.png` (the easier piece)

`main.rs`'s `draw_player_sprites` currently does, per screen column in
the sprite's range:
```rust
draw_rectangle(column as f32, top_y, 1.1, sprite_height, color);
```
Replace with a `Texture2D` slice draw. Steps:
1. Load the texture once, before the render loop starts (not per
   frame) — `macroquad::texture::load_texture(path).await`. It has to
   happen inside the async context `Window::from_config` already gives
   you (i.e. near the top of `run_window`, alongside where `predictor`/
   `remote_players` etc. get initialized), not in `main()`/`start()`
   before the window exists.
2. Path robustness: don't assume the process's CWD — use
   `concat!(env!("CARGO_MANIFEST_DIR"), "/assets/sprites/enemy.png")`
   (same pattern `server/tests/tick_loop.rs` already uses for its log
   file path) so it works regardless of where `cargo run` is invoked
   from.
3. Per column in the sprite's range (the existing `for column in
   range` loop), instead of one solid color for the whole strip, slice
   a 1px-wide vertical strip of source pixels and stretch it to the
   destination rectangle:
   ```rust
   let u = (column - range.start) as f32 / range.len().max(1) as f32;
   let source_x = u * texture.width();
   draw_texture_ex(&texture, column as f32, top_y, tint_color, DrawTextureParams {
       dest_size: Some(vec2(1.1, sprite_height)),
       source: Some(Rect { x: source_x, y: 0.0, w: 1.0, h: texture.height() }),
       ..Default::default()
   });
   ```
4. Keep the existing distance-based `brightness` calculation — pass it
   as the tint `Color` instead of baking it into a solid fill color;
   `draw_texture_ex` multiplies the texture by the tint, so the same
   darken-with-distance effect carries over for free.
5. Transparency: `enemy.png` already has a real alpha channel (checked
   with PIL — `getbbox()` correctly found the figure's silhouette), so
   columns/pixels outside the figure should just not draw — no extra
   masking code needed, macroquad respects the source alpha.

## 2. Textured walls (the bigger piece)

This needs one new piece of math that doesn't exist yet: **where along
the wall, not just how far away, each ray hit** — a 0.0–1.0 fraction
across the wall segment's width, used to pick a texture column.

1. In `client::render::raycast::cast_ray` (`client/src/render/raycast.rs`):
   the DDA loop already knows, at the moment it registers a hit,
   which axis was stepped (`side: Vertical` vs `Horizontal`) and the
   hit `distance`. Compute the exact hit point:
   `hit_point = origin + direction * distance`, then:
   - `Vertical` hit (ray crossed an E/W wall): `wall_u = hit_point.y.fract()`
   - `Horizontal` hit (crossed a N/S wall): `wall_u = hit_point.x.fract()`
   Watch the direction of travel — depending on which side of the cell
   was entered from, you may need `1.0 - fract` to keep textures from
   mirroring when walking past a wall from the opposite direction (this
   is the classic raycaster texturing gotcha — Lode's raycasting
   tutorial covers it well if you want a reference implementation to
   check against). Add `wall_u: f32` to `RayHit` and thread it through
   to `ViewColumn` in `cast_view`.
2. **Test this like the rest of the raycaster** (this codebase's whole
   testing culture is "hand-compute the expected value for a known
   case"): pick a tiny hand-built grid and a ray you can compute the
   exact hit point for by hand, assert `wall_u` matches. The existing
   tests in `raycast.rs` (`east_ray_hits_known_internal_wall_half_a_cell_away`
   etc.) are the pattern to copy.
3. In `main.rs`'s `draw_maze_view`, load `brick_001.png` (or pick a
   texture per wall `side`/per-cell for variety — `brick_002.png` and
   `stone_wall.jpg` are both already downloaded for exactly this) the
   same way as the sprite texture (once, at startup, `CARGO_MANIFEST_DIR`-
   relative path). Replace the flat `draw_rectangle(... color)` call
   with a `draw_texture_ex` slice using `wall_u` to pick the source
   column, same technique as the sprite slicing above, keeping the
   existing `brightness`/`face_shade` distance-and-orientation shading
   as the tint color.
4. `stone_wall.jpg` has no alpha channel (it's a JPEG) — that's fine
   for walls (no transparency needed there), just don't reuse the same
   texture-loading code path unmodified if it assumes RGBA/PNG
   somewhere.

## Extracting more sprite frames later (optional, bigger job)

If you want more than one static pose (e.g. facing-direction-dependent
sprites, or a death/idle animation), the sheet's layout isn't a clean
grid — different rows have different cell widths. The way `enemy.png`
was found: load the sheet with PIL, scan each row's alpha channel for
column runs (contiguous non-transparent spans) to get per-frame
bounding boxes, crop candidates, and visually check them one at a time.
That whole exploration (row-height guess, column-run detection script)
isn't saved anywhere — you'd redo it, but it only took a few minutes
once row height was guessed correctly (1024px / 8 rows = 128px worked
cleanly). `soldier_sheet.png` is still in the repo for exactly this.

## Everything else about this branch

- Base it stays off `develop` — don't merge into `develop`, per the
  earlier decision that visual polish isn't part of the audit and
  shouldn't touch the gated core game state.
- No new tests are needed for `main.rs`'s drawing calls themselves
  (rendering output isn't unit-testable, same reasoning
  `ARCHITECTURE.md` §8.4 already gives for the raycaster/minimap: test
  the *math*, eyeball the pixels) — but the new `wall_u` math in
  `raycast.rs` absolutely should get the hand-computed-value treatment
  like everything else in that file.
- After either piece: `cargo clean && cargo build --workspace` should
  stay at zero warnings, and a real two-client manual check (same
  pattern used throughout this project — one server, two clients, look
  at each other) is the actual verification, since none of this is
  unit-testable at the pixel level.
