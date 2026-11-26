//! Spike S07-4's blob server (G37, `spike/netd-ring3` only): `cargo xtask
//! netd-blob ADDR:PORT BYTES` answers every HTTP request on ADDR:PORT with
//! BYTES zero bytes, written 256 KiB at a time from memory, so the host's disk
//! and Python stay out of the figures. One thread per connection.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

pub fn main(args: &[String]) -> Result<(), String> {
    let [addr, bytes] = args else {
        return Err(String::from("usage: cargo xtask netd-blob ADDR:PORT BYTES"));
    };
    let bytes: u64 = bytes
        .parse()
        .map_err(|_| format!("not a byte count: {bytes}"))?;
    let listener = TcpListener::bind(addr).map_err(|e| format!("netd-blob: bind {addr}: {e}"))?;
    eprintln!("netd-blob: serving {bytes} bytes on {addr}");
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        std::thread::spawn(move || {
            if let Err(e) = serve(stream, bytes) {
                eprintln!("netd-blob: {e}");
            }
        });
    }
    Ok(())
}

fn serve(mut stream: TcpStream, bytes: u64) -> std::io::Result<()> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
        if stream.read(&mut byte)? == 0 {
            return Ok(());
        }
        head.push(byte[0]);
    }
    let _ = stream.set_nodelay(true);
    write!(stream, "HTTP/1.0 200 OK\r\nContent-Length: {bytes}\r\n\r\n")?;
    let chunk = vec![0u8; 256 * 1024];
    let mut left = bytes;
    while left > 0 {
        let n = left.min(chunk.len() as u64) as usize;
        stream.write_all(&chunk[..n])?;
        left -= n as u64;
    }
    eprintln!("netd-blob: sent {bytes} bytes");
    Ok(())
}
