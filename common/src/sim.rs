//! Movement, collision, and shooting resolution (ARCHITECTURE.md §4.3).
//! Pure functions of their inputs — no RNG, no clock — which is what lets
//! the server's authoritative simulation and the client's local prediction
//! (§6.3) call the exact same function and always agree.

use crate::maze::{MazeGrid, WALL_E, WALL_N, WALL_S, WALL_W};
use crate::types::{PlayerId, Vec2};

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

/// What a shot hit, and how far away (ARCHITECTURE.md §4.3: shooting is
/// instant-hit, first collision — wall or player — wins).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RaycastHit {
    Wall { distance: f32 },
    Player { id: PlayerId, distance: f32 },
}

/// Instant-hit raycast from `origin` along `facing` (radians), against
/// `maze`'s walls and `players`. `players` is the *candidate* target list —
/// the caller excludes the shooter themselves before calling this, since
/// self-exclusion is a game-rule decision, not a geometry one. Returns
/// `None` on a clean miss (nothing within `max_range`).
pub fn raycast_hit(
    maze: &MazeGrid,
    origin: Vec2,
    facing: f32,
    players: &[(PlayerId, Vec2)],
    player_radius: f32,
    max_range: f32,
) -> Option<RaycastHit> {
    let dir = Vec2::new(facing.cos(), facing.sin());
    let wall_distance = raycast_wall_distance(maze, origin, dir, max_range);

    let mut closest_player: Option<(PlayerId, f32)> = None;
    for &(id, pos) in players {
        let Some(dist) = ray_circle_distance(origin, dir, pos, player_radius) else {
            continue;
        };
        // A player at or beyond the wall distance is occluded — the wall
        // was hit first, so this player is never reached.
        if dist >= wall_distance {
            continue;
        }
        if closest_player.is_none_or(|(_, best)| dist < best) {
            closest_player = Some((id, dist));
        }
    }

    if let Some((id, distance)) = closest_player {
        return Some(RaycastHit::Player { id, distance });
    }
    if wall_distance < max_range {
        return Some(RaycastHit::Wall {
            distance: wall_distance,
        });
    }
    None
}

/// DDA grid raycast: distance from `origin` to the first wall crossed
/// travelling along unit vector `dir`, capped at `max_range` (returned
/// unchanged if no wall is found within range — the caller's signal for
/// "no wall hit").
fn raycast_wall_distance(maze: &MazeGrid, origin: Vec2, dir: Vec2, max_range: f32) -> f32 {
    let mut cx = origin.x.floor() as i32;
    let mut cy = origin.y.floor() as i32;

    let step_x: i32 = if dir.x > 0.0 {
        1
    } else if dir.x < 0.0 {
        -1
    } else {
        0
    };
    let step_y: i32 = if dir.y > 0.0 {
        1
    } else if dir.y < 0.0 {
        -1
    } else {
        0
    };

    let delta_dist_x = if dir.x != 0.0 {
        (1.0 / dir.x).abs()
    } else {
        f32::INFINITY
    };
    let delta_dist_y = if dir.y != 0.0 {
        (1.0 / dir.y).abs()
    } else {
        f32::INFINITY
    };

    let mut side_dist_x = if dir.x > 0.0 {
        (cx as f32 + 1.0 - origin.x) * delta_dist_x
    } else if dir.x < 0.0 {
        (origin.x - cx as f32) * delta_dist_x
    } else {
        f32::INFINITY
    };
    let mut side_dist_y = if dir.y > 0.0 {
        (cy as f32 + 1.0 - origin.y) * delta_dist_y
    } else if dir.y < 0.0 {
        (origin.y - cy as f32) * delta_dist_y
    } else {
        f32::INFINITY
    };

    loop {
        // Borders are always walled (§5.3 invariant 3), so a well-formed
        // maze never actually lets the ray leave the grid — this is a
        // defensive fallback, not a path real mazes take.
        if cx < 0 || cy < 0 || cx >= maze.width as i32 || cy >= maze.height as i32 {
            return max_range;
        }

        let walls = maze.walls_at(cx as u16, cy as u16);

        if side_dist_x < side_dist_y {
            if side_dist_x > max_range {
                return max_range;
            }
            let wall_bit = if step_x > 0 { WALL_E } else { WALL_W };
            if walls & wall_bit != 0 {
                return side_dist_x;
            }
            side_dist_x += delta_dist_x;
            cx += step_x;
        } else {
            if side_dist_y > max_range {
                return max_range;
            }
            let wall_bit = if step_y > 0 { WALL_S } else { WALL_N };
            if walls & wall_bit != 0 {
                return side_dist_y;
            }
            side_dist_y += delta_dist_y;
            cy += step_y;
        }
    }
}

