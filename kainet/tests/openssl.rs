//! kainet's client against a second implementation, `openssl s_server`
//! (plan p5-networking §6), on this host only: it catches a provider that is
//! only consistent with itself. openssl runs here and never ships; where it
//! is not installed the test says so and checks nothing.
//!
//! Its certificates come from openssl too, so they hold what rcgen cannot
//! make: an RSA CA (PKCS#1 v1.5 signatures on the chain) and RSA server keys
//! (RSA-PSS handshake signatures), besides a P-384 key. Each suite and group
//! is asked for by name; a TLS 1.2-only server and a 1024-bit RSA key are
//! refused.

use kainet::client::{self, Wall};
use kainet::tls::{Session, TlsError, Transport};
use rustls::client::UnbufferedClientConnection;
use rustls::crypto::{CryptoProvider, GetRandomFailed, SecureRandom};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{AlertDescription, CertificateError, CipherSuite, Error, NamedGroup, RootCertStore};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

const NAME: &str = "openssl.test";

#[derive(Debug)]
struct HostRandom;

impl SecureRandom for HostRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        getrandom::fill(buf).map_err(|_| GetRandomFailed)
    }
}

static RANDOM: HostRandom = HostRandom;

fn ours() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(kainet::provider::provider(&RANDOM)))
        .clone()
}

fn now_s() -> Option<u64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn openssl_here() -> bool {
    Command::new("openssl")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn run(dir: &Path, args: &[&str]) {
    let out = Command::new("openssl")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("openssl runs");
    assert!(
        out.status.success(),
        "openssl {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const LEAF_EXT: &str = "[v3]\nsubjectAltName=DNS:openssl.test\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n";

/// An RSA-2048 CA in `dir`, and leaves signed by it: `rsa2048`, `rsa1024`
/// and `p384`. The CA's certificate in DER.
fn certificates(dir: &Path) -> Vec<u8> {
    std::fs::write(dir.join("leaf.ext"), LEAF_EXT).unwrap();
    run(
        dir,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "ca.key",
            "-out",
            "ca.pem",
            "-days",
            "3650",
            "-subj",
            "/CN=kainet openssl CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ],
    );
    for (name, key) in [
        ("rsa2048", &["-newkey", "rsa:2048"][..]),
        ("rsa1024", &["-newkey", "rsa:1024"][..]),
        (
            "p384",
            &["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-384"][..],
        ),
    ] {
        let (keyfile, csr, pem) = (
            format!("{name}.key"),
            format!("{name}.csr"),
            format!("{name}.pem"),
        );
        let mut args = vec!["req"];
        args.extend_from_slice(key);
        args.extend_from_slice(&[
            "-nodes",
            "-keyout",
            &keyfile,
            "-out",
            &csr,
            "-subj",
            "/CN=openssl.test",
        ]);
        run(dir, &args);
        run(
            dir,
            &[
                "x509",
                "-req",
                "-in",
                &csr,
                "-CA",
                "ca.pem",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                &pem,
                "-days",
                "3650",
                "-sha256",
                "-extfile",
                "leaf.ext",
                "-extensions",
                "v3",
            ],
        );
    }
    run(
        dir,
        &["x509", "-in", "ca.pem", "-outform", "DER", "-out", "ca.der"],
    );
    std::fs::read(dir.join("ca.der")).unwrap()
}

/// `openssl s_server` for one connection with leaf `name` and `extra`
/// options, once it is listening; its port.
fn serve(dir: &Path, name: &str, extra: &[&str]) -> (Child, u16) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let accept = format!("127.0.0.1:{port}");
    let (cert, key) = (format!("{name}.pem"), format!("{name}.key"));
    let mut args = vec![
        "s_server", "-accept", &accept, "-cert", &cert, "-key", &key, "-www", "-naccept", "1",
    ];
    args.extend_from_slice(extra);
    let mut child = Command::new("openssl")
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("s_server starts");
    let stdout = child.stdout.take().unwrap();
    for line in BufReader::new(stdout).lines() {
        if line.unwrap_or_default().contains("ACCEPT") {
            break;
        }
    }
    (child, port)
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

/// What a connection to `port` negotiated, and the first line s_server's
/// status page sent back.
fn talk(ca: &[u8], port: u16) -> Result<(CipherSuite, NamedGroup, String), TlsError> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(ca.to_vec())).unwrap();
    let wall = Wall {
        now_s,
        floor_s: client::FLOOR_S,
    };
    let config = client::config(ours(), roots, Arc::new(wall)).unwrap();
    let conn =
        UnbufferedClientConnection::new(config, ServerName::try_from(NAME).unwrap()).unwrap();
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut session = Session::new(conn, Tcp(stream));
    session.handshake()?;
    let suite = session
        .tls()
        .connection()
        .negotiated_cipher_suite()
        .unwrap()
        .suite();
    let group = session
        .tls()
        .connection()
        .negotiated_key_exchange_group()
        .unwrap()
        .name();
    session.write_all(b"GET / HTTP/1.0\r\n\r\n")?;
    let mut page = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match session.read(&mut buf) {
            Ok(0) | Err(TlsError::Truncated) => break,
            Ok(n) => page.extend_from_slice(&buf[..n]),
            Err(error) => return Err(error),
        }
    }
    let first = String::from_utf8_lossy(&page)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    Ok((suite, group, first))
}

