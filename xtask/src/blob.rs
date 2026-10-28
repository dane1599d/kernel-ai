//! Spike P5-S2 (not for merge): the blob server `test=net-throughput` downloads
//! from at 10.0.2.2. `GET /blob/N` answers N bytes of a fixed pattern, then
//! closes; the guest stops reading when its time is up, so a broken pipe is
//! normal.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

const CHUNK: usize = 1 << 20;
/// Longest request head read.
const MAX_HEAD: usize = 4096;

pub struct BlobServer {
    pub port: u16,
}

pub fn start() -> Result<BlobServer, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot start the blob server: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("blob server has no address: {e}"))?
        .port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || serve(stream));
        }
    });
    Ok(BlobServer { port })
}

/// The byte count in `GET /blob/N`, if the head asks for one.
fn wanted(head: &[u8]) -> Option<u64> {
    let line = head.split(|&b| b == b'\r').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let path = line.strip_prefix("GET ")?.split(' ').next()?;
    path.strip_prefix("/blob/")?.parse().ok()
}

fn serve(mut stream: TcpStream) {
    let mut head = Vec::new();
    let mut buf = [0u8; 512];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < MAX_HEAD {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let Some(mut left) = wanted(&head) else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    };
    let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {left}\r\nConnection: close\r\n\r\n");
    if stream.write_all(reply.as_bytes()).is_err() {
        return;
    }
    let chunk: Vec<u8> = (0..CHUNK).map(|i| (i % 251) as u8).collect();
    while left > 0 {
        let n = left.min(CHUNK as u64) as usize;
        if stream.write_all(&chunk[..n]).is_err() {
            return;
        }
        left -= n as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_the_asked_number_of_bytes_and_refuses_other_paths() {
        assert_eq!(wanted(b"GET /blob/1024 HTTP/1.1\r\n\r\n"), Some(1024));
        assert_eq!(wanted(b"GET /other HTTP/1.1\r\n\r\n"), None);
        let server = start().expect("start");
        let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
        stream
            .write_all(b"GET /blob/3000000 HTTP/1.1\r\nHost: blob\r\n\r\n")
            .expect("send");
        let mut body = Vec::new();
        stream.read_to_end(&mut body).expect("read");
        let at = body
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("head")
            + 4;
        assert_eq!(body.len() - at, 3_000_000);
        assert!(
            body[at..]
                .iter()
                .enumerate()
                .all(|(i, &b)| b == ((i % CHUNK) % 251) as u8)
        );
    }
}
