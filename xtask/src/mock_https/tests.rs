//! The mock through `kainet`'s own client over host TCP, with plain HTTP
//! written by hand (task 5's client is not here yet): each host, each layout,
//! each fault, and what a hostile client gets.

use super::*;
use kainet::tls::TlsError;
use rustls::client::UnbufferedClientConnection;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::time_provider::TimeProvider;
use rustls::{CertificateError, ClientConfig, Error, RootCertStore};
use std::time::Instant;

/// 2030-01-01: after the expired certificates' end, inside the good ones.
const NOW_S: u64 = 1_893_456_000;

#[derive(Debug)]
struct At;

impl TimeProvider for At {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(NOW_S)))
    }
}

fn file(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

fn mock() -> Mock {
    let content = Content::new()
        .repository([
            (
                "metadata/timestamp.json".to_string(),
                b"{\"signed\":{}}".to_vec(),
            ),
            ("targets/blob".to_string(), file(1000)),
        ])
        .release_asset("v0.2.0", "weather.kpkg", file(3000))
        .model("Qwen/Qwen3-4B-GGUF", "abc123", "model.gguf", file(5000));
    start(content).unwrap()
}

/// A response: its status, headers (names lower-cased) and body.
#[derive(Debug)]
struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

fn connect(mock: &Mock, sni: &str) -> Session<UnbufferedClientConnection, Tcp> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(mock.ca().to_vec())).unwrap();
    let config = ClientConfig::builder_with_details(pki::provider(), Arc::new(At))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from(sni.to_string()).unwrap();
    let conn = UnbufferedClientConnection::new(Arc::new(config), name).unwrap();
    let stream = TcpStream::connect(("127.0.0.1", mock.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    Session::new(conn, Tcp(stream))
}

/// Sends `head` to `sni` and reads what comes back until the connection
/// ends: the bytes, and how it ended.
fn exchange(
    mock: &Mock,
    sni: &str,
    head: &str,
) -> Result<(Vec<u8>, Result<(), TlsError>), TlsError> {
    let mut session = connect(mock, sni);
    session.handshake()?;
    session.write_all(head.as_bytes())?;
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match session.read(&mut buf) {
            Ok(0) => return Ok((got, Ok(()))),
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(error) => return Ok((got, Err(error))),
        }
    }
}

