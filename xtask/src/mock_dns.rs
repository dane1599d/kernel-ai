//! A stand-in DNS server for the network cases: answers the `netcore::fixture`
//! zone over UDP on a free local port, which the guest reaches at 10.0.2.2
//! through QEMU's user networking and is told about with `dns=`. Tests never
//! need the internet.

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

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
        format!("dns=10.0.2.2:{}", self.port)
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
        while let Ok((len, from)) = socket.recv_from(&mut buf) {
            counter.fetch_add(1, Ordering::SeqCst);
            if let Some(reply) = netcore::fixture::answer(&buf[..len])
                && let Err(error) = socket.send_to(&reply, from)
            {
                eprintln!("xtask: mock DNS: {error}");
            }
        }
    });
    Ok(MockDns { port, queries })
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
        assert_eq!(server.cmdline(), format!("dns=10.0.2.2:{}", server.port));
    }
}
