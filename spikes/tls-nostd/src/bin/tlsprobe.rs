//! P5-S1's real-host check, run only on a GitHub runner (plan §5: "the real
//! hosts are reached only from a GitHub runner"): a TLS 1.3 handshake with each
//! host on our provider and Mozilla's roots, then one `GET /` whose status line it reads, read-only. It
//! prints what each host negotiated: the suite, the key-exchange group, the
//! CertificateVerify scheme, and the signature algorithms its chain needed.
//!
//! `tlsprobe [--root ROOT.der] HOST[:PORT][@CONNECT_ADDR]...`; with no hosts it
//! probes the four of the plan. `--root` trusts that one root instead of
//! Mozilla's, for the CI job's `openssl s_server` interop check.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{UnbufferedClientConnection, WebPkiServerVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use spike_tls::pipe::{self, PipeError};
use spike_tls::verify::{COUNTS, NAMES};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HOSTS: [&str; 4] = [
    "github.com",
    "dane1599d.github.io",
    "release-assets.githubusercontent.com",
    "huggingface.co",
];
const TIMEOUT: Duration = Duration::from_secs(15);

/// Records the CertificateVerify scheme, and passes everything to webpki.
#[derive(Debug)]
struct Recording {
    inner: Arc<WebPkiServerVerifier>,
    scheme: Mutex<Option<SignatureScheme>>,
}

impl ServerCertVerifier for Recording {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        *self.scheme.lock().expect("lock") = Some(dss.scheme);
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

fn counts() -> [u32; 9] {
    core::array::from_fn(|i| COUNTS[i].load(Ordering::Relaxed))
}

struct Target {
    name: String,
    port: u16,
    connect: Option<String>,
}

fn parse(arg: &str) -> Target {
    let (host, connect) = match arg.split_once('@') {
        Some((host, addr)) => (host, Some(addr.to_string())),
        None => (arg, None),
    };
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, port.parse().unwrap_or(443)),
        None => (host, 443),
    };
    Target {
        name: name.to_string(),
        port,
        connect,
    }
}