fn split(bytes: &[u8]) -> Response {
    let end = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a whole head");
    let head = std::str::from_utf8(&bytes[..end]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap()[9..12].parse().unwrap();
    let headers = lines
        .map(|l| {
            let (n, v) = l.split_once(": ").unwrap();
            (n.to_ascii_lowercase(), v.to_string())
        })
        .collect();
    Response {
        status,
        headers,
        body: bytes[end + 4..].to_vec(),
    }
}

/// A whole response from `method path` on `host`, with `extra` header lines.
fn fetch(mock: &Mock, host: &str, method: &str, path: &str, extra: &str) -> Response {
    let head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n");
    let (bytes, end) = exchange(mock, host, &head).unwrap();
    assert_eq!(end, Ok(()), "the response ended with a close_notify");
    split(&bytes)
}

fn refusal(mock: &Mock, host: &str) -> Error {
    match exchange(mock, host, "GET / HTTP/1.1\r\n\r\n") {
        Err(TlsError::Tls(error)) => error,
        other => panic!("the handshake went on: {other:?}"),
    }
}

#[test]
fn a_file_comes_whole_by_range_and_by_head() {
    let mock = mock();
    let whole = fetch(&mock, REPO, "GET", "/targets/blob", "");
    assert_eq!((whole.status, whole.body.clone()), (200, file(1000)));
    assert_eq!(whole.header("content-length"), Some("1000"));
    let part = fetch(
        &mock,
        REPO,
        "GET",
        "/targets/blob",
        "Range: bytes=10-19\r\n",
    );
    assert_eq!(part.status, 206);
    assert_eq!(part.body, file(1000)[10..20]);
    assert_eq!(part.header("content-range"), Some("bytes 10-19/1000"));
    let tail = fetch(&mock, REPO, "GET", "/targets/blob", "Range: bytes=-5\r\n");
    assert_eq!(tail.body, file(1000)[995..]);
    let head = fetch(&mock, REPO, "HEAD", "/targets/blob", "");
    assert_eq!((head.status, head.body.len()), (200, 0));
    assert_eq!(head.header("content-length"), Some("1000"));
    let past = fetch(
        &mock,
        REPO,
        "GET",
        "/targets/blob",
        "Range: bytes=1000-\r\n",
    );
    assert_eq!(past.status, 416);
    assert_eq!(past.header("content-range"), Some("bytes */1000"));
    assert_eq!(fetch(&mock, REPO, "GET", "/nothing", "").status, 404);
}

#[test]
fn a_release_download_goes_by_the_assets_host_and_a_model_by_its_resolve_path() {
    let mock = mock();
    let path = "/dane1599d/kernel-ai-updates/releases/download/v0.2.0/weather.kpkg";
    let found = fetch(&mock, RELEASES, "GET", path, "");
    assert_eq!(found.status, 302);
    let location = found.header("location").unwrap();
    assert_eq!(
        location,
        "https://assets.kai.test/assets/v0.2.0/weather.kpkg"
    );
    let asset = fetch(&mock, ASSETS, "GET", &location[23..], "");
    assert_eq!((asset.status, asset.body), (200, file(3000)));
    let model = fetch(
        &mock,
        MODELS,
        "GET",
        "/Qwen/Qwen3-4B-GGUF/resolve/abc123/model.gguf",
        "",
    );
    assert_eq!((model.status, model.body.len()), (200, 5000));
    let metadata = fetch(&mock, REPO, "GET", "/metadata/timestamp.json", "");
    assert_eq!(metadata.body, b"{\"signed\":{}}");
}

#[test]
fn a_host_header_that_is_not_the_sni_is_misdirected() {
    let mock = mock();
    let head = format!("GET /targets/blob HTTP/1.1\r\nHost: {ASSETS}\r\n\r\n");
    let (bytes, _) = exchange(&mock, REPO, &head).unwrap();
    assert_eq!(split(&bytes).status, 421);
}

#[test]
fn a_host_the_mock_does_not_serve_gets_no_certificate() {
    let mock = mock();
    let error = refusal(&mock, "nowhere.kai.test");
    assert!(matches!(error, Error::AlertReceived(_)), "{error:?}");
}

#[test]
fn each_certificate_fault_is_refused_by_name_and_only_on_its_host() {
    let mock = mock();
    mock.set_fault(REPO, "/anything", Fault::Expired);
    assert!(matches!(
        refusal(&mock, REPO),
        Error::InvalidCertificate(CertificateError::ExpiredContext { .. })
    ));
    assert_eq!(fetch(&mock, ASSETS, "GET", "/nothing", "").status, 404);
    mock.set_fault(REPO, "*", Fault::WrongName);
    assert!(matches!(
        refusal(&mock, REPO),
        Error::InvalidCertificate(CertificateError::NotValidForNameContext { .. })
    ));
    mock.set_fault(REPO, "*", Fault::UnknownCa);
    assert_eq!(
        refusal(&mock, REPO),
        Error::InvalidCertificate(CertificateError::UnknownIssuer)
    );
    mock.clear_faults();
    assert_eq!(fetch(&mock, REPO, "GET", "/targets/blob", "").status, 200);
}

#[test]
fn a_cut_ends_the_body_at_its_byte_with_no_close_notify() {
    let mock = mock();
    mock.set_fault(REPO, "/targets/blob", Fault::CutAt(100));
    let head = format!("GET /targets/blob HTTP/1.1\r\nHost: {REPO}\r\n\r\n");
    let (bytes, end) = exchange(&mock, REPO, &head).unwrap();
    assert_eq!(end, Err(TlsError::Truncated));
    let response = split(&bytes);
    assert_eq!(response.header("content-length"), Some("1000"));
    assert_eq!(response.body, file(1000)[..100]);
}

#[test]
fn a_flipped_byte_is_the_files_byte_in_any_range() {
    let mock = mock();
    mock.set_fault(REPO, "/targets/blob", Fault::Flip(12));
    let whole = fetch(&mock, REPO, "GET", "/targets/blob", "");
    let differ: Vec<usize> = (0..1000)
        .filter(|&i| whole.body[i] != file(1000)[i])
        .collect();
    assert_eq!(differ, [12]);
    let part = fetch(
        &mock,
        REPO,
        "GET",
        "/targets/blob",
        "Range: bytes=10-19\r\n",
    );
    assert_eq!(part.body[2], file(1000)[12] ^ 1);
    let after = fetch(
        &mock,
        REPO,
        "GET",
        "/targets/blob",
        "Range: bytes=20-29\r\n",
    );
    assert_eq!(after.body, file(1000)[20..30]);
}

#[test]
fn too_many_requests_says_when_to_come_back() {
    let mock = mock();
    mock.set_fault(MODELS, "*", Fault::TooMany(7));
    let got = fetch(
        &mock,
        MODELS,
        "GET",
        "/Qwen/Qwen3-4B-GGUF/resolve/abc123/model.gguf",
        "",
    );
    assert_eq!((got.status, got.header("retry-after")), (429, Some("7")));
}

#[test]
fn a_slow_link_takes_its_time() {
    let mock = mock();
    mock.set_fault(REPO, "/targets/blob", Fault::Slow(4000));
    let start = Instant::now();
    let got = fetch(&mock, REPO, "GET", "/targets/blob", "");
    assert_eq!(got.body, file(1000));
    assert!(
        start.elapsed() >= Duration::from_millis(200),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn a_redirect_off_the_handles_hosts_names_another_host() {
    let mock = mock();
    mock.set_fault(ASSETS, "*", Fault::RedirectOff);
    let got = fetch(&mock, ASSETS, "GET", "/assets/v0.2.0/weather.kpkg", "");
    assert_eq!(got.status, 302);
    assert_eq!(
        got.header("location"),
        Some("https://elsewhere.kai.test/assets/v0.2.0/weather.kpkg")
    );
}

#[test]
fn hostile_requests_get_a_status_and_nothing_else() {
    let mock = mock();
    let status = |head: &str| {
        let (bytes, _) = exchange(&mock, REPO, head).unwrap();
        split(&bytes).status
    };
    assert_eq!(
        status("POST / HTTP/1.1\r\nHost: updates.kai.test\r\n\r\n"),
        405
    );
    assert_eq!(status("GET / HTTP/1.1\r\n\r\n"), 400);
    assert_eq!(
        status(
            "GET /targets/blob HTTP/1.1\r\nHost: updates.kai.test\r\nRange: bytes=1-2,5-6\r\n\r\n"
        ),
        416
    );
    let long = format!(
        "GET / HTTP/1.1\r\nHost: updates.kai.test\r\nX-Long: {}\r\n\r\n",
        "a".repeat(MAX_HEAD + 10)
    );
    assert_eq!(status(&long), 431);
}

#[test]
fn every_answer_is_logged_with_its_range() {
    let mock = mock();
    fetch(&mock, REPO, "GET", "/targets/blob", "Range: bytes=0-9\r\n");
    fetch(&mock, REPO, "HEAD", "/nothing", "");
    assert_eq!(
        mock.served(),
        [
            Served {
                host: REPO.into(),
                method: "GET".into(),
                path: "/targets/blob".into(),
                range: Some((0, 10)),
                status: 206,
            },
            Served {
                host: REPO.into(),
                method: "HEAD".into(),
                path: "/nothing".into(),
                range: None,
                status: 404,
            },
        ]
    );
}

#[test]
fn connections_past_the_cap_are_closed_at_once() {
    let mock = mock();
    let held: Vec<TcpStream> = (0..MAX_CONNECTIONS)
        .map(|_| TcpStream::connect(("127.0.0.1", mock.port)).unwrap())
        .collect();
    let mut over = TcpStream::connect(("127.0.0.1", mock.port)).unwrap();
    over.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut byte = [0u8; 1];
    // Closed, not left to wait: an end of stream, or a reset.
    match over.read(&mut byte) {
        Ok(0) => {}
        Ok(n) => panic!("{n} bytes from a connection past the cap"),
        Err(e) => assert!(
            e.kind() != std::io::ErrorKind::WouldBlock && e.kind() != std::io::ErrorKind::TimedOut,
            "{e}"
        ),
    }
    drop(held);
    // The held connections end and free their places.
    let start = Instant::now();
    loop {
        let head = format!("GET /targets/blob HTTP/1.1\r\nHost: {REPO}\r\n\r\n");
        if let Ok((bytes, Ok(()))) = exchange(&mock, REPO, &head) {
            assert_eq!(split(&bytes).status, 200);
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no place freed");
        thread::sleep(Duration::from_millis(50));
    }
}
