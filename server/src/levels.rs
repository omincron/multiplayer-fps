//! Level/difficulty table (ARCHITECTURE.md §4.4): strictly increasing
//! grid area, strictly decreasing `braid_factor` — fewer loops means more
//! dead ends means harder navigation, per §5.2's own explanation of why
//! `braid_factor` is the difficulty knob.
//!
//! **Keep in sync by hand**: `common/src/maze.rs`'s
//! `dead_end_density_ordering_holds_across_the_real_level_table` test
//! hardcodes these same three `(width, height, braid_factor)` triples to
//! validate the density-ordering invariant against the real table, per
//! PLAN.md's Milestone 14 instruction to replace Milestone 1's placeholder
//! table with the real one. It can't import this array directly —
//! `common` has zero dependencies on `server` (ARCHITECTURE.md §2) — so a
//! change here must be mirrored there.

pub struct Level {
    pub grid_w: u16,
    pub grid_h: u16,
    pub braid_factor: f32,
    pub name: &'static str,
}

pub const LEVELS: &[Level] = &[
    Level {
        grid_w: 20,
        grid_h: 20,
        braid_factor: 0.6,
        name: "Novice Maze",
    },
    Level {
        grid_w: 30,
        grid_h: 30,
        braid_factor: 0.3,
        name: "Adept Maze",
    },
    Level {
        grid_w: 40,
        grid_h: 40,
        braid_factor: 0.05,
        name: "Veteran Maze",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_least_three_levels() {
        assert!(LEVELS.len() >= 3, "ARCHITECTURE.md §1 requires at least 3 difficulty levels");
    }

    #[test]
    fn area_strictly_increases_and_braid_factor_strictly_decreases() {
        for pair in LEVELS.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            assert!(
                (a.grid_w as u32 * a.grid_h as u32) < (b.grid_w as u32 * b.grid_h as u32),
                "{} -> {}: area did not strictly increase",
                a.name,
                b.name
            );
            assert!(
                a.braid_factor > b.braid_factor,
                "{} -> {}: braid_factor did not strictly decrease",
                a.name,
                b.name
            );
        }
    }
}
