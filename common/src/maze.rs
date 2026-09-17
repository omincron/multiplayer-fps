//! Maze data structure and generation (ARCHITECTURE.md §5).
//!
//! `generate` is a pure function: same `MazeSpec` in, byte-identical
//! `MazeData` out, on any machine. That determinism is what lets
//! `MazeSource::Generated` send only a ~24-byte seed over the wire instead
//! of the expanded grid (§5.1) — both sides call `generate` and agree.

use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// Bump on any change to `generate`'s output for a given `MazeSpec` — it is
/// checked against the peer's version alongside `PROTOCOL_VERSION` so a
/// generator drift fails loudly instead of two players walking around
/// different mazes (ARCHITECTURE.md §3.2, §5.2).
pub const GENERATOR_VERSION: u16 = 1;

pub const GENERATOR_NAME: &str = "recursive-backtracker+braid";

pub const WALL_N: u8 = 1;
pub const WALL_E: u8 = 2;
pub const WALL_S: u8 = 4;
pub const WALL_W: u8 = 8;
const ALL_WALLS: u8 = WALL_N | WALL_E | WALL_S | WALL_W;

/// What travels on the wire for a generated maze: ~24 bytes regardless of
/// maze size. The receiver calls `generate(spec)` to rebuild the grid
/// locally rather than receiving it (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MazeSpec {
    pub seed: u64,
    pub width: u16,
    pub height: u16,
    /// Probability, per dead-end cell, of opening it into a loop during the
    /// braiding pass. 0.0 = pure perfect maze (hardest). Near 1.0 = most
    /// dead ends opened (easiest). See `generate` for the algorithm.
    pub braid_factor: f32,
    pub generator_version: u16,
}

/// The grid itself: one `u8` bitmask per cell (`WALL_N`/`E`/`S`/`W`), row
/// major. Every interior wall is stored on both cells that share it —
/// nothing enforces that invariant structurally, so code that mutates walls
/// must mutate both sides (`walls_symmetric` checks it in tests).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MazeGrid {
    pub width: u16,
    pub height: u16,
    pub walls: Vec<u8>,
}

impl MazeGrid {
    pub fn index(&self, x: u16, y: u16) -> usize {
        y as usize * self.width as usize + x as usize
    }

    pub fn walls_at(&self, x: u16, y: u16) -> u8 {
        self.walls[self.index(x, y)]
    }

    pub fn cell_count(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// Where a maze's grid came from. Generated mazes regenerate from the seed
/// (§5.1); custom ones (the editor bonus, §7.3) carry the expanded grid
/// because they have no seed that could reproduce them. `common::protocol`
/// (Milestone 2) re-exports this type as the payload of
/// `ServerMsg::Welcome` / `EventKind::LevelChanged`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MazeSource {
    Generated(MazeSpec),
    Custom(MazeGrid),
}

/// What both client and server hold in memory: one representation used for
/// server-side collision, client-side raycasting, and the minimap alike
/// (§5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MazeData {
    pub grid: MazeGrid,
    pub source: MazeSource,
    pub generator_name: String,
}

/// Generate a maze from `spec`. Pure function: no I/O, no shared mutable
/// state, deterministic given the same input (§5.2 — see the
/// `determinism_same_spec_same_output` test below).
///
/// Algorithm: randomized recursive backtracker (produces a "perfect" maze —
/// exactly one path between any two cells), then a braiding pass that opens
/// some dead ends into loops. Braiding iterates *dead-end cells*
/// specifically, not every interior wall — removing a wall from every cell
/// independently at `braid_factor = 0.6` would delete ~60% of all walls and
/// produce an open room with scattered pillars, not a maze.
pub fn generate(spec: MazeSpec) -> MazeData {
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);
    let mut grid = carve_spanning_tree(spec.width.max(1), spec.height.max(1), &mut rng);
    braid(&mut grid, &mut rng, spec.braid_factor);
    MazeData {
        grid,
        source: MazeSource::Generated(spec),
        generator_name: GENERATOR_NAME.to_string(),
    }
}

