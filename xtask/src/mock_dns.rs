//! A stand-in DNS server for the network cases: answers the `netcore::fixture`
//! zone over UDP on a free local port, which the guest reaches at 10.0.2.2
//! through QEMU's user networking and is told about with `dns=`. Tests never
//! need the internet.

use std::collections::HashSet;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

pub struct MockDns {
    pub port: u16,
    queries: Arc<AtomicUsize>,
}

impl MockDns {
    /// Queries received so far, answered or not.
    pub fn queries(&self) -> usize {
        self.queries.load(Ordering::SeqCst)
    }

    /// The kernel command-line word that points the guest here.
    pub fn cmdline(&self) -> String {
        // The mock's fixtures and the HTTPS mock sit on QEMU's 10.0.2.0/24,
        // which only `online=test` lets a name's answer lead to (audit K #17).
        format!("dns=10.0.2.2:{} online=test", self.port)
    }
}

/// Starts the responder on a free local port. It runs until xtask exits.
pub fn start() -> Result<MockDns, String> {
    let socket = UdpSocket::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot start the mock DNS server: {e}"))?;
    let port = socket
        .local_addr()
        .map_err(|e| format!("mock DNS server has no address: {e}"))?
        .port();
    let queries = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&queries);
    thread::spawn(move || {
        let mut buf = [0u8; 1500];
        let mut slow_askers = HashSet::new();
        while let Ok((len, from)) = socket.recv_from(&mut buf) {
            counter.fetch_add(1, Ordering::SeqCst);
            let query = &buf[..len];
            let Some(reply) = netcore::fixture::answer(query) else {
                continue;
            };
            let delay = netcore::fixture::delay_ms(query);
            if delay == 0 {
                send(&socket, &reply, from);
                continue;
            }
            // A slow name is answered once per asking socket, late, from a
            // thread of its own so the queries after it are not held up.
            if !slow_askers.insert(from) {
                continue;
            }
            match socket.try_clone() {
                Ok(late) => {
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(delay));
                        send(&late, &reply, from);
                    });
                }
                Err(error) => eprintln!("xtask: mock DNS: {error}"),
            }
        }
    });
    Ok(MockDns { port, queries })
}

fn send(socket: &UdpSocket, reply: &[u8], to: SocketAddr) {
    if let Err(error) = socket.send_to(reply, to) {
        eprintln!("xtask: mock DNS: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_the_fixture_zone_and_ignores_the_silent_name() {
        let server = start().unwrap();
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .unwrap();
        client.connect(("127.0.0.1", server.port)).unwrap();
        client
            .send(&netcore::dns::query(7, netcore::fixture::KAI))
            .unwrap();
        let mut buf = [0u8; 512];
        let len = client.recv(&mut buf).unwrap();
        let answer = netcore::dns::parse_reply(7, netcore::fixture::KAI, &buf[..len]).unwrap();
        assert_eq!(answer.addresses[0].octets(), netcore::fixture::KAI_ADDRESS);
        client
            .send(&netcore::dns::query(8, netcore::fixture::SILENT))
            .unwrap();
        assert!(client.recv(&mut buf).is_err());
        assert_eq!(server.queries(), 2);
        let cmdline = server.cmdline();
        let words: Vec<&str> = cmdline.split_whitespace().collect();
        assert!(words.contains(&format!("dns=10.0.2.2:{}", server.port).as_str()));
        assert!(words.contains(&"online=test"), "{cmdline}");
    }

    #[test]
    fn the_slow_name_is_answered_once_and_late_without_holding_up_others() {
        use netcore::fixture::{KAI, SLOW, SLOW_ADDRESS, SLOW_DELAY_MS};
        let server = start().unwrap();
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let late = Duration::from_millis(SLOW_DELAY_MS);
        client.set_read_timeout(Some(late * 2)).unwrap();
        client.connect(("127.0.0.1", server.port)).unwrap();
        let started = std::time::Instant::now();
        client.send(&netcore::dns::query(1, SLOW)).unwrap();
        client.send(&netcore::dns::query(2, SLOW)).unwrap();
        client.send(&netcore::dns::query(3, KAI)).unwrap();
        let mut buf = [0u8; 512];
        let len = client.recv(&mut buf).unwrap();
        assert!(netcore::dns::parse_reply(3, KAI, &buf[..len]).is_ok());
        assert!(started.elapsed() < late);
        let len = client.recv(&mut buf).unwrap();
        let slow = netcore::dns::parse_reply(1, SLOW, &buf[..len]).unwrap();
        assert_eq!(slow.addresses[0].octets(), SLOW_ADDRESS);
        assert!(started.elapsed() >= late);
        // The repeat from the same socket is never answered.
        client
            .set_read_timeout(Some(Duration::from_millis(1_000)))
            .unwrap();
        assert!(client.recv(&mut buf).is_err());
        assert_eq!(server.queries(), 3);
    }
}
