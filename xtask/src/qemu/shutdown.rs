//! A power-off case's verdict: the reason in QMP's SHUTDOWN event. Under
//! -no-reboot and -no-shutdown a reset or a triple fault also leaves QEMU
//! paused in `shutdown`, so only the reason tells a power-off
//! (`guest-shutdown`) from them (`guest-reset`).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// The reason QEMU gives when the guest turned the machine off.
pub const GUEST_SHUTDOWN: &str = "guest-shutdown";
/// How long the listener keeps trying to reach QMP once QEMU has started.
const CONNECT_WITHIN: Duration = Duration::from_secs(30);
const CONNECT_RETRY: Duration = Duration::from_millis(100);

/// Listens on QMP `port` from the start of the boot, so the event cannot pass
/// before anyone hears it; the receiver gets the first SHUTDOWN's reason. QMP
/// takes one client, so a case that listens sends no `@` lines.
pub fn listen(port: u16) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Some(reason) = first_shutdown(port) {
            let _ = tx.send(reason);
        }
    });
    rx
}

/// Reads events until a SHUTDOWN, or until QEMU closes the socket.
fn first_shutdown(port: u16) -> Option<String> {
    let mut reader = BufReader::new(connect(port)?);
    let mut line = String::new();
    // QEMU's greeting, then no event comes until capabilities are negotiated.
    reader.read_line(&mut line).ok()?;
    writeln!(reader.get_mut(), r#"{{"execute":"qmp_capabilities"}}"#).ok()?;
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if let Some(reason) = shutdown_reason(&line) {
            return Some(reason);
        }
    }
}

fn connect(port: u16) -> Option<TcpStream> {
    let started = Instant::now();
    while started.elapsed() < CONNECT_WITHIN {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) {
            return Some(stream);
        }
        thread::sleep(CONNECT_RETRY);
    }
    None
}

/// The reason in a QMP SHUTDOWN event; None for any other line.
pub fn shutdown_reason(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("event")?.as_str()? != "SHUTDOWN" {
        return None;
    }
    let reason = value.get("data")?.get("reason")?.as_str()?;
    Some(reason.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    const RESET: &str = r#"{"timestamp": {"seconds": 1, "microseconds": 2}, "event": "SHUTDOWN", "data": {"guest": true, "reason": "guest-reset"}}"#;
    const OFF: &str = r#"{"timestamp": {"seconds": 1, "microseconds": 2}, "event": "SHUTDOWN", "data": {"guest": true, "reason": "guest-shutdown"}}"#;

    #[test]
    fn a_shutdown_event_gives_its_reason() {
        assert_eq!(shutdown_reason(OFF).as_deref(), Some(GUEST_SHUTDOWN));
        assert_eq!(shutdown_reason(RESET).as_deref(), Some("guest-reset"));
    }

    #[test]
    fn other_lines_give_no_reason() {
        assert_eq!(shutdown_reason(r#"{"return": {}}"#), None);
        assert_eq!(shutdown_reason(r#"{"event": "STOP"}"#), None);
        assert_eq!(shutdown_reason(r#"{"event": "SHUTDOWN"}"#), None);
        assert_eq!(shutdown_reason("not json"), None);
    }

    /// A QMP server that greets, takes the capabilities command, then sends
    /// `events`, one per line.
    fn fake_qmp(events: &'static [&'static str]) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            writeln!(
                stream,
                r#"{{"QMP": {{"version": {{}}, "capabilities": []}}}}"#
            )
            .unwrap();
            let mut got = [0u8; 256];
            let n = stream.read(&mut got).unwrap();
            assert!(String::from_utf8_lossy(&got[..n]).contains("qmp_capabilities"));
            writeln!(stream, r#"{{"return": {{}}}}"#).unwrap();
            for event in events {
                writeln!(stream, "{event}").unwrap();
            }
        });
        port
    }

    #[test]
    fn the_listener_hears_the_first_shutdown_after_other_events() {
        let port = fake_qmp(&[r#"{"event": "RESUME"}"#, RESET, OFF]);
        let reason = listen(port).recv_timeout(Duration::from_secs(10));
        assert_eq!(reason.as_deref(), Ok("guest-reset"));
    }

    #[test]
    fn the_listener_sends_nothing_when_qemu_closes_first() {
        let port = fake_qmp(&[r#"{"event": "STOP"}"#]);
        let got = listen(port).recv_timeout(Duration::from_secs(10));
        assert_eq!(got, Err(mpsc::RecvTimeoutError::Disconnected));
    }
}