fn carve_spanning_tree(width: u16, height: u16, rng: &mut ChaCha8Rng) -> MazeGrid {
    let cell_count = width as usize * height as usize;
    let mut walls = vec![ALL_WALLS; cell_count];
    let idx = |x: u16, y: u16| -> usize { y as usize * width as usize + x as usize };

    let mut visited = vec![false; cell_count];
    let mut stack: Vec<(u16, u16)> = Vec::new();
    visited[idx(0, 0)] = true;
    stack.push((0, 0));

    // (wall bit removed on the current cell, dx, dy, wall bit removed on the neighbour)
    const DIRS: [(u8, i32, i32, u8); 4] = [
        (WALL_N, 0, -1, WALL_S),
        (WALL_E, 1, 0, WALL_W),
        (WALL_S, 0, 1, WALL_N),
        (WALL_W, -1, 0, WALL_E),
    ];

    while let Some(&(cx, cy)) = stack.last() {
        let mut unvisited_neighbors: Vec<(u16, u16, u8, u8)> = Vec::new();
        for (cur_bit, dx, dy, nb_bit) in DIRS {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                continue;
            }
            let (nx, ny) = (nx as u16, ny as u16);
            if !visited[idx(nx, ny)] {
                unvisited_neighbors.push((nx, ny, cur_bit, nb_bit));
            }
        }

        if let Some(&(nx, ny, cur_bit, nb_bit)) = unvisited_neighbors.choose(rng) {
            walls[idx(cx, cy)] &= !cur_bit;
            walls[idx(nx, ny)] &= !nb_bit;
            visited[idx(nx, ny)] = true;
            stack.push((nx, ny));
        } else {
            stack.pop();
        }
    }

    MazeGrid {
        width,
        height,
        walls,
    }
}

/// Open some dead ends (cells with exactly 3 walls) into loops. Each
/// dead-end cell, visited in a seeded random order, has probability
/// `braid_factor` of losing one wall — chosen among its walls that lead to
/// an in-bounds neighbour, never a border wall (that would let a player
/// walk off the grid, breaking the border invariant). No connectivity guard
/// is needed: removing a wall can only ever increase connectivity.
fn braid(grid: &mut MazeGrid, rng: &mut ChaCha8Rng, braid_factor: f32) {
    let (width, height) = (grid.width, grid.height);
    let idx = |x: u16, y: u16| -> usize { y as usize * width as usize + x as usize };

    const DIRS: [(u8, i32, i32, u8); 4] = [
        (WALL_N, 0, -1, WALL_S),
        (WALL_E, 1, 0, WALL_W),
        (WALL_S, 0, 1, WALL_N),
        (WALL_W, -1, 0, WALL_E),
    ];

    let mut dead_ends: Vec<(u16, u16)> = Vec::new();
    for y in 0..height {
        for x in 0..width {
            if grid.walls[idx(x, y)].count_ones() == 3 {
                dead_ends.push((x, y));
            }
        }
    }
    dead_ends.shuffle(rng);

    for (x, y) in dead_ends {
        let i = idx(x, y);
        // A neighbouring dead end's braid step earlier in this same pass may
        // have already opened a wall into this cell.
        if grid.walls[i].count_ones() != 3 {
            continue;
        }
        if rng.r#gen::<f32>() >= braid_factor {
            continue;
        }

        let mut candidates: Vec<(u8, u16, u16, u8)> = Vec::new();
        for (bit, dx, dy, nb_bit) in DIRS {
            if grid.walls[i] & bit == 0 {
                continue; // this is the dead end's single opening
            }
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                continue; // border wall: never remove
            }
            candidates.push((bit, nx as u16, ny as u16, nb_bit));
        }

        if let Some(&(bit, nx, ny, nb_bit)) = candidates.choose(rng) {
            let ni = idx(nx, ny);
            grid.walls[i] &= !bit;
            grid.walls[ni] &= !nb_bit;
        }
    }
}

