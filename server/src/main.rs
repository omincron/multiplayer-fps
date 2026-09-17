//! Authoritative game server entry point.

use common::config::Config;
use server::net;

fn main() {
    let config = Config::default();
    match net::spawn(config.clone()) {
        Ok(addr) => {
            println!("maze_wars server listening on {addr}");
        }
        Err(e) => {
            eprintln!("maze_wars server: failed to bind {}: {e}", config.bind_addr);
            std::process::exit(1);
        }
    }
    // net::spawn's threads run for the life of the process (Milestone 6:
    // no graceful shutdown yet). Park the main thread rather than return
    // and let the process exit immediately.
    loop {
        std::thread::park();
    }
}
