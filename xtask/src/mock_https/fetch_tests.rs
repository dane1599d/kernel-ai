//! The host half of P5 task 12: `kainet::fetch`, as the `fetch` service will
//! run an app's `kai:net/http` request, against the mock over TLS. An app
//! granted the model host fetches from it, and is refused another host by
//! its URL or by a redirect, a body over 64 KiB and a bad certificate.

use super::*;
use kainet::client::{self, Wall};
use kainet::fetch::{self, Error, Grant, MAX_BODY, Request};
use kainet::http::{Dial, HttpError, Method, TlsConnect};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;

fn at_2030() -> Option<u64> {
    Some(1_893_456_000)
}

struct Local(u16);

impl Dial for Local {
    type Transport = Tcp;

    fn dial(&mut self, _: &str, _: u16) -> Result<Tcp, HttpError> {
        let stream = TcpStream::connect(("127.0.0.1", self.0))
            .map_err(|_| HttpError::Connect("the mock refused"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|_| HttpError::Connect("no timeout"))?;
        Ok(Tcp(stream))
    }
}

fn connector(mock: &Mock) -> TlsConnect<Local> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(mock.ca().to_vec())).unwrap();
    let wall = Wall {
        now_s: at_2030,
        floor_s: client::FLOOR_S,
    };
    TlsConnect {
        config: client::config(pki::provider(), roots, Arc::new(wall)).unwrap(),
        dial: Local(mock.port),
    }
}

const MODEL: &str = "/Qwen/Qwen3-4B-GGUF/resolve/abc123/config.json";

fn mock() -> Mock {
    let content = Content::new()
        .model(
            "Qwen/Qwen3-4B-GGUF",
            "abc123",
            "config.json",
            b"{\"layers\":36}".to_vec(),
        )
        .model(
            "Qwen/Qwen3-4B-GGUF",
            "abc123",
            "big.bin",
            vec![7; MAX_BODY + 1],
        )
        .repository([("metadata/timestamp.json".to_string(), b"{}".to_vec())]);
    start(content).unwrap()
}

fn get(url: &str) -> Request {
    Request {
        url: url.into(),
        method: Method::Get,
        range: None,
    }
}

fn serve(mock: &Mock, grant: &Grant, request: &Request) -> Result<fetch::Response, Error> {
    let mut connect: TlsConnect<Local> = connector(mock);
    let zero = || 0;
    fetch::serve(grant, request, &mut connect, &zero, u64::MAX)
}