/// Distance along the ray (`origin` + t·`dir`, `dir` unit length) to the
/// nearest point where it enters the circle at `center` with `radius`, or
/// `None` if the ray never comes within `radius` of it.
fn ray_circle_distance(origin: Vec2, dir: Vec2, center: Vec2, radius: f32) -> Option<f32> {
    let oc = Vec2::new(origin.x - center.x, origin.y - center.y);
    let b = oc.x * dir.x + oc.y * dir.y;
    let c = oc.x * oc.x + oc.y * oc.y - radius * radius;
    let discriminant = b * b - c;
    if discriminant < 0.0 {
        return None;
    }
    let sqrt_d = discriminant.sqrt();
    let t1 = -b - sqrt_d;
    let t2 = -b + sqrt_d;
    if t1 >= 0.0 {
        Some(t1)
    } else if t2 >= 0.0 {
        Some(t2)
    } else {
        None
    }
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

    // --- raycast_hit (Milestone 5, ARCHITECTURE.md §4.3) ------------------

    /// 10-cell horizontal corridor. `blocked_after`, if given, closes the
    /// edge between that cell and the next one (both directions stay
    /// closed beyond it, but the shorter side stays open) — everything
    /// else internal is open; both ends are closed (from `walled_grid`).
    fn corridor(blocked_after: Option<u16>) -> MazeGrid {
        let mut grid = walled_grid(10, 1);
        for x in 0..9u16 {
            if Some(x) != blocked_after {
                open(&mut grid, x, 0, x + 1, 0);
            }
        }
        grid
    }

    const FACING_EAST: f32 = 0.0;

    #[test]
    fn wall_only_scene_hits_correct_wall_at_correct_distance() {
        let grid = corridor(None);
        let origin = Vec2::new(0.5, 0.5);

        let hit = raycast_hit(&grid, origin, FACING_EAST, &[], 0.3, 20.0);

        match hit {
            Some(RaycastHit::Wall { distance }) => {
                assert!(
                    (distance - 9.5).abs() < 1e-3,
                    "expected the east border wall at distance 9.5, got {distance}"
                );
            }
            other => panic!("expected a wall hit, got {other:?}"),
        }
    }

    #[test]
    fn player_closer_than_wall_is_reported_hit() {
        let grid = corridor(None);
        let origin = Vec2::new(0.5, 0.5);
        let target_id: PlayerId = 7;
        let players = [(target_id, Vec2::new(5.0, 0.5))];

        let hit = raycast_hit(&grid, origin, FACING_EAST, &players, 0.3, 20.0);

        match hit {
            Some(RaycastHit::Player { id, distance }) => {
                assert_eq!(id, target_id);
                // Near edge of the player's circle: (5.0 - 0.3) - 0.5 = 4.2
                assert!(
                    (distance - 4.2).abs() < 1e-3,
                    "expected player hit at distance 4.2, got {distance}"
                );
            }
            other => panic!("expected a player hit, got {other:?}"),
        }
    }

    #[test]
    fn wall_closer_than_player_wins_and_occludes_them() {
        // Wall closes the corridor between cell 3 and cell 4; the player
        // sits well beyond it, on the far side.
        let grid = corridor(Some(3));
        let origin = Vec2::new(0.5, 0.5);
        let hidden_id: PlayerId = 9;
        let players = [(hidden_id, Vec2::new(7.0, 0.5))];

        let hit = raycast_hit(&grid, origin, FACING_EAST, &players, 0.3, 20.0);

        match hit {
            Some(RaycastHit::Wall { distance }) => {
                assert!(
                    (distance - 3.5).abs() < 1e-3,
                    "expected the blocking wall at distance 3.5, got {distance}"
                );
            }
            other => panic!(
                "expected the wall to win and occlude the player behind it, got {other:?}"
            ),
        }
    }

    #[test]
    fn clean_miss_when_nothing_within_range() {
        let grid = corridor(None); // fully open, no obstruction
        let origin = Vec2::new(0.5, 0.5);

        // max_range well short of the far wall (at distance 9.5) and no
        // players anywhere near the ray.
        let hit = raycast_hit(&grid, origin, FACING_EAST, &[], 0.3, 5.0);

        assert_eq!(hit, None, "expected a clean miss, got {hit:?}");
    }
}
