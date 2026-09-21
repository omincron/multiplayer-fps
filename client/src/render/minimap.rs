use common::maze::{MazeGrid, WALL_E, WALL_N, WALL_S, WALL_W};
use common::types::Vec2;
use macroquad::prelude::{Color, draw_circle, draw_line, draw_rectangle};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub fn world_to_minimap(
    position: Vec2,
    maze_width: u16,
    maze_height: u16,
    viewport: MinimapRect,
) -> Vec2 {
    if maze_width == 0 || maze_height == 0 {
        return Vec2::new(viewport.x, viewport.y);
    }
    Vec2::new(
        viewport.x + position.x / maze_width as f32 * viewport.width,
        viewport.y + position.y / maze_height as f32 * viewport.height,
    )
}

pub fn draw_minimap(
    grid: &MazeGrid,
    self_position: Vec2,
    self_facing: f32,
    remote_positions: &[Vec2],
    viewport: MinimapRect,
) {
    draw_rectangle(
        viewport.x - 4.0,
        viewport.y - 4.0,
        viewport.width + 8.0,
        viewport.height + 8.0,
        Color::from_rgba(5, 8, 12, 220),
    );
    let cell_width = viewport.width / grid.width as f32;
    let cell_height = viewport.height / grid.height as f32;
    let wall_color = Color::from_rgba(130, 235, 200, 235);
    for y in 0..grid.height {
        for x in 0..grid.width {
            let walls = grid.walls_at(x, y);
            let left = viewport.x + x as f32 * cell_width;
            let top = viewport.y + y as f32 * cell_height;
            let right = left + cell_width;
            let bottom = top + cell_height;
            if walls & WALL_N != 0 {
                draw_line(left, top, right, top, 1.0, wall_color);
            }
            if walls & WALL_W != 0 {
                draw_line(left, top, left, bottom, 1.0, wall_color);
            }
            if x + 1 == grid.width && walls & WALL_E != 0 {
                draw_line(right, top, right, bottom, 1.0, wall_color);
            }
            if y + 1 == grid.height && walls & WALL_S != 0 {
                draw_line(left, bottom, right, bottom, 1.0, wall_color);
            }
        }
    }

    for remote in remote_positions {
        let point = world_to_minimap(*remote, grid.width, grid.height, viewport);
        draw_circle(point.x, point.y, 4.0, Color::from_rgba(240, 90, 90, 255));
    }

    let own = world_to_minimap(self_position, grid.width, grid.height, viewport);
    draw_circle(own.x, own.y, 5.0, Color::from_rgba(255, 220, 70, 255));
    draw_line(
        own.x,
        own.y,
        own.x + self_facing.cos() * 11.0,
        own.y + self_facing.sin() * 11.0,
        2.0,
        Color::from_rgba(255, 245, 180, 255),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::types::Vec2;

    #[test]
    fn maps_world_cell_centers_into_minimap_coordinates() {
        let viewport = MinimapRect {
            x: 100.0,
            y: 50.0,
            width: 200.0,
            height: 100.0,
        };

        let mapped = world_to_minimap(Vec2::new(5.0, 2.5), 10, 5, viewport);

        assert!((mapped.x - 200.0).abs() < 0.001);
        assert!((mapped.y - 100.0).abs() < 0.001);
    }

    #[test]
    fn preserves_top_left_and_bottom_right_orientation() {
        let viewport = MinimapRect {
            x: 20.0,
            y: 30.0,
            width: 160.0,
            height: 80.0,
        };

        assert_eq!(
            world_to_minimap(Vec2::new(0.0, 0.0), 8, 4, viewport),
            Vec2::new(20.0, 30.0)
        );
        assert_eq!(
            world_to_minimap(Vec2::new(8.0, 4.0), 8, 4, viewport),
            Vec2::new(180.0, 110.0)
        );
    }
}
