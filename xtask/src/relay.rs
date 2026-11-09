//! `cargo xtask relay <addr:port>`: copies its standard input to a TCP
//! connection to `addr:port` and the connection to its standard output, until
//! either side ends. QEMU's user network starts one for each guest connection
//! to the HTTPS mock's forwarded address (`qemu::forward_https`), so a guest
//! reaches the mock, which listens on a free host port, on port 443.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::thread;

/// The most bytes one copy moves at a time.
const BUFFER: usize = 16 * 1024;

pub fn main(args: &[String]) -> Result<(), String> {
    let [target] = args else {
        return Err("usage: cargo xtask relay <addr:port>".into());
    };
    let stream =
        TcpStream::connect(target.as_str()).map_err(|e| format!("relay: {target}: {e}"))?;
    let _ = stream.set_nodelay(true);
    let upstream = stream
        .try_clone()
        .map_err(|e| format!("relay: {target}: {e}"))?;
    // Guest to mock: standard input until it ends, then a half close.
    let sender = thread::spawn(move || {
        let mut upstream = upstream;
        let _ = copy(&mut io::stdin().lock(), &mut upstream);
        let _ = upstream.shutdown(Shutdown::Write);
    });
    // Mock to guest: until the mock ends the connection.
    let mut downstream = stream;
    let result = copy(&mut downstream, &mut io::stdout().lock());
    let _ = downstream.shutdown(Shutdown::Both);
    // The guest's side may never end on its own; the relay ends with the mock's.
    drop(sender);
    result.map_err(|e| format!("relay: {target}: {e}"))
}

/// Copies `from` to `to`, flushing each piece, until `from` ends.
fn copy(from: &mut impl Read, to: &mut impl Write) -> io::Result<()> {
    let mut buf = vec![0u8; BUFFER];
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        to.write_all(&buf[..n])?;
        to.flush()?;
    }
}
