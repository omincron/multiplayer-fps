//! Movement and collision resolution (ARCHITECTURE.md §4.3). Pure
//! functions of their inputs — no RNG, no clock — which is what lets the
//! server's authoritative simulation and the client's local prediction
//! (§6.3) call the exact same function and always agree.

use crate::maze::{MazeGrid, WALL_E, WALL_N, WALL_S, WALL_W};
use crate::types::Vec2;

/// Distance a single collision substep may cover before re-checking
/// against the walls. Small enough that a step never tunnels through a
/// wall even at large `dt_s` — some tests deliberately use a `dt_s` many
/// times a real tick's to check the dt-scaling property in one call.
const MAX_SUBSTEP: f32 = 0.02;

/// One fixed simulation step: `pos` after moving in `move_dir` for
/// `dt_s` seconds at `speed` units/sec, resolved against `maze`'s walls
/// with a circle of `radius`, sliding along a wall instead of stopping
/// dead on any collision.
///
/// `move_dir` is a *direction*, not a displacement: it is clamped to at
/// most unit length inside this function (never trusted as-is from the
/// wire, §4.3), and `speed` is always supplied by the caller — the server
/// owns `PLAYER_SPEED_UPS`, so a client cannot buy extra speed by scaling
/// up its own `move_dir`.
pub fn resolve_move(
    maze: &MazeGrid,
    pos: Vec2,
    move_dir: Vec2,
    speed: f32,
    dt_s: f32,
    radius: f32,
) -> Vec2 {
    let dir = clamp_to_unit_length(move_dir);
    let dx = dir.x * speed * dt_s;
    let dy = dir.y * speed * dt_s;

    // Axis-separated resolution: resolve X, then Y from the (possibly
    // X-blocked) result. This is what produces sliding for free — a
    // diagonal move blocked on one axis still applies the other, instead
    // of a naive full-stop-on-any-collision.
    let after_x = move_axis(maze, pos, radius, dx, 0.0);
    move_axis(maze, after_x, radius, 0.0, dy)
}

fn clamp_to_unit_length(v: Vec2) -> Vec2 {
    let len = (v.x * v.x + v.y * v.y).sqrt();
    if len > 1.0 {
        Vec2::new(v.x / len, v.y / len)
    } else {
        v
    }
}

/// Moves `pos` by `(dx, dy)` in small substeps, stopping at the first
/// substep that would cross a wall. Called once with `dy = 0.0` and once
/// with `dx = 0.0` by `resolve_move` — a single shared function rather
/// than near-identical `move_axis_x`/`move_axis_y` copies.
fn move_axis(maze: &MazeGrid, pos: Vec2, radius: f32, dx: f32, dy: f32) -> Vec2 {
    let distance = dx.abs().max(dy.abs());
    if distance == 0.0 {
        return pos;
    }
    let steps = ((distance / MAX_SUBSTEP).ceil() as u32).max(1);
    let (step_x, step_y) = (dx / steps as f32, dy / steps as f32);
    let mut p = pos;
    for _ in 0..steps {
        let candidate = Vec2::new(p.x + step_x, p.y + step_y);
        if circle_fits(maze, candidate, radius) {
            p = candidate;
        } else {
            break;
        }
    }
    p
}

