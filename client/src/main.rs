use client::fps::FpsMeter;
use client::maze::build_and_validate;
use client::net::{Connection, connect};
use client::prompts::{prompt_name, prompt_server_address};
use client::render::raycast::{WallSide, cast_view};
use common::maze::MazeData;
use common::types::Vec2;
use macroquad::prelude::*;
use std::io::{self, Write};

fn main() {
    if let Err(error) = start() {
        eprintln!("Client failed: {error}");
        std::process::exit(1);
    }
}

fn start() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();

    let address = prompt_server_address(&mut input, &mut output)?;
    let name = prompt_name(&mut input, &mut output)?;
    writeln!(output, "Starting...")?;
    output.flush()?;

    let connection = connect(address, &name)?;
    let maze = build_and_validate(connection.welcome.maze.clone())?;
    macroquad::Window::from_config(window_config(), run_window(connection, name, maze));
    Ok(())
}

fn window_config() -> Conf {
    Conf {
        window_title: "Maze Wars".to_owned(),
        window_width: 1280,
        window_height: 720,
        high_dpi: true,
        ..Default::default()
    }
}

async fn run_window(connection: Connection, name: String, maze: MazeData) {
    let mut fps_meter = FpsMeter::default();
    let camera_position = Vec2::new(0.5, 0.5);
    let mut camera_angle = 0.0_f32;
    const FIELD_OF_VIEW: f32 = std::f32::consts::FRAC_PI_3;
    const TURN_SPEED: f32 = 1.8;

    loop {
        let frame_time = get_frame_time();
        fps_meter.record_frame(frame_time);
        if is_key_down(KeyCode::Left) || is_key_down(KeyCode::A) {
            camera_angle -= TURN_SPEED * frame_time;
        }
        if is_key_down(KeyCode::Right) || is_key_down(KeyCode::D) {
            camera_angle += TURN_SPEED * frame_time;
        }

        draw_maze_view(&maze, camera_position, camera_angle, FIELD_OF_VIEW);

        draw_text(
            format!(
                "Connected as {name} (player {})",
                connection.welcome.player_id
            ),
            24.0,
            36.0,
            24.0,
            LIGHTGRAY,
        );
        draw_text(
            format!("FPS: {:.0}", fps_meter.fps()),
            24.0,
            70.0,
            28.0,
            YELLOW,
        );
        draw_text(
            "Look: Left/Right or A/D",
            24.0,
            screen_height() - 24.0,
            22.0,
            LIGHTGRAY,
        );

        next_frame().await;
    }
}

fn draw_maze_view(maze: &MazeData, camera: Vec2, facing: f32, field_of_view: f32) {
    let width = screen_width();
    let height = screen_height();
    draw_rectangle(
        0.0,
        0.0,
        width,
        height * 0.5,
        Color::from_rgba(10, 16, 28, 255),
    );
    draw_rectangle(
        0.0,
        height * 0.5,
        width,
        height * 0.5,
        Color::from_rgba(22, 25, 29, 255),
    );

    let columns = width.ceil().max(1.0) as usize;
    let max_distance = (maze.grid.width as f32).hypot(maze.grid.height as f32) + 1.0;
    let projection_plane = (width * 0.5) / (field_of_view * 0.5).tan();
    for (column, hit) in cast_view(
        &maze.grid,
        camera,
        facing,
        field_of_view,
        columns,
        max_distance,
    )
    .into_iter()
    .enumerate()
    {
        let Some(hit) = hit else { continue };
        let depth = hit.depth.max(0.0001);
        let wall_height = (projection_plane / depth).min(height * 3.0);
        let brightness = (1.0 / (1.0 + depth * 0.12)).clamp(0.2, 1.0);
        let face_shade = if hit.side == WallSide::Vertical {
            1.0
        } else {
            0.72
        };
        let color = Color::new(
            0.15 * brightness * face_shade,
            0.85 * brightness * face_shade,
            0.62 * brightness * face_shade,
            1.0,
        );
        draw_rectangle(
            column as f32,
            (height - wall_height) * 0.5,
            1.1,
            wall_height,
            color,
        );
    }
}
