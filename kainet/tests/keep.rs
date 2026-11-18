//! `Keep`: a download's GETs reuse one connection to one host while each
//! response is read whole, and never otherwise.

use kainet::http::{Connect, Fetch, HttpError, Keep, Limits, Method, Range, Stream, Url};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

/// What every connection was sent, by connection number.
type Sent = Rc<RefCell<Vec<(usize, String)>>>;

/// One connection: answers each request with the next of its replies, and
/// once they run out ends (`recv` gives 0), as a server closing an idle
/// connection does.
struct Conn {
    id: usize,
    replies: VecDeque<Vec<u8>>,
    out: Vec<u8>,
    at: usize,
    sent: Sent,
}

impl Stream for Conn {
    fn send(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        self.sent
            .borrow_mut()
            .push((self.id, String::from_utf8_lossy(bytes).into_owned()));
        self.out = self.replies.pop_front().unwrap_or_default();
        self.at = 0;
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        let n = buf.len().min(self.out.len() - self.at);
        buf[..n].copy_from_slice(&self.out[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Each new connection gets `per_connection` of the script's replies, in turn:
/// its host's own script, or the shared one for a host with none.
struct Server {
    script: VecDeque<Vec<u8>>,
    hosts: BTreeMap<String, VecDeque<Vec<u8>>>,
    per_connection: usize,
    connects: Vec<String>,
    sent: Sent,
}

impl Server {
    fn new(replies: &[&[u8]], per_connection: usize) -> Self {
        Self {
            script: replies.iter().map(|r| r.to_vec()).collect(),
            hosts: BTreeMap::new(),
            per_connection,
            connects: Vec::new(),
            sent: Sent::default(),
        }
    }

    fn with_host(mut self, host: &str, replies: &[&[u8]]) -> Self {
        self.hosts
            .insert(host.into(), replies.iter().map(|r| r.to_vec()).collect());
        self
    }

    fn requests(&self) -> Vec<(usize, String)> {
        self.sent.borrow().clone()
    }
}

impl Connect for Server {
    type Stream = Conn;

    fn connect(&mut self, url: &Url) -> Result<Conn, HttpError> {
        self.connects.push(url.host.clone());
        let script = self.hosts.get_mut(&url.host).unwrap_or(&mut self.script);
        let take = self.per_connection.min(script.len());
        Ok(Conn {
            id: self.connects.len(),
            replies: script.drain(..take).collect(),
            out: Vec::new(),
            at: 0,
            sent: Rc::clone(&self.sent),
        })
    }
}

const PART: &[u8] =
    b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 0-3/12\r\n\r\nabcd";

fn part(first: u64) -> Vec<u8> {
    format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes {first}-{}/12\r\n\r\nabcd",
        first + 3
    )
    .into_bytes()
}

fn anyone(_: &str, _: u16) -> bool {
    true
}

fn zero() -> u64 {
    0
}

/// GETs `range` of `url` through `connect`: the result and the body.
fn get(
    connect: &mut impl Connect,
    url: &str,
    range: Option<Range>,
) -> (Result<u16, HttpError>, Vec<u8>) {
    let fetch = Fetch {
        limits: Limits::new(1 << 20, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    let mut body = Vec::new();
    let got = fetch.run(
        connect,
        Method::Get,
        &Url::parse(url).unwrap(),
        range,
        &mut |bytes| {
            body.extend_from_slice(bytes);
            Ok(())
        },
    );
    (got.map(|got| got.head.status), body)
}

fn range(first: u64) -> Option<Range> {
    Some(Range {
        first,
        last: Some(first + 3),
    })
}

#[test]
fn a_downloads_chunks_to_one_host_share_one_connection() {
    let (a, b, c) = (part(0), part(4), part(8));
    let mut keep = Keep::new(Server::new(&[&a, &b, &c], 3));
    for first in [0, 4, 8] {
        let (got, body) = get(&mut keep, "https://assets.kai.test/f", range(first));
        assert_eq!(got, Ok(206));
        assert_eq!(body, b"abcd");
    }
    assert!(keep.holds());
    let server = keep.inner_mut();
    assert_eq!(server.connects, ["assets.kai.test"]);
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|(id, text)| *id == 1 && text.contains("Connection: keep-alive\r\n"))
    );
}

#[test]
fn without_keep_each_request_closes_its_connection() {
    let mut server = Server::new(&[PART, PART], 1);
    for _ in 0..2 {
        assert_eq!(
            get(&mut server, "https://assets.kai.test/f", range(0)).0,
            Ok(206)
        );
    }
    assert_eq!(server.connects.len(), 2);
    assert!(
        server
            .requests()
            .iter()
            .all(|(_, text)| text.contains("Connection: close\r\n"))
    );
}

#[test]
fn a_kept_connection_the_server_closed_is_asked_again_on_a_new_one() {
    // One response a connection: the kept one ends before the second's head.
    let (a, b) = (part(0), part(4));
    let mut keep = Keep::new(Server::new(&[&a, &b], 1));
    assert_eq!(
        get(&mut keep, "https://assets.kai.test/f", range(0)).0,
        Ok(206)
    );
    let (got, body) = get(&mut keep, "https://assets.kai.test/f", range(4));
    assert_eq!(got, Ok(206));
    assert_eq!(body, b"abcd", "nothing passed on twice");
    let server = keep.inner_mut();
    assert_eq!(server.connects.len(), 2);
    let ids: Vec<usize> = server.requests().iter().map(|(id, _)| *id).collect();
    assert_eq!(
        ids,
        [1, 1, 2],
        "asked on the kept one, then once on a new one"
    );
}

#[test]
fn a_server_that_says_close_is_not_kept() {
    let closing = b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nConnection: close\r\nContent-Range: bytes 0-3/12\r\n\r\nabcd";
    let mut keep = Keep::new(Server::new(&[closing, PART], 1));
    assert_eq!(
        get(&mut keep, "https://assets.kai.test/f", range(0)).0,
        Ok(206)
    );
    assert!(!keep.holds());
    assert_eq!(
        get(&mut keep, "https://assets.kai.test/f", range(0)).0,
        Ok(206)
    );
    assert_eq!(keep.inner_mut().connects.len(), 2);
}

#[test]
fn a_body_until_close_or_a_long_one_not_passed_on_is_not_kept() {
    let until_close = b"HTTP/1.1 200 OK\r\n\r\nabcd";
    let missing = b"HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\n\r\nno!";
    let mut keep = Keep::new(Server::new(&[until_close], 2));
    assert_eq!(get(&mut keep, "https://assets.kai.test/f", None).0, Ok(200));
    assert!(!keep.holds(), "a body until close leaves nothing to reuse");
    // A short body not passed on is read through, and the connection kept.
    let mut keep = Keep::new(Server::new(&[missing], 2));
    let (got, body) = get(&mut keep, "https://assets.kai.test/f", range(0));
    assert_eq!((got, body), (Ok(404), Vec::new()));
    assert!(keep.holds());
    // A long one is not read through.
    let long = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Length: 5000\r\n\r\n{}",
        "x".repeat(5000)
    );
    let mut keep = Keep::new(Server::new(&[long.as_bytes()], 2));
    assert_eq!(
        get(&mut keep, "https://assets.kai.test/f", range(0)).0,
        Ok(404)
    );
    assert!(!keep.holds());
}

#[test]
fn a_cut_drops_the_connection() {
    let short = b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 0-3/12\r\n\r\nab";
    let mut keep = Keep::new(Server::new(&[short, PART], 1));
    assert_eq!(
        get(&mut keep, "https://assets.kai.test/f", range(0)).0,
        Err(HttpError::Truncated)
    );
    assert!(!keep.holds());
    assert_eq!(
        get(&mut keep, "https://assets.kai.test/f", range(0)).0,
        Ok(206)
    );
    assert_eq!(keep.inner_mut().connects.len(), 2);
}

#[test]
fn another_host_or_port_gets_its_own_connection() {
    let mut keep = Keep::new(Server::new(&[PART, PART, PART], 1));
    assert_eq!(
        get(&mut keep, "https://assets.kai.test/f", range(0)).0,
        Ok(206)
    );
    assert_eq!(
        get(&mut keep, "https://other.kai.test/f", range(0)).0,
        Ok(206)
    );
    assert_eq!(
        get(&mut keep, "https://other.kai.test:8443/f", range(0)).0,
        Ok(206)
    );
    let ids: Vec<usize> = keep
        .inner_mut()
        .requests()
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(ids, [1, 2, 3]);
}

#[test]
fn a_post_never_takes_a_kept_connection() {
    let answer = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
    let mut keep = Keep::new(Server::new(&[PART, answer], 1));
    assert_eq!(
        get(&mut keep, "https://assets.kai.test/f", range(0)).0,
        Ok(206)
    );
    assert!(keep.holds());
    let fetch = Fetch {
        limits: Limits::new(1 << 20, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    let url = Url::parse("https://assets.kai.test/post").unwrap();
    assert_eq!(fetch.post(&mut keep, &url, b"x").unwrap().head.status, 200);
    assert!(!keep.holds());
    let requests = keep.inner_mut().requests();
    assert_eq!(requests[1].0, 2, "the POST went on a new connection");
    assert!(requests[1].1.starts_with("POST ") && requests[1].1.contains("Connection: close\r\n"));
}

#[test]
fn a_redirected_downloads_two_hosts_each_keep_their_connection() {
    let redirect: &[u8] =
        b"HTTP/1.1 302 Found\r\nLocation: https://assets.kai.test/f\r\nContent-Length: 5\r\n\r\nmoved";
    let (a, b, c) = (part(0), part(4), part(8));
    let server = Server::new(&[], 3)
        .with_host("releases.kai.test", &[redirect, redirect, redirect])
        .with_host("assets.kai.test", &[&a, &b, &c]);
    let mut keep = Keep::new(server);
    for first in [0, 4, 8] {
        let (got, body) = get(&mut keep, "https://releases.kai.test/f", range(first));
        assert_eq!(got, Ok(206));
        assert_eq!(body, b"abcd");
    }
    assert_eq!(keep.held(), 2);
    assert_eq!(
        keep.inner_mut().connects,
        ["releases.kai.test", "assets.kai.test"],
        "one connection a host for three chunks"
    );
}

#[test]
fn a_redirect_with_a_long_body_is_not_kept() {
    let long = format!(
        "HTTP/1.1 302 Found\r\nLocation: https://assets.kai.test/f\r\nContent-Length: 5000\r\n\r\n{}",
        "x".repeat(5000)
    );
    let server = Server::new(&[], 3)
        .with_host("releases.kai.test", &[long.as_bytes()])
        .with_host("assets.kai.test", &[PART]);
    let mut keep = Keep::new(server);
    assert_eq!(
        get(&mut keep, "https://releases.kai.test/f", range(0)).0,
        Ok(206)
    );
    assert_eq!(keep.held(), 1, "only the assets host's");
}