/// True if every cell is reachable from every other cell (§5.3 invariant 1).
///
/// Only exercised by tests today. Once the maze editor (Milestone 16, §7.3)
/// needs to validate a loaded `--maze` file against the same invariant
/// list, promote this to a real `pub` item called from that load path
/// instead of duplicating the flood fill there.
#[cfg(test)]
pub(crate) fn is_fully_connected(grid: &MazeGrid) -> bool {
    if grid.cell_count() == 0 {
        return true;
    }
    let (width, height) = (grid.width, grid.height);
    let idx = |x: u16, y: u16| -> usize { y as usize * width as usize + x as usize };
    let mut seen = vec![false; grid.cell_count()];
    let mut stack = vec![(0u16, 0u16)];
    seen[idx(0, 0)] = true;
    let mut visited_count = 1;

    const DIRS: [(u8, i32, i32); 4] = [(WALL_N, 0, -1), (WALL_E, 1, 0), (WALL_S, 0, 1), (WALL_W, -1, 0)];

    while let Some((cx, cy)) = stack.pop() {
        let cell_walls = grid.walls_at(cx, cy);
        for (bit, dx, dy) in DIRS {
            if cell_walls & bit != 0 {
                continue; // wall present, no passage
            }
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                continue;
            }
            let (nx, ny) = (nx as u16, ny as u16);
            let ni = idx(nx, ny);
            if !seen[ni] {
                seen[ni] = true;
                visited_count += 1;
                stack.push((nx, ny));
            }
        }
    }

    visited_count == grid.cell_count()
}

/// True if no cell is walled on all 4 sides (§5.3 invariant 2).
#[cfg(test)]
pub(crate) fn has_no_isolated_cell(grid: &MazeGrid) -> bool {
    grid.walls.iter().all(|&w| w != ALL_WALLS)
}

/// True if every border cell has its outer-facing wall(s) set (§5.3
/// invariant 3).
#[cfg(test)]
pub(crate) fn borders_are_closed(grid: &MazeGrid) -> bool {
    let (width, height) = (grid.width, grid.height);
    for x in 0..width {
        if grid.walls_at(x, 0) & WALL_N == 0 {
            return false;
        }
        if grid.walls_at(x, height - 1) & WALL_S == 0 {
            return false;
        }
    }
    for y in 0..height {
        if grid.walls_at(0, y) & WALL_W == 0 {
            return false;
        }
        if grid.walls_at(width - 1, y) & WALL_E == 0 {
            return false;
        }
    }
    true
}

/// True if every interior edge agrees between the two cells sharing it
/// (§5.3 invariant 4).
#[cfg(test)]
pub(crate) fn walls_are_symmetric(grid: &MazeGrid) -> bool {
    let (width, height) = (grid.width, grid.height);
    for y in 0..height {
        for x in 0..width {
            if x + 1 < width {
                let a_has_e = grid.walls_at(x, y) & WALL_E != 0;
                let b_has_w = grid.walls_at(x + 1, y) & WALL_W != 0;
                if a_has_e != b_has_w {
                    return false;
                }
            }
            if y + 1 < height {
                let a_has_s = grid.walls_at(x, y) & WALL_S != 0;
                let b_has_n = grid.walls_at(x, y + 1) & WALL_N != 0;
                if a_has_s != b_has_n {
                    return false;
                }
            }
        }
    }
    true
}

