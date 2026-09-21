use common::maze::{MazeGrid, WALL_E, WALL_N, WALL_S, WALL_W};
use common::types::Vec2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallSide {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Distance along the normalized ray direction, in maze-cell units.
    pub distance: f32,
    pub side: WallSide,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewColumn {
    pub depth: f32,
    pub side: WallSide,
}

pub fn cast_view(
    grid: &MazeGrid,
    origin: Vec2,
    camera_angle: f32,
    field_of_view: f32,
    columns: usize,
    max_distance: f32,
) -> Vec<Option<ViewColumn>> {
    if columns == 0 {
        return Vec::new();
    }

    (0..columns)
        .map(|column| {
            let fraction = (column as f32 + 0.5) / columns as f32;
            let ray_angle = camera_angle - field_of_view * 0.5 + fraction * field_of_view;
            cast_ray(grid, origin, ray_angle, max_distance).map(|hit| ViewColumn {
                depth: perpendicular_distance(hit.distance, ray_angle, camera_angle),
                side: hit.side,
            })
        })
        .collect()
}

pub fn perpendicular_distance(ray_distance: f32, ray_angle: f32, camera_angle: f32) -> f32 {
    ray_distance * (ray_angle - camera_angle).cos()
}

/// Cast one normalized direction ray through the maze using grid DDA.
pub fn cast_ray(grid: &MazeGrid, origin: Vec2, angle: f32, max_distance: f32) -> Option<RayHit> {
    if !origin.x.is_finite()
        || !origin.y.is_finite()
        || !angle.is_finite()
        || !max_distance.is_finite()
        || max_distance <= 0.0
    {
        return None;
    }

    let mut cell_x = origin.x.floor() as i32;
    let mut cell_y = origin.y.floor() as i32;
    if !in_bounds(grid, cell_x, cell_y) {
        return None;
    }

    let dir_x = angle.cos();
    let dir_y = angle.sin();
    let delta_x = reciprocal_abs(dir_x);
    let delta_y = reciprocal_abs(dir_y);

    let (step_x, mut next_x) = if dir_x < 0.0 {
        (-1, (origin.x - cell_x as f32) * delta_x)
    } else {
        (1, (cell_x as f32 + 1.0 - origin.x) * delta_x)
    };
    let (step_y, mut next_y) = if dir_y < 0.0 {
        (-1, (origin.y - cell_y as f32) * delta_y)
    } else {
        (1, (cell_y as f32 + 1.0 - origin.y) * delta_y)
    };

    loop {
        let (distance, side, wall) = if next_x < next_y {
            let result = (
                next_x,
                WallSide::Vertical,
                if step_x > 0 { WALL_E } else { WALL_W },
            );
            next_x += delta_x;
            result
        } else {
            let result = (
                next_y,
                WallSide::Horizontal,
                if step_y > 0 { WALL_S } else { WALL_N },
            );
            next_y += delta_y;
            result
        };

        if distance > max_distance {
            return None;
        }

        if grid.walls_at(cell_x as u16, cell_y as u16) & wall != 0 {
            return Some(RayHit { distance, side });
        }

        match side {
            WallSide::Vertical => cell_x += step_x,
            WallSide::Horizontal => cell_y += step_y,
        }
        if !in_bounds(grid, cell_x, cell_y) {
            return None;
        }
    }
}

fn reciprocal_abs(value: f32) -> f32 {
    if value.abs() < f32::EPSILON {
        f32::INFINITY
    } else {
        1.0 / value.abs()
    }
}

fn in_bounds(grid: &MazeGrid, x: i32, y: i32) -> bool {
    x >= 0 && y >= 0 && x < grid.width as i32 && y < grid.height as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::maze::{MazeGrid, WALL_E, WALL_N, WALL_S, WALL_W};
    use common::types::Vec2;

    fn grid_with_closed_border(width: u16, height: u16) -> MazeGrid {
        let mut grid = MazeGrid {
            width,
            height,
            walls: vec![0; width as usize * height as usize],
        };
        for y in 0..height {
            for x in 0..width {
                let index = grid.index(x, y);
                if y == 0 {
                    grid.walls[index] |= WALL_N;
                }
                if x + 1 == width {
                    grid.walls[index] |= WALL_E;
                }
                if y + 1 == height {
                    grid.walls[index] |= WALL_S;
                }
                if x == 0 {
                    grid.walls[index] |= WALL_W;
                }
            }
        }
        grid
    }

    fn add_vertical_wall(grid: &mut MazeGrid, left_x: u16, y: u16) {
        let left = grid.index(left_x, y);
        let right = grid.index(left_x + 1, y);
        grid.walls[left] |= WALL_E;
        grid.walls[right] |= WALL_W;
    }

    #[test]
    fn east_ray_hits_known_internal_wall_half_a_cell_away() {
        let mut grid = grid_with_closed_border(3, 3);
        add_vertical_wall(&mut grid, 1, 1);

        let hit = cast_ray(&grid, Vec2::new(1.5, 1.5), 0.0, 10.0).unwrap();

        assert!((hit.distance - 0.5).abs() < 0.0001);
        assert_eq!(hit.side, WallSide::Vertical);
    }

    #[test]
    fn west_ray_crosses_open_cell_then_hits_border() {
        let grid = grid_with_closed_border(3, 3);

        let hit = cast_ray(&grid, Vec2::new(1.5, 1.5), std::f32::consts::PI, 10.0).unwrap();

        assert!((hit.distance - 1.5).abs() < 0.0001);
        assert_eq!(hit.side, WallSide::Vertical);
    }

    #[test]
    fn north_ray_hits_border_at_hand_calculated_distance() {
        let grid = grid_with_closed_border(3, 3);

        let hit = cast_ray(
            &grid,
            Vec2::new(1.5, 1.25),
            -std::f32::consts::FRAC_PI_2,
            10.0,
        )
        .unwrap();

        assert!((hit.distance - 1.25).abs() < 0.0001);
        assert_eq!(hit.side, WallSide::Horizontal);
    }

    #[test]
    fn wall_beyond_max_distance_is_a_miss() {
        let grid = grid_with_closed_border(3, 3);
        assert!(cast_ray(&grid, Vec2::new(1.5, 1.5), 0.0, 1.0).is_none());
    }

    #[test]
    fn perpendicular_distance_removes_fisheye_from_angled_ray() {
        let camera_angle = 0.0;
        let ray_angle = std::f32::consts::FRAC_PI_6;
        let raw_distance_to_vertical_wall = 0.5 / ray_angle.cos();

        let corrected =
            perpendicular_distance(raw_distance_to_vertical_wall, ray_angle, camera_angle);

        assert!((corrected - 0.5).abs() < 0.0001);
    }
}
