use common::config::{
    GENERATOR_VERSION, JOIN_MAX_ATTEMPTS, JOIN_RETRY_BASE_MS, MAX_PAYLOAD_BYTES, PROTOCOL_VERSION,
};
use common::maze::MazeSource;
use common::protocol::{ClientMsg, ServerMsg, decode, encode};
use common::types::{PlayerId, Tick};
use std::fmt;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct HandshakeOptions {
    pub attempts: u32,
    pub base_delay: Duration,
}

impl Default for HandshakeOptions {
    fn default() -> Self {
        Self {
            attempts: JOIN_MAX_ATTEMPTS,
            base_delay: Duration::from_millis(JOIN_RETRY_BASE_MS),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WelcomeData {
    pub player_id: PlayerId,
    pub level: u8,
    pub level_epoch: u32,
    pub maze: MazeSource,
    pub tick_rate_hz: u16,
    pub server_tick: Tick,
    pub server_time_ms: u64,
}

#[derive(Debug)]
pub struct Connection {
    pub socket: UdpSocket,
    pub welcome: WelcomeData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    Rejected(String),
    Exhausted { address: SocketAddr, attempts: u32 },
    Io(String),
    Protocol(String),
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(reason) => write!(formatter, "connection rejected: {reason}"),
            Self::Exhausted { address, attempts } => {
                write!(
                    formatter,
                    "could not connect to {address} after {attempts} attempts"
                )
            }
            Self::Io(reason) => write!(formatter, "network error: {reason}"),
            Self::Protocol(reason) => write!(formatter, "protocol error: {reason}"),
        }
    }
}

impl std::error::Error for HandshakeError {}

pub fn retry_delays_ms(base_delay_ms: u64, attempts: u32) -> Vec<u64> {
    (0..attempts)
        .map(|attempt| base_delay_ms.saturating_mul(2_u64.saturating_pow(attempt)))
        .collect()
}

pub fn connect(address: SocketAddr, name: &str) -> Result<Connection, HandshakeError> {
    connect_with_options(address, name, HandshakeOptions::default())
}

pub fn connect_with_options(
    address: SocketAddr,
    name: &str,
    options: HandshakeOptions,
) -> Result<Connection, HandshakeError> {
    let bind_address = if address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind_address).map_err(io_error)?;
    socket.connect(address).map_err(io_error)?;

    let join = ClientMsg::Join {
        name: name.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        generator_version: GENERATOR_VERSION,
    };
    let payload = encode(&join).map_err(|error| HandshakeError::Protocol(error.to_string()))?;
    let mut buffer = [0_u8; MAX_PAYLOAD_BYTES];

    for delay_ms in retry_delays_ms(options.base_delay.as_millis() as u64, options.attempts) {
        socket.send(&payload).map_err(io_error)?;
        socket
            .set_read_timeout(Some(Duration::from_millis(delay_ms)))
            .map_err(io_error)?;

        match socket.recv(&mut buffer) {
            Ok(length) => match decode::<ServerMsg>(&buffer[..length]) {
                Ok(ServerMsg::Welcome {
                    player_id,
                    level,
                    level_epoch,
                    maze,
                    tick_rate_hz,
                    server_tick,
                    server_time_ms,
                }) => {
                    socket.set_read_timeout(None).map_err(io_error)?;
                    return Ok(Connection {
                        socket,
                        welcome: WelcomeData {
                            player_id,
                            level,
                            level_epoch,
                            maze,
                            tick_rate_hz,
                            server_tick,
                            server_time_ms,
                        },
                    });
                }
                Ok(ServerMsg::Rejected { reason }) => return Err(HandshakeError::Rejected(reason)),
                Ok(_) => continue,
                Err(error) => return Err(HandshakeError::Protocol(error.to_string())),
            },
            Err(error) if is_retryable(&error) => continue,
            Err(error) => return Err(io_error(error)),
        }
    }

    Err(HandshakeError::Exhausted {
        address,
        attempts: options.attempts,
    })
}

fn is_retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionRefused
    )
}

fn io_error(error: io::Error) -> HandshakeError {
    HandshakeError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::config::{GENERATOR_VERSION, JOIN_MAX_ATTEMPTS, JOIN_RETRY_BASE_MS};
    use common::maze::{MazeSource, MazeSpec};
    use common::protocol::{ClientMsg, ServerMsg, decode, encode};
    use std::net::UdpSocket;
    use std::thread;
    use std::time::{Duration, Instant};

    fn welcome() -> ServerMsg {
        ServerMsg::Welcome {
            player_id: 7,
            level: 1,
            level_epoch: 2,
            maze: MazeSource::Generated(MazeSpec {
                seed: 99,
                width: 20,
                height: 20,
                braid_factor: 0.6,
                generator_version: GENERATOR_VERSION,
            }),
            tick_rate_hz: 30,
            server_tick: 10,
            server_time_ms: 500,
        }
    }

    fn fake_server(
        reply_after_join: usize,
        reply: ServerMsg,
    ) -> (std::net::SocketAddr, thread::JoinHandle<usize>) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut buffer = [0_u8; MAX_PAYLOAD_BYTES];
            for received in 1..=reply_after_join {
                let (length, peer) = socket.recv_from(&mut buffer).unwrap();
                let message: ClientMsg = decode(&buffer[..length]).unwrap();
                assert!(matches!(message, ClientMsg::Join { .. }));
                if received == reply_after_join {
                    socket.send_to(&encode(&reply).unwrap(), peer).unwrap();
                    return received;
                }
            }
            unreachable!()
        });
        (address, handle)
    }

    #[test]
    fn production_retry_schedule_uses_shared_constants_and_doubles() {
        let delays = retry_delays_ms(JOIN_RETRY_BASE_MS, JOIN_MAX_ATTEMPTS);
        assert_eq!(delays.len(), JOIN_MAX_ATTEMPTS as usize);
        assert_eq!(delays.first(), Some(&JOIN_RETRY_BASE_MS));
        assert!(delays.windows(2).all(|pair| pair[1] == pair[0] * 2));
    }

    #[test]
    fn succeeds_after_two_dropped_join_attempts() {
        let (address, server) = fake_server(3, welcome());
        let options = HandshakeOptions {
            attempts: 4,
            base_delay: Duration::from_millis(10),
        };
        let connection = connect_with_options(address, "alice", options).unwrap();
        assert_eq!(connection.welcome.player_id, 7);
        assert_eq!(server.join().unwrap(), 3);
    }

    #[test]
    fn rejection_is_final_and_does_not_retry() {
        let (address, server) = fake_server(
            1,
            ServerMsg::Rejected {
                reason: "name taken".into(),
            },
        );
        let options = HandshakeOptions {
            attempts: 4,
            base_delay: Duration::from_millis(10),
        };
        let started = Instant::now();
        let error = connect_with_options(address, "alice", options).unwrap_err();
        assert!(matches!(error, HandshakeError::Rejected(ref reason) if reason == "name taken"));
        assert!(started.elapsed() < Duration::from_millis(40));
        assert_eq!(server.join().unwrap(), 1);
    }

    #[test]
    fn exhausted_attempts_report_the_address_and_attempt_count() {
        let unused = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let options = HandshakeOptions {
            attempts: 2,
            base_delay: Duration::from_millis(5),
        };
        let error = connect_with_options(unused, "alice", options).unwrap_err();
        let text = error.to_string();
        assert!(text.contains(&unused.to_string()));
        assert!(text.contains("2 attempts"));
    }
}
