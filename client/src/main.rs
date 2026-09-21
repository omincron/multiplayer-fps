use client::fps::FpsMeter;
use client::maze::build_and_validate;
use client::net::{Connection, connect, send_message, spawn_receiver};
use client::players::RemotePlayers;
use client::predict::Predictor;
use client::prompts::{prompt_name, prompt_server_address};
use client::render::minimap::{MinimapRect, draw_minimap};
use client::render::raycast::{WallSide, cast_view};
use common::maze::MazeData;
use common::protocol::{ClientMsg, ServerMsg};
use common::types::Vec2;
use macroquad::prelude::*;
use std::io::{self, Write};
use std::sync::mpsc::Receiver;

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
    let incoming = spawn_receiver(&connection.socket)?;
    macroquad::Window::from_config(
        window_config(),
        run_window(connection, incoming, name, maze),
    );
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

async fn run_window(
    connection: Connection,
    incoming: Receiver<ServerMsg>,
    name: String,
    maze: MazeData,
) {
    let mut fps_meter = FpsMeter::default();
    let spawn_position = Vec2::new(
        (maze.grid.width / 2) as f32 + 0.5,
        (maze.grid.height / 2) as f32 + 0.5,
    );
    let mut predictor = Predictor::new(spawn_position);
    let mut camera_correction = Vec2::ZERO;
    let mut camera_angle = 0.0_f32;
    let mut remote_players = RemotePlayers::default();
    let mut network_error: Option<String> = None;
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

        let forward = (is_key_down(KeyCode::W) || is_key_down(KeyCode::Up)) as u8 as f32
            - (is_key_down(KeyCode::S) || is_key_down(KeyCode::Down)) as u8 as f32;
        let move_dir = Vec2::new(camera_angle.cos() * forward, camera_angle.sin() * forward);
        for input in predictor.advance_frame(frame_time, move_dir, camera_angle, false, &maze.grid)
        {
            let message = ClientMsg::Input {
                input_tick: input.input_tick,
                move_dir: input.move_dir,
                facing: input.facing,
                shoot: input.shoot,
            };
            if let Err(error) = send_message(&connection.socket, &message) {
                network_error = Some(error.to_string());
            }
        }

        while let Ok(message) = incoming.try_recv() {
            match message {
                ServerMsg::WorldState {
                    level_epoch,
                    players,
                    ..
                } if level_epoch == connection.welcome.level_epoch => {
                    let now_ms = (get_time() * 1000.0) as u64;
                    remote_players.apply_snapshot(connection.welcome.player_id, &players, now_ms);
                    if let Some(player) = players
                        .iter()
                        .find(|player| player.id == connection.welcome.player_id)
                    {
                        let correction =
                            predictor.reconcile(player.pos, player.last_input_tick, &maze.grid);
                        camera_correction.x -= correction.x;
                        camera_correction.y -= correction.y;
                    }
                }
                ServerMsg::Event { event_id, kind } => {
                    remote_players.apply_event(&kind);
                    if let Err(error) =
                        send_message(&connection.socket, &ClientMsg::Ack { event_id })
                    {
                        network_error = Some(error.to_string());
                    }
                }
                ServerMsg::Welcome { .. }
                | ServerMsg::Rejected { .. }
                | ServerMsg::WorldState { .. }
                | ServerMsg::Pong { .. } => {}
            }
        }

        let correction_decay = (-12.0 * frame_time).exp();
        camera_correction.x *= correction_decay;
        camera_correction.y *= correction_decay;
        let predicted = predictor.position();
        let camera_position = Vec2::new(
            predicted.x + camera_correction.x,
            predicted.y + camera_correction.y,
        );
        draw_maze_view(&maze, camera_position, camera_angle, FIELD_OF_VIEW);
        let minimap_scale = 220.0 / (maze.grid.width.max(maze.grid.height) as f32);
        let minimap = MinimapRect {
            width: maze.grid.width as f32 * minimap_scale,
            height: maze.grid.height as f32 * minimap_scale,
            x: screen_width() - maze.grid.width as f32 * minimap_scale - 20.0,
            y: 20.0,
        };
        let remote_positions: Vec<Vec2> = remote_players.iter().map(|player| player.pos).collect();
        draw_minimap(
            &maze.grid,
            predicted,
            camera_angle,
            &remote_positions,
            minimap,
        );

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
            "Move: W/S or Up/Down   Look: A/D or Left/Right",
            24.0,
            screen_height() - 24.0,
            22.0,
            LIGHTGRAY,
        );
        if let Some(error) = &network_error {
            draw_text(format!("Network error: {error}"), 24.0, 104.0, 22.0, RED);
        }

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
