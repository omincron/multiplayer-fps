use client::fps::FpsMeter;
use client::net::{Connection, connect};
use client::prompts::{prompt_name, prompt_server_address};
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
    macroquad::Window::from_config(window_config(), run_window(connection, name));
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

async fn run_window(connection: Connection, name: String) {
    let mut fps_meter = FpsMeter::default();

    loop {
        fps_meter.record_frame(get_frame_time());
        clear_background(Color::from_rgba(12, 17, 24, 255));

        draw_text("MAZE WARS", 48.0, 72.0, 42.0, GREEN);
        draw_text(
            format!(
                "Connected as {name} (player {})",
                connection.welcome.player_id
            ),
            48.0,
            118.0,
            26.0,
            LIGHTGRAY,
        );
        draw_text(
            format!("FPS: {:.0}", fps_meter.fps()),
            48.0,
            162.0,
            30.0,
            YELLOW,
        );
        draw_text(
            "Rendering pipeline ready — maze rendering is Milestone 9",
            48.0,
            210.0,
            22.0,
            GRAY,
        );

        next_frame().await;
    }
}
