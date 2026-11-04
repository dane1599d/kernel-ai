//! P5-S1's real-host check, run only by `.github/workflows/tls-hosts.yml` on a
//! GitHub runner, never on a developer's PC: a TLS 1.3 handshake with our
//! provider and webpki-roots against the four hosts P5 reaches, then a HEAD
//! request. It prints what each host chose (suite, group) and what its
//! certificates hold (key and signature algorithms), and fails if any host
//! does not handshake.

use kainet::tls::{Session, Transport};
use kainet::{client, provider};
use rustls::ClientConfig;
use rustls::client::UnbufferedClientConnection;
use rustls::crypto::{GetRandomFailed, SecureRandom};
use rustls::pki_types::{ServerName, UnixTime};
use rustls::time_provider::TimeProvider;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// The hosts of plan p5-networking: GitHub, the update repository's Pages
/// host, GitHub's release-assets host, and Hugging Face.
const HOSTS: &[&str] = &[
    "github.com",
    "dane1599d.github.io",
    "release-assets.githubusercontent.com",
    "huggingface.co",
];

#[derive(Debug)]
struct HostRandom;

impl SecureRandom for HostRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        getrandom::fill(buf).map_err(|_| GetRandomFailed)
    }
}

static RANDOM: HostRandom = HostRandom;

#[derive(Debug)]
struct Wall;

impl TimeProvider for Wall {
    fn current_time(&self) -> Option<UnixTime> {
        let since = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?;
        Some(UnixTime::since_unix_epoch(since))
    }
}

struct Tcp(TcpStream);

impl Transport for Tcp {
    fn send(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        self.0.write_all(bytes).map_err(|_| "send failed")
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        self.0.read(buf).map_err(|_| "recv failed")
    }
}

/// The known algorithm OIDs, DER-encoded, that a certificate may name.
const OIDS: &[(&str, &[u8])] = &[
    ("ecPublicKey", &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]),
    ("P-256", &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]),
    ("P-384", &[0x2b, 0x81, 0x04, 0x00, 0x22]),
    (
        "ecdsa-with-SHA256",
        &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02],
    ),
    (
        "ecdsa-with-SHA384",
        &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03],
    ),
    (
        "rsaEncryption",
        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01],
    ),
    (
        "sha256WithRSA",
        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b],
    ),
    (
        "sha384WithRSA",
        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c],
    ),
    (
        "RSASSA-PSS",
        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a],
    ),
    ("Ed25519", &[0x2b, 0x65, 0x70]),
];

/// The algorithm names a DER certificate mentions, as OID tags (0x06).
fn algorithms(der: &[u8]) -> Vec<&'static str> {
    OIDS.iter()
        .filter(|(_, oid)| {
            der.windows(oid.len() + 2)
                .any(|w| w[0] == 0x06 && w[1] as usize == oid.len() && &w[2..] == *oid)
        })
        .map(|(name, _)| *name)
        .collect()
}

fn check(host: &str, config: &Arc<ClientConfig>) -> Result<(), String> {
    let stream = TcpStream::connect((host, 443)).map_err(|e| format!("connect: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let conn = UnbufferedClientConnection::new(config.clone(), name).map_err(|e| e.to_string())?;
    let mut session = Session::new(conn, Tcp(stream));
    let start = Instant::now();
    session
        .handshake()
        .map_err(|e| format!("handshake: {e:?}"))?;
    let took = start.elapsed();
    let conn = session.tls().connection();
    let suite = conn.negotiated_cipher_suite().map(|s| s.suite());
    let group = conn.negotiated_key_exchange_group().map(|g| g.name());
    let version = conn.protocol_version();
    let chain: Vec<Vec<&str>> = conn
        .peer_certificates()
        .unwrap_or_default()
        .iter()
        .map(|cert| algorithms(cert))
        .collect();
    let request = format!("HEAD / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    session
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e:?}"))?;
    let mut buf = [0u8; 512];
    let n = session.read(&mut buf).map_err(|e| format!("read: {e:?}"))?;
    let status = String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    println!(
        "{host}: {version:?}, {suite:?}, {group:?}, handshake {} ms, {status:?}",
        took.as_millis()
    );
    for (i, algs) in chain.iter().enumerate() {
        println!("  certificate {i}: {}", algs.join(", "));
    }
    Ok(())
}

fn main() {
    let provider = Arc::new(provider::provider(&RANDOM));
    let config = client::config(provider, client::mozilla_roots(), Arc::new(Wall))
        .expect("our provider has TLS 1.3 suites");
    let mut failed = 0;
    for host in HOSTS {
        if let Err(why) = check(host, &config) {
            println!("{host}: FAILED: {why}");
            failed += 1;
        }
    }
    println!(
        "{} of {} hosts handshook on TLS 1.3",
        HOSTS.len() - failed,
        HOSTS.len()
    );
    if failed > 0 {
        std::process::exit(1);
    }
}
