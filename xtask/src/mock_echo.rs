//! A TCP echo server for the stream case (P5 task 2): sends back whatever each
//! connection sends, until it closes, on a free local port, which the guest
//! reaches at 10.0.2.2 through QEMU's user networking and is told about with
//! `echo=`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

pub struct MockEcho {
    pub port: u16,
}

impl MockEcho {
    /// The kernel command-line word that points the guest here.
    pub fn cmdline(&self) -> String {
        format!("echo=10.0.2.2:{}", self.port)
    }
}

/// Starts the server on a free local port. It runs until xtask exits.
pub fn start() -> Result<MockEcho, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot start the echo server: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("the echo server has no address: {e}"))?
        .port();
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            thread::spawn(move || echo(stream));
        }
    });
    Ok(MockEcho { port })
}

fn echo(mut stream: TcpStream) {
    let mut buf = [0u8; 16 * 1024];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 || stream.write_all(&buf[..n]).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_back_what_it_gets() {
        let server = start().unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        client.write_all(b"hello").unwrap();
        let mut back = [0u8; 5];
        client.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"hello");
        assert_eq!(server.cmdline(), format!("echo=10.0.2.2:{}", server.port));
    }
}
