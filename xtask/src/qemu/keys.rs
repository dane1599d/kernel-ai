//! Keys for the guest: through the QEMU monitor, and QMP for `@` lines.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

/// The stages of a case's keys, typed in the order the guest announced them.
/// The monitor serves one client at a time, so two stages started together would
/// take turns in whatever order their connections landed (a Ctrl-O and the path
/// that follows it, the path first): each stage waits for the one before it.
pub(super) struct Typist {
    monitor: u16,
    qmp: Option<u16>,
    last: Option<thread::JoinHandle<()>>,
}

impl Typist {
    pub(super) fn new(monitor: u16, qmp: Option<u16>) -> Self {
        Self {
            monitor,
            qmp,
            last: None,
        }
    }

    /// Types `keys` once the stages sent before have been typed.
    pub(super) fn send(&mut self, keys: &'static [&'static str]) {
        let before = self.last.take();
        let (monitor, qmp) = (self.monitor, self.qmp);
        self.last = Some(thread::spawn(move || {
            if let Some(before) = before {
                let _ = before.join();
            }
            type_keys(monitor, qmp, keys);
        }));
    }
}

/// Types keys into the guest through the QEMU monitor: a key name with
/// `sendkey`, a `!` line as the monitor command after the `!`, an `@` line
/// as a QMP command on `qmp`, and `~` and a number as a wait of that many ms.
fn type_keys(port: u16, qmp: Option<u16>, keys: &[&str]) {
    let mut qmp = qmp.map(Qmp::new);
    for _ in 0..50 {
        if let Ok(mut monitor) = TcpStream::connect(("127.0.0.1", port)) {
            thread::sleep(Duration::from_millis(200));
            for key in keys {
                if let Some(ms) = key.strip_prefix('~').and_then(|ms| ms.parse().ok()) {
                    thread::sleep(Duration::from_millis(ms));
                    continue;
                }
                let sent = match (key.strip_prefix('!'), key.strip_prefix('@')) {
                    (Some(command), _) => writeln!(monitor, "{command}"),
                    (_, Some(command)) => match qmp.as_mut() {
                        Some(qmp) => qmp.run(command),
                        None => Err(std::io::Error::other("no QMP socket for an @ line")),
                    },
                    _ => writeln!(monitor, "sendkey {key}"),
                };
                if sent.is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(150));
            }
            println!("xtask: sent {} inputs through the QEMU monitor", keys.len());
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    eprintln!("xtask: could not reach the QEMU monitor on port {port}");
}

/// A QMP connection, opened on first use: QMP takes one client at a time and
/// wants its capabilities negotiated first.
struct Qmp {
    port: u16,
    stream: Option<BufReader<TcpStream>>,
}

impl Qmp {
    pub(super) fn new(port: u16) -> Self {
        Self { port, stream: None }
    }

    /// Sends one command and waits for its answer, skipping events. The first
    /// command connects and negotiates capabilities.
    fn run(&mut self, command: &str) -> std::io::Result<()> {
        if self.stream.is_none() {
            let stream = TcpStream::connect(("127.0.0.1", self.port))?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            let mut reader = BufReader::new(stream);
            let mut greeting = String::new();
            reader.read_line(&mut greeting)?;
            Self::send(&mut reader, r#"{"execute":"qmp_capabilities"}"#)?;
            self.stream = Some(reader);
        }
        let reader = self.stream.as_mut().expect("connected just above");
        Self::send(reader, command)
    }

    pub(super) fn send(reader: &mut BufReader<TcpStream>, command: &str) -> std::io::Result<()> {
        writeln!(reader.get_mut(), "{command}")?;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Err(std::io::Error::other("QMP closed the connection"));
            }
            if line.contains("\"return\"") {
                return Ok(());
            }
            if line.contains("\"error\"") {
                return Err(std::io::Error::other(format!("QMP: {}", line.trim())));
            }
        }
    }
}
