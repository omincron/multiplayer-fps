use common::config::GENERATOR_VERSION;
use common::maze::{MazeData, MazeGrid, MazeSource, WALL_E, WALL_N, WALL_S, WALL_W, generate};
use std::collections::VecDeque;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MazeBuildError {
    GeneratorVersion { received: u16, expected: u16 },
    InvalidDimensions,
    InvalidWallBits { x: u16, y: u16 },
    OpenBorder { x: u16, y: u16 },
    AsymmetricWall { x: u16, y: u16 },
    IsolatedCell { x: u16, y: u16 },
    Disconnected,
}

impl fmt::Display for MazeBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GeneratorVersion { received, expected } => write!(
                formatter,
                "maze generator version {received} does not match client version {expected}"
            ),
            Self::InvalidDimensions => write!(formatter, "maze dimensions do not match wall data"),
            Self::InvalidWallBits { x, y } => {
                write!(formatter, "cell ({x}, {y}) has unknown wall bits")
            }
            Self::OpenBorder { x, y } => {
                write!(formatter, "maze border is open at cell ({x}, {y})")
            }
            Self::AsymmetricWall { x, y } => {
                write!(formatter, "wall is asymmetric at cell ({x}, {y})")
            }
            Self::IsolatedCell { x, y } => write!(formatter, "cell ({x}, {y}) is isolated"),
            Self::Disconnected => write!(formatter, "maze contains disconnected regions"),
        }
    }
}

impl std::error::Error for MazeBuildError {}

pub fn build_and_validate(source: MazeSource) -> Result<MazeData, MazeBuildError> {
    let maze = match source {
        MazeSource::Generated(spec) => {
            if spec.generator_version != GENERATOR_VERSION {
                return Err(MazeBuildError::GeneratorVersion {
                    received: spec.generator_version,
                    expected: GENERATOR_VERSION,
                });
            }
            generate(spec)
        }
        MazeSource::Custom(grid) => MazeData {
            source: MazeSource::Custom(grid.clone()),
            grid,
            generator_name: "custom".to_owned(),
        },
    };
    validate_grid(&maze.grid)?;
    Ok(maze)
}

fn validate_grid(grid: &MazeGrid) -> Result<(), MazeBuildError> {
    let expected_len = grid.width as usize * grid.height as usize;
    if grid.width == 0 || grid.height == 0 || grid.walls.len() != expected_len {
        return Err(MazeBuildError::InvalidDimensions);
    }

    const KNOWN_WALLS: u8 = WALL_N | WALL_E | WALL_S | WALL_W;
    for y in 0..grid.height {
        for x in 0..grid.width {
            let walls = grid.walls_at(x, y);
            if walls & !KNOWN_WALLS != 0 {
                return Err(MazeBuildError::InvalidWallBits { x, y });
            }
            if (y == 0 && walls & WALL_N == 0)
                || (x + 1 == grid.width && walls & WALL_E == 0)
                || (y + 1 == grid.height && walls & WALL_S == 0)
                || (x == 0 && walls & WALL_W == 0)
            {
                return Err(MazeBuildError::OpenBorder { x, y });
            }
            if x + 1 < grid.width {
                let east = walls & WALL_E != 0;
                let west = grid.walls_at(x + 1, y) & WALL_W != 0;
                if east != west {
                    return Err(MazeBuildError::AsymmetricWall { x, y });
                }
            }
            if y + 1 < grid.height {
                let south = walls & WALL_S != 0;
                let north = grid.walls_at(x, y + 1) & WALL_N != 0;
                if south != north {
                    return Err(MazeBuildError::AsymmetricWall { x, y });
                }
            }
            if walls & KNOWN_WALLS == KNOWN_WALLS {
                return Err(MazeBuildError::IsolatedCell { x, y });
            }
        }
    }

    if reachable_cell_count(grid) != expected_len {
        return Err(MazeBuildError::Disconnected);
    }
    Ok(())
}

fn reachable_cell_count(grid: &MazeGrid) -> usize {
    let mut seen = vec![false; grid.cell_count()];
    let mut queue = VecDeque::from([(0_u16, 0_u16)]);
    seen[0] = true;
    let mut count = 0;

    const DIRECTIONS: [(u8, i32, i32); 4] = [
        (WALL_N, 0, -1),
        (WALL_E, 1, 0),
        (WALL_S, 0, 1),
        (WALL_W, -1, 0),
    ];
    while let Some((x, y)) = queue.pop_front() {
        count += 1;
        let walls = grid.walls_at(x, y);
        for (wall, dx, dy) in DIRECTIONS {
            if walls & wall != 0 {
                continue;
            }
            let nx = (x as i32 + dx) as u16;
            let ny = (y as i32 + dy) as u16;
            let index = grid.index(nx, ny);
            if !seen[index] {
                seen[index] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::config::GENERATOR_VERSION;
    use common::maze::{MazeGrid, MazeSource, MazeSpec, WALL_E, WALL_W, generate};

    fn generated_source() -> MazeSource {
        MazeSource::Generated(MazeSpec {
            seed: 42,
            width: 8,
            height: 6,
            braid_factor: 0.3,
            generator_version: GENERATOR_VERSION,
        })
    }

    #[test]
    fn generated_source_rebuilds_the_same_maze() {
        let source = generated_source();
        let expected = match source {
            MazeSource::Generated(spec) => generate(spec),
            MazeSource::Custom(_) => unreachable!(),
        };

        assert_eq!(build_and_validate(source).unwrap(), expected);
    }

    #[test]
    fn valid_custom_grid_is_accepted() {
        let generated = build_and_validate(generated_source()).unwrap();
        let grid = generated.grid;

        let maze = build_and_validate(MazeSource::Custom(grid.clone())).unwrap();

        assert_eq!(maze.grid, grid);
        assert!(matches!(maze.source, MazeSource::Custom(_)));
    }

    #[test]
    fn asymmetric_custom_wall_is_rejected() {
        let generated = build_and_validate(generated_source()).unwrap();
        let mut grid = generated.grid;
        let left = grid.index(0, 0);
        let right = grid.index(1, 0);
        grid.walls[left] |= WALL_E;
        grid.walls[right] &= !WALL_W;

        let error = build_and_validate(MazeSource::Custom(grid)).unwrap_err();
        assert!(matches!(error, MazeBuildError::AsymmetricWall { .. }));
    }

    #[test]
    fn incorrect_wall_vector_length_is_rejected() {
        let grid = MazeGrid {
            width: 2,
            height: 2,
            walls: vec![0; 3],
        };
        let error = build_and_validate(MazeSource::Custom(grid)).unwrap_err();
        assert!(matches!(error, MazeBuildError::InvalidDimensions));
    }
}
