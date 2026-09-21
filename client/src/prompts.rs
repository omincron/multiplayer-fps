use std::io::{self, BufRead, Write};
use std::net::SocketAddr;

/// Keeps usernames small enough for UI display and protocol payload headroom.
pub const MAX_NAME_CHARS: usize = 24;

pub fn prompt_server_address(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<SocketAddr> {
    loop {
        write!(output, "Enter IP Address: ")?;
        output.flush()?;
        let value = read_line(input)?;
        match value.trim().parse() {
            Ok(address) => return Ok(address),
            Err(_) => writeln!(
                output,
                "Invalid address. Use IP:port, for example 127.0.0.1:7777."
            )?,
        }
    }
}

pub fn prompt_name(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<String> {
    loop {
        write!(output, "Enter Name: ")?;
        output.flush()?;
        let value = read_line(input)?;
        let name = value.trim();
        if name.is_empty() {
            writeln!(output, "Name must not be empty.")?;
        } else if name.chars().count() > MAX_NAME_CHARS {
            writeln!(output, "Name must be at most {MAX_NAME_CHARS} characters.")?;
        } else {
            return Ok(name.to_owned());
        }
    }
}

fn read_line(input: &mut impl BufRead) -> io::Result<String> {
    let mut value = String::new();
    if input.read_line(&mut value)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "input closed"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn invalid_address_is_explained_and_prompted_again() {
        let mut input = Cursor::new(b"not-an-address\n127.0.0.1:7777\n");
        let mut output = Vec::new();

        let address = prompt_server_address(&mut input, &mut output).unwrap();

        assert_eq!(address, "127.0.0.1:7777".parse().unwrap());
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.matches("Enter IP Address:").count(), 2);
        assert!(text.contains("Invalid address"));
    }

    #[test]
    fn empty_and_oversized_names_are_prompted_again() {
        let oversized = "x".repeat(MAX_NAME_CHARS + 1);
        let source = format!("   \n{oversized}\n alice \n");
        let mut input = Cursor::new(source.into_bytes());
        let mut output = Vec::new();

        let name = prompt_name(&mut input, &mut output).unwrap();

        assert_eq!(name, "alice");
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.matches("Enter Name:").count(), 3);
        assert!(text.contains("must not be empty"));
        assert!(text.contains("at most"));
    }
}