fn probe(target: &Target, roots: &RootCertStore) -> Result<String, String> {
    let provider = Arc::new(spike_tls::provider(spike_tls::suites()));
    let inner =
        WebPkiServerVerifier::builder_with_provider(Arc::new(roots.clone()), provider.clone())
            .build()
            .map_err(|e| format!("verifier: {e}"))?;
    let verifier = Arc::new(Recording {
        inner,
        scheme: Mutex::new(None),
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| format!("config: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    let name = ServerName::try_from(target.name.clone()).map_err(|e| format!("name: {e}"))?;
    let addr = target
        .connect
        .clone()
        .unwrap_or_else(|| format!("{}:{}", target.name, target.port));

    let before = counts();
    let started = Instant::now();
    let mut socket = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    socket.set_read_timeout(Some(TIMEOUT)).ok();
    let connected = started.elapsed();
    let mut conn =
        UnbufferedClientConnection::new(Arc::new(config), name).map_err(|e| format!("{e}"))?;
    let (mut incoming, mut outgoing) = (Vec::new(), Vec::new());
    let cpu = std::cell::Cell::new(Duration::ZERO);
    let mut buf = vec![0u8; 64 * 1024];
    let step = |conn: &mut UnbufferedClientConnection,
                incoming: &mut Vec<u8>,
                outgoing: &mut Vec<u8>,
                send: &mut &[u8],
                sink: &mut dyn FnMut(&[u8])|
     -> Result<pipe::Turn, String> {
        let t = Instant::now();
        let turn = pipe::turn(conn, incoming, outgoing, send, usize::MAX, sink);
        cpu.set(cpu.get() + t.elapsed());
        turn.map_err(|e| match e {
            PipeError::Tls(e) => format!("TLS: {e}"),
            other => format!("{other:?}"),
        })
    };
    loop {
        let turn = step(
            &mut conn,
            &mut incoming,
            &mut outgoing,
            &mut &[][..],
            &mut |_| {},
        )?;
        socket
            .write_all(&outgoing)
            .map_err(|e| format!("send: {e}"))?;
        outgoing.clear();
        if turn.ready {
            break;
        }
        let n = socket
            .read(&mut buf)
            .map_err(|e| format!("receive during the handshake: {e}"))?;
        if n == 0 {
            return Err("the server closed during the handshake".into());
        }
        incoming.extend_from_slice(&buf[..n]);
    }
    let handshake = started.elapsed();
    let client_cpu = cpu.get();

    let request = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nUser-Agent: kai-p5-s1-tlsprobe\r\nConnection: close\r\n\r\n",
        target.name
    );
    let mut response = Vec::new();
    step(
        &mut conn,
        &mut incoming,
        &mut outgoing,
        &mut request.as_bytes(),
        &mut |_| {},
    )?;
    socket
        .write_all(&outgoing)
        .map_err(|e| format!("send: {e}"))?;
    outgoing.clear();
    while !response.windows(2).any(|w| w == b"\r\n") {
        let n = socket.read(&mut buf).map_err(|e| format!("receive: {e}"))?;
        if n == 0 {
            break;
        }
        incoming.extend_from_slice(&buf[..n]);
        let turn = step(
            &mut conn,
            &mut incoming,
            &mut outgoing,
            &mut &[][..],
            &mut |payload| response.extend_from_slice(payload),
        )?;
        if turn.closed {
            break;
        }
    }
    let status = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or("(no response)")
        .to_string();

    let after = counts();
    let used: Vec<String> = (0..NAMES.len())
        .filter(|&i| after[i] > before[i])
        .map(|i| format!("{} x{}", NAMES[i], after[i] - before[i]))
        .collect();
    let leaf = conn
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|c| leaf_key(c.as_ref()))
        .unwrap_or("?");
    let chain_len = conn.peer_certificates().map_or(0, |c| c.len());
    Ok(format!(
        "{}:{} TLS 1.3 OK: suite {:?}, group {:?}, CertificateVerify {:?}, leaf key {leaf}, \
         {chain_len} certificates sent, signatures checked: {}; TCP connect {} ms, \
         handshake {} ms wall, {} ms client CPU; GET / -> {status}",
        target.name,
        target.port,
        conn.negotiated_cipher_suite().map(|s| s.suite()),
        conn.negotiated_key_exchange_group().map(|g| g.name()),
        verifier.scheme.lock().expect("lock").as_ref(),
        used.join(", "),
        connected.as_millis(),
        handshake.as_millis(),
        client_cpu.as_millis(),
    ))
}

/// The leaf's key type, from the SubjectPublicKeyInfo's algorithm OIDs.
fn leaf_key(der: &[u8]) -> &'static str {
    const EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
    const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
    const P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
    const RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    const ED25519: &[u8] = &[0x2b, 0x65, 0x70];
    let has = |oid: &[u8]| der.windows(oid.len()).any(|w| w == oid);
    match () {
        _ if has(EC) && has(P256) => "ECDSA P-256",
        _ if has(EC) && has(P384) => "ECDSA P-384",
        _ if has(RSA) => "RSA",
        _ if has(ED25519) => "Ed25519",
        _ => "other",
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let roots = match args.iter().position(|a| a == "--root") {
        Some(i) if i + 1 < args.len() => {
            let path = args.remove(i + 1);
            args.remove(i);
            let der = match std::fs::read(&path) {
                Ok(der) => der,
                Err(e) => {
                    eprintln!("tlsprobe: cannot read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let mut roots = RootCertStore::empty();
            if let Err(e) = roots.add(CertificateDer::from(der)) {
                eprintln!("tlsprobe: {path} is not a usable root: {e}");
                return ExitCode::FAILURE;
            }
            roots
        }
        _ => spike_tls::mozilla_roots(),
    };
    let targets: Vec<Target> = if args.is_empty() {
        HOSTS.iter().map(|h| parse(h)).collect()
    } else {
        args.iter().map(|a| parse(a)).collect()
    };
    let mut ok = true;
    for target in &targets {
        match probe(target, &roots) {
            Ok(line) => println!("tlsprobe: {line}"),
            Err(error) => {
                ok = false;
                println!("tlsprobe: {}:{} FAILED: {error}", target.name, target.port);
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