#[test]
fn an_app_fetches_from_its_granted_host() {
    let mock = mock();
    let grant = Grant::new(&["models.kai.test"]).unwrap();
    let got = serve(&mock, &grant, &get(&format!("https://{MODELS}{MODEL}"))).unwrap();
    assert_eq!(got.status, 200);
    assert_eq!(got.body, b"{\"layers\":36}");
    assert_eq!(got.url, format!("https://{MODELS}{MODEL}"));
    assert_eq!(
        got.content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(got.retry_after, None);
}

/// A 429 is a response, with when to come back, not an error (C's review).
#[test]
fn a_busy_host_answers_with_when_to_come_back() {
    let mock = mock();
    mock.set_fault(MODELS, MODEL, Fault::TooMany(7));
    let grant = Grant::new(&["models.kai.test"]).unwrap();
    let got = serve(&mock, &grant, &get(&format!("https://{MODELS}{MODEL}"))).unwrap();
    assert_eq!(
        (got.status, got.retry_after, got.body.len()),
        (429, Some(7), 0)
    );
}

#[test]
fn another_host_is_refused_before_it_is_reached() {
    let mock = mock();
    let grant = Grant::new(&["models.kai.test"]).unwrap();
    let url = format!("https://{REPO}/metadata/timestamp.json");
    assert_eq!(
        serve(&mock, &grant, &get(&url)),
        Err(Error::Refused(REPO.into()))
    );
    let port = format!("https://{MODELS}:8443{MODEL}");
    assert_eq!(
        serve(&mock, &grant, &get(&port)),
        Err(Error::Refused(MODELS.into()))
    );
    assert!(mock.served().is_empty(), "nothing reached the mock");
}

#[test]
fn a_redirect_off_the_grant_is_refused() {
    let mock = mock();
    mock.set_fault(MODELS, MODEL, Fault::RedirectOff);
    let grant = Grant::new(&["models.kai.test"]).unwrap();
    let got = serve(&mock, &grant, &get(&format!("https://{MODELS}{MODEL}")));
    assert_eq!(got, Err(Error::Refused(OFF.into())));
    assert!(mock.served().iter().all(|s| s.host != OFF));
}

#[test]
fn a_body_over_the_cap_and_a_bad_certificate_are_named() {
    let mock = mock();
    let grant = Grant::new(&["models.kai.test"]).unwrap();
    let big = format!("https://{MODELS}/Qwen/Qwen3-4B-GGUF/resolve/abc123/big.bin");
    assert_eq!(serve(&mock, &grant, &get(&big)), Err(Error::TooLarge));
    mock.set_fault(MODELS, "*", Fault::Expired);
    let got = serve(&mock, &grant, &get(&format!("https://{MODELS}{MODEL}")));
    assert!(
        matches!(&got, Err(Error::Tls(why)) if why.contains("Expired")),
        "{got:?}"
    );
    assert!(matches!(
        serve(&mock, &grant, &get("http://models.kai.test/")),
        Err(Error::BadUrl(_))
    ));
}

/// The WIT draft is valid WIT, with the world the service will serve.
#[test]
fn the_kai_net_draft_parses() {
    let path = crate::workspace_root().join("kainet/wit/kai-net/http.wit");
    let mut resolve = wit_parser::Resolve::default();
    let package = resolve.push_file(&path).unwrap();
    let name = &resolve.packages[package].name;
    assert_eq!(format!("{name}"), "kai:net@0.1.0");
    let world = resolve
        .select_world(&[package], Some("http-client"))
        .unwrap();
    assert_eq!(resolve.worlds[world].imports.len(), 1);
    assert_eq!(resolve.worlds[world].exports.len(), 1);
}

/// `serve_within`, as the `fetch` service runs a call (P5 task 12): on a
/// handle for one host, a redirect within it is followed, one to another
/// host comes back unfollowed with its Location made absolute, and a call
/// with all five hops taken is refused.
#[test]
fn the_service_follows_only_redirects_within_its_handle() {
    let content = Content::new()
        .file(REPO, "/hello", b"hello".to_vec())
        .redirect(REPO, "/old", "/hello")
        .redirect(REPO, "/away", &format!("https://{ASSETS}/blob"));
    let mock = start(content).unwrap();
    let only_repo = |host: &str, port: u16| port == 443 && host == REPO;
    let zero = || 0;
    let within = |path: &str, hops: u8| {
        let mut connect: TlsConnect<Local> = connector(&mock);
        let request = get(&format!("https://{REPO}{path}"));
        fetch::serve_within(&request, hops, &only_repo, &mut connect, &zero, u64::MAX)
    };
    let (got, location) = within("/old", 0).unwrap();
    assert_eq!(
        (got.status, got.body.as_slice(), location),
        (200, &b"hello"[..], None)
    );
    assert_eq!(got.url, format!("https://{REPO}/hello"));
    let (got, location) = within("/away", 0).unwrap();
    assert_eq!(
        (
            got.status,
            got.body.len(),
            got.url.as_str(),
            location.as_deref()
        ),
        (
            302,
            0,
            format!("https://{REPO}/away").as_str(),
            Some(format!("https://{ASSETS}/blob").as_str())
        )
    );
    assert_eq!(
        within("/old", fetch::MAX_HOPS).map(|(r, _)| r.status),
        Err(Error::Http("more than five redirects".into()))
    );
    assert_eq!(
        within("/old", fetch::MAX_HOPS + 1).map(|(r, _)| r.status),
        Err(Error::Http("more than five redirects".into()))
    );
}