/// A directory of test `name`'s own. The tests run in parallel, and each
/// removes its directory when done, so none may hold another's.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kainet-openssl-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn our_client_and_openssl_s_server_agree() {
    if !openssl_here() {
        eprintln!("openssl is not installed here: nothing checked");
        return;
    }
    let dir = scratch("agree");
    let ca = certificates(&dir);
    let cases: [(&str, &[&str], CipherSuite, NamedGroup); 5] = [
        (
            "rsa2048",
            &[
                "-tls1_3",
                "-ciphersuites",
                "TLS_AES_128_GCM_SHA256",
                "-groups",
                "X25519",
            ],
            CipherSuite::TLS13_AES_128_GCM_SHA256,
            NamedGroup::X25519,
        ),
        (
            "rsa2048",
            &[
                "-tls1_3",
                "-ciphersuites",
                "TLS_AES_256_GCM_SHA384",
                "-groups",
                "P-256",
            ],
            CipherSuite::TLS13_AES_256_GCM_SHA384,
            NamedGroup::secp256r1,
        ),
        (
            "rsa2048",
            &[
                "-tls1_3",
                "-ciphersuites",
                "TLS_CHACHA20_POLY1305_SHA256",
                "-groups",
                "X25519",
            ],
            CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
            NamedGroup::X25519,
        ),
        (
            "p384",
            &[
                "-tls1_3",
                "-ciphersuites",
                "TLS_AES_128_GCM_SHA256",
                "-groups",
                "P-256",
            ],
            CipherSuite::TLS13_AES_128_GCM_SHA256,
            NamedGroup::secp256r1,
        ),
        (
            "p384",
            &[
                "-tls1_3",
                "-ciphersuites",
                "TLS_CHACHA20_POLY1305_SHA256",
                "-groups",
                "X25519",
            ],
            CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
            NamedGroup::X25519,
        ),
    ];
    for (leaf, options, suite, group) in cases {
        let (mut child, port) = serve(&dir, leaf, options);
        let got = talk(&ca, port);
        let _ = child.kill();
        let (got_suite, got_group, first) =
            got.unwrap_or_else(|e| panic!("{leaf} {options:?}: {e:?}"));
        assert_eq!((got_suite, got_group), (suite, group), "{leaf} {options:?}");
        assert!(first.starts_with("HTTP/1.0 200"), "{leaf}: {first:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tls12_only_server_and_a_1024_bit_key_are_refused() {
    if !openssl_here() {
        eprintln!("openssl is not installed here: nothing checked");
        return;
    }
    let dir = scratch("refused");
    let ca = certificates(&dir);
    // openssl says TLS 1.2 is all it has; our check refuses the short key's
    // handshake signature.
    let tls12 = TlsError::Tls(Error::AlertReceived(AlertDescription::ProtocolVersion));
    let short = TlsError::Tls(Error::InvalidCertificate(CertificateError::BadSignature));
    for (leaf, options, refused) in [
        ("rsa2048", &["-tls1_2"][..], tls12),
        (
            "rsa1024",
            &["-tls1_3", "-cipher", "DEFAULT@SECLEVEL=0"][..],
            short,
        ),
    ] {
        let (mut child, port) = serve(&dir, leaf, options);
        let got = talk(&ca, port);
        let _ = child.kill();
        assert_eq!(got.err(), Some(refused), "{leaf} {options:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