/// Dead ends per open cell (§5.3 invariant 5 — the density this project
/// uses as "difficulty", never a raw count: raw counts grow with area on
/// their own as the level table's grid size grows). `pub`, not test-only:
/// Milestone 14 reuses this exact function to check the real
/// `server::levels` table once it exists.
pub fn dead_end_density(grid: &MazeGrid) -> f64 {
    let dead_ends = grid.walls.iter().filter(|&&w| w.count_ones() == 3).count();
    dead_ends as f64 / grid.cell_count() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(seed: u64, width: u16, height: u16, braid_factor: f32) -> MazeSpec {
        MazeSpec {
            seed,
            width,
            height,
            braid_factor,
            generator_version: GENERATOR_VERSION,
        }
    }

    #[test]
    fn small_maze_is_connected_and_closed() {
        let data = generate(spec(1, 5, 5, 0.3));
        assert!(is_fully_connected(&data.grid));
        assert!(has_no_isolated_cell(&data.grid));
        assert!(borders_are_closed(&data.grid));
        assert!(walls_are_symmetric(&data.grid));
    }

    #[test]
    fn determinism_same_spec_same_output() {
        let a = generate(spec(42, 20, 15, 0.4));
        let b = generate(spec(42, 20, 15, 0.4));
        assert_eq!(a.grid, b.grid);
    }

    #[test]
    fn different_seed_different_maze() {
        let a = generate(spec(1, 20, 15, 0.4));
        let b = generate(spec(2, 20, 15, 0.4));
        assert_ne!(a.grid, b.grid);
    }

    #[test]
    fn braid_zero_is_a_perfect_maze() {
        // braid_factor = 0.0: no dead end should ever be opened, so the
        // dead-end count must equal what the bare spanning tree produced.
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let unbraided = carve_spanning_tree(12, 12, &mut rng);
        let unbraided_dead_ends = dead_end_density(&unbraided);

        let data = generate(spec(7, 12, 12, 0.0));
        assert_eq!(dead_end_density(&data.grid), unbraided_dead_ends);
    }

    #[test]
    fn dead_end_density_monotonic_fixed_size() {
        // Fixed grid size, varying braid_factor: density must strictly
        // increase as braid_factor drops. Stochastic per seed, so average
        // over many seeds rather than asserting on one lucky/unlucky trial.
        let factors = [0.9_f32, 0.5, 0.1];
        let seeds = 0..40u64;
        let mut avg_density = Vec::new();
        for &factor in &factors {
            let total: f64 = seeds
                .clone()
                .map(|seed| dead_end_density(&generate(spec(seed, 20, 20, factor)).grid))
                .sum();
            avg_density.push(total / seeds.clone().count() as f64);
        }
        assert!(
            avg_density[0] < avg_density[1] && avg_density[1] < avg_density[2],
            "expected strictly increasing density as braid_factor drops, got {:?} for factors {:?}",
            avg_density,
            factors
        );
    }

    #[test]
    fn dead_end_density_monotonic_across_placeholder_level_table() {
        // Placeholder stand-in for server::levels (built in Milestone 14):
        // grid area increases, braid_factor decreases, level over level.
        // Density (not raw count, §5.3 invariant 5) must still increase
        // even though area is changing at the same time.
        let table = [(20u16, 20u16, 0.6_f32), (30, 30, 0.3), (40, 40, 0.05)];
        let seeds = 0..40u64;
        let mut avg_density = Vec::new();
        for &(w, h, factor) in &table {
            let total: f64 = seeds
                .clone()
                .map(|seed| dead_end_density(&generate(spec(seed, w, h, factor)).grid))
                .sum();
            avg_density.push(total / seeds.clone().count() as f64);
        }
        assert!(
            avg_density[0] < avg_density[1] && avg_density[1] < avg_density[2],
            "expected strictly increasing density across level table, got {:?}",
            avg_density
        );
    }

    proptest::proptest! {
        #[test]
        fn connectivity_and_invariants_hold(
            seed in proptest::prelude::any::<u64>(),
            width in 2u16..40,
            height in 2u16..40,
            braid_factor in 0.0f32..1.0,
        ) {
            let data = generate(spec(seed, width, height, braid_factor));
            proptest::prop_assert!(is_fully_connected(&data.grid));
            proptest::prop_assert!(has_no_isolated_cell(&data.grid));
            proptest::prop_assert!(borders_are_closed(&data.grid));
            proptest::prop_assert!(walls_are_symmetric(&data.grid));
        }
    }
}
