//! P5-S1's real-host check, run only by `.github/workflows/tls-hosts.yml` on a
//! GitHub runner, never on a developer's PC: a TLS 1.3 handshake with our
//! provider and webpki-roots against the four hosts P5 reaches, then a HEAD
//! request. It prints what each host chose (suite, group) and what its
//! certificates hold (key and signature algorithms), and fails if any host
//! does not handshake.
//!
//! Then it follows the pinned Qwen3-4B source's resolve URL by HEAD, hop by
//! hop, and prints each hop's host and status and the `Location` host it
//! redirects to: the storage hosts a model download reaches, which
//! `modelchan::fetch::HUGGING_FACE` pins (AP-2's ask on #672). It fails if a
//! hop does not handshake.

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

/// The pinned Qwen3-4B source, as `xtask/models.json` names it.
const MODEL_HOST: &str = "huggingface.co";
const MODEL_PATH: &str = "/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/a06e946bb6b655725eafa393f4a9745d460374c9/Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
/// `kainet::http`'s redirect limit.
const MAX_HOPS: usize = 5;
/// The most bytes of a response head read.
const MAX_HEAD: usize = 16 * 1024;

type Tls13 = Session<UnbufferedClientConnection, Tcp>;

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

/// A TLS 1.3 session with `host` on 443, its handshake done, and how long
/// the handshake took.
fn open(host: &str, config: &Arc<ClientConfig>) -> Result<(Tls13, Duration), String> {
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
    Ok((session, start.elapsed()))
}

/// A HEAD of `path` on `host`: the response head, up to its blank line, at
/// most [`MAX_HEAD`] bytes.
fn head(session: &mut Tls13, host: &str, path: &str) -> Result<String, String> {
    let request = format!("HEAD {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    session
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e:?}"))?;
    let mut got = Vec::new();
    let mut buf = [0u8; 2048];
    while !got.windows(4).any(|w| w == b"\r\n\r\n") && got.len() < MAX_HEAD {
        match session.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
            // A server may close without close_notify once it has answered.
            Err(_) if got.starts_with(b"HTTP/") => break,
            Err(e) => return Err(format!("read: {e:?}")),
        }
    }
    Ok(String::from_utf8_lossy(&got).into_owned())
}

/// Where a `Location` sends the next request: its host and path. A relative
/// one stays on `host`.
fn next(host: &str, location: &str) -> Result<(String, String), String> {
    if let Some(rest) = location.strip_prefix("https://") {
        return Ok(match rest.split_once('/') {
            Some((to, path)) => (to.to_string(), format!("/{path}")),
            None => (rest.to_string(), "/".to_string()),
        });
    }
    if location.starts_with('/') {
        return Ok((host.to_string(), location.to_string()));
    }
    let shown = location.split('?').next().unwrap_or("");
    Err(format!("{host}: a Location that is not https: {shown:?}"))
}

/// Follows the pinned model's resolve URL by HEAD, hop by hop, printing each
/// hop's host, path and status and the `Location` host. A query string is
/// never printed: a signed storage URL's holds a short-lived signature.
fn follow(config: &Arc<ClientConfig>) -> Result<(), String> {
    let (mut host, mut path) = (MODEL_HOST.to_string(), MODEL_PATH.to_string());
    for hop in 0..=MAX_HOPS {
        let (mut session, took) = open(&host, config).map_err(|e| format!("{host}: {e}"))?;
        let version = session.tls().connection().protocol_version();
        let head = head(&mut session, &host, &path).map_err(|e| format!("{host}: {e}"))?;
        let status = head.lines().next().unwrap_or("").to_string();
        let shown = path.split('?').next().unwrap_or("");
        println!(
            "model hop {hop}: {host}{shown}: {version:?}, handshake {} ms, {status:?}",
            took.as_millis()
        );
        let location = head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("location")
                .then(|| value.trim().to_string())
        });
        let Some(location) = location else {
            return Ok(());
        };
        (host, path) = next(&host, &location)?;
        println!("  Location host: {host}");
    }
    Err(format!("more than {MAX_HOPS} redirects"))
}

fn check(host: &str, config: &Arc<ClientConfig>) -> Result<(), String> {
    let (mut session, took) = open(host, config)?;
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
    let status = head(&mut session, host, "/")?
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
    let model = follow(&config);
    if let Err(why) = &model {
        println!("the model's resolve URL: FAILED: {why}");
    }
    if failed > 0 || model.is_err() {
        std::process::exit(1);
    }
}
