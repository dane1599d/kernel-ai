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
    no_delay_on_std();
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

/// QEMU's user network gives the relay its end of a loopback TCP pair as
/// standard input and output (libslirp makes the pair over TCP for
/// out-of-band data), with Nagle on: a response's body could wait for QEMU's
/// delayed ACK of its head, about 40 ms a request (#816).
#[cfg(unix)]
fn no_delay_on_std() {
    use std::mem::ManuallyDrop;
    use std::os::fd::FromRawFd;
    for fd in [0, 1] {
        // SAFETY: standard input and output stay open for the process's life,
        // and `ManuallyDrop` keeps this stream from closing them.
        let stream = ManuallyDrop::new(unsafe { TcpStream::from_raw_fd(fd) });
        // On a pipe or a terminal the call fails, and nothing changes.
        let _ = stream.set_nodelay(true);
    }
}

#[cfg(not(unix))]
fn no_delay_on_std() {}

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