/// True if a circle of `radius` centered at `pos` does not cross a wall of
/// the cell it's in. Only checks the current cell's own four edges —
/// sufficient because `move_axis` only ever advances `MAX_SUBSTEP` at a
/// time and `radius` is always well under half a cell, so the circle never
/// reaches far enough to be blocked by a cell it isn't currently touching.
fn circle_fits(maze: &MazeGrid, pos: Vec2, radius: f32) -> bool {
    if pos.x < 0.0 || pos.y < 0.0 {
        return false;
    }
    let cx = pos.x as i32;
    let cy = pos.y as i32;
    if cx >= maze.width as i32 || cy >= maze.height as i32 {
        return false;
    }
    let walls = maze.walls_at(cx as u16, cy as u16);
    let local_x = pos.x - cx as f32;
    let local_y = pos.y - cy as f32;

    if walls & WALL_W != 0 && local_x - radius < 0.0 {
        return false;
    }
    if walls & WALL_E != 0 && local_x + radius > 1.0 {
        return false;
    }
    if walls & WALL_N != 0 && local_y - radius < 0.0 {
        return false;
    }
    if walls & WALL_S != 0 && local_y + radius > 1.0 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 0.03; // one MAX_SUBSTEP of slack, plus margin

    /// All walls closed everywhere; tests open specific edges from this.
    fn walled_grid(width: u16, height: u16) -> MazeGrid {
        MazeGrid {
            width,
            height,
            walls: vec![WALL_N | WALL_E | WALL_S | WALL_W; width as usize * height as usize],
        }
    }

    /// Opens the shared edge between two orthogonally-adjacent cells,
    /// clearing both mirrored bits (maze.rs §5.1's symmetry invariant) —
    /// this collision code only ever reads the departing cell's own bit,
    /// but keeping both sides consistent matches what `generate()` would
    /// actually produce and avoids a test grid that's a bad model of a
    /// real one.
    fn open(grid: &mut MazeGrid, x1: u16, y1: u16, x2: u16, y2: u16) {
        let i1 = grid.index(x1, y1);
        let i2 = grid.index(x2, y2);
        if x2 == x1 + 1 && y2 == y1 {
            grid.walls[i1] &= !WALL_E;
            grid.walls[i2] &= !WALL_W;
        } else if x1 == x2 + 1 && y2 == y1 {
            grid.walls[i1] &= !WALL_W;
            grid.walls[i2] &= !WALL_E;
        } else if y2 == y1 + 1 && x2 == x1 {
            grid.walls[i1] &= !WALL_S;
            grid.walls[i2] &= !WALL_N;
        } else if y1 == y2 + 1 && x2 == x1 {
            grid.walls[i1] &= !WALL_N;
            grid.walls[i2] &= !WALL_S;
        } else {
            panic!("open() called on non-adjacent cells");
        }
    }

    #[test]
    fn stops_at_end_wall_not_past_or_short() {
        // 3-cell horizontal corridor, open internally, closed at both ends.
        let mut grid = walled_grid(3, 1);
        open(&mut grid, 0, 0, 1, 0);
        open(&mut grid, 1, 0, 2, 0);

        let radius = 0.2;
        let start = Vec2::new(0.5, 0.5);
        // Huge dt_s: guarantees reaching the far wall regardless of speed.
        let end = resolve_move(&grid, start, Vec2::new(1.0, 0.0), 3.0, 10.0, radius);

        let expected_x = 3.0 - radius; // circle edge touches the east wall of cell 2
        assert!(
            (end.x - expected_x).abs() < EPS,
            "expected x ~= {expected_x}, got {}",
            end.x
        );
        assert!(end.x <= expected_x + EPS, "moved past the wall: {}", end.x);
        assert!((end.y - 0.5).abs() < 1e-4, "unexpected y drift: {}", end.y);
    }

    #[test]
    fn diagonal_into_corner_still_slides() {
        // Cell (0,0): East wall closed (blocks x), South open (y is free).
        let mut grid = walled_grid(2, 2);
        open(&mut grid, 0, 0, 0, 1); // south open
        // East wall of (0,0) stays closed: default from walled_grid.

        let radius = 0.2;
        let start = Vec2::new(0.5, 0.2);
        let dt_s = 0.5;
        let speed = 1.0;
        let dir = Vec2::new(1.0, 1.0); // diagonal into the closed east wall

        let end = resolve_move(&grid, start, dir, speed, dt_s, radius);

        // X should be stopped at the wall, not free-flowing.
        let expected_x = 1.0 - radius;
        assert!(
            (end.x - expected_x).abs() < EPS,
            "expected x ~= {expected_x}, got {}",
            end.x
        );

        // Y must still have made forward progress (not a full stop): the
        // clamped diagonal direction has unit length, so its y component
        // is 1/sqrt(2); a naive full-stop-on-any-collision implementation
        // would leave y unchanged at 0.2, which this rejects.
        let clamped_dy = (1.0f32 / 2.0f32.sqrt()) * speed * dt_s;
        let expected_y = start.y + clamped_dy;
        assert!(
            (end.y - expected_y).abs() < EPS,
            "expected y ~= {expected_y} (unobstructed slide), got {}",
            end.y
        );
        assert!(
            end.y > start.y + EPS,
            "diagonal collision produced a full stop instead of a slide"
        );
    }

    #[test]
    fn doorway_at_radius_boundary() {
        // Single-cell-wide vertical corridor (column x in [2,3)), 4 cells
        // tall, side walls (W/E) closed the whole way down, open N-S
        // internally.
        let mut grid = walled_grid(5, 4);
        for y in 0..3u16 {
            open(&mut grid, 2, y, 2, y + 1);
        }

        let start = Vec2::new(2.5, 0.5); // perfectly centered
        let dt_s = 1.0;
        let speed = 3.0; // covers the whole corridor in one call

        // Just under half the corridor width: must pass straight through.
        let fits = resolve_move(&grid, start, Vec2::new(0.0, 1.0), speed, dt_s, 0.49);
        assert!(
            fits.y > start.y + 1.0,
            "radius just under the boundary should not be blocked, got y = {}",
            fits.y
        );

        // Just over half the corridor width: must be blocked immediately
        // by the side walls, even though the doorway ahead is open.
        let blocked = resolve_move(&grid, start, Vec2::new(0.0, 1.0), speed, dt_s, 0.51);
        assert!(
            (blocked.y - start.y).abs() < EPS,
            "radius just over the boundary should be blocked at the start, got y = {}",
            blocked.y
        );
    }

    #[test]
    fn double_dt_doubles_displacement_in_open_space() {
        // Clear every internal wall so the whole grid is open space.
        let mut grid = walled_grid(20, 20);
        for y in 0..20u16 {
            for x in 0..19u16 {
                open(&mut grid, x, y, x + 1, y);
            }
        }
        for x in 0..20u16 {
            for y in 0..19u16 {
                open(&mut grid, x, y, x, y + 1);
            }
        }

        let start = Vec2::new(10.0, 10.0);
        let dir = Vec2::new(1.0, 0.0);
        let speed = 3.0;
        let radius = 0.2;

        let once = resolve_move(&grid, start, dir, speed, 0.1, radius);
        let twice = resolve_move(&grid, start, dir, speed, 0.2, radius);

        let disp_once = once.x - start.x;
        let disp_twice = twice.x - start.x;
        assert!(
            disp_once > 0.1,
            "expected real movement, got near-zero displacement {disp_once} (stub returning pos unchanged would pass the doubling check below vacuously)"
        );
        assert!(
            (disp_twice - 2.0 * disp_once).abs() < 1e-3,
            "expected double displacement, got {disp_once} then {disp_twice}"
        );
    }

    #[test]
    fn oversized_move_dir_is_clamped_to_unit_length() {
        let mut grid = walled_grid(10, 10);
        for y in 0..10u16 {
            for x in 0..9u16 {
                open(&mut grid, x, y, x + 1, y);
            }
        }
        for x in 0..10u16 {
            for y in 0..9u16 {
                open(&mut grid, x, y, x, y + 1);
            }
        }

        let start = Vec2::new(5.0, 5.0);
        let speed = 3.0;
        let dt_s = 0.1;
        let radius = 0.2;

        let unit = resolve_move(&grid, start, Vec2::new(1.0, 0.0), speed, dt_s, radius);
        let huge = resolve_move(&grid, start, Vec2::new(100.0, 0.0), speed, dt_s, radius);

        assert!(
            unit.x > start.x + 0.1,
            "expected real movement, got near-zero displacement (stub returning pos unchanged would pass the equality check below vacuously)"
        );
        assert!(
            (unit.x - huge.x).abs() < 1e-3 && (unit.y - huge.y).abs() < 1e-3,
            "a magnitude-100 move_dir should displace identically to a unit one: {unit:?} vs {huge:?}"
        );
    }
}
