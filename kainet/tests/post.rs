//! A POST is made once (`Fetch::post`, `fetch::post_within`): its request is
//! exact, a redirect (303 included) is never followed, nothing is tried again,
//! and a failure after a byte may have gone out is a cut the server may have.

use kainet::fetch::{self, Error};
use kainet::http::{Connect, Fetch, HttpError, Limits, Method, Stream, Url};
use std::cell::RefCell;
use std::rc::Rc;

/// What the scripted server saw: one entry a connection, with what it was sent.
type Seen = Rc<RefCell<Vec<Vec<u8>>>>;

/// How a connection behaves.
#[derive(Clone, Copy)]
enum Fails {
    /// It takes the request and gives the reply, then ends cleanly.
    Never,
    /// The reply ends with a cut, no clean end.
    Cut,
    /// A send fails after this many bytes went out.
    SendAfter(usize),
}

struct Scripted {
    reply: Vec<u8>,
    at: usize,
    fails: Fails,
    seen: Seen,
}

impl Stream for Scripted {
    fn send(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        let mut seen = self.seen.borrow_mut();
        let sent = seen.last_mut().expect("a connection was opened");
        if let Fails::SendAfter(n) = self.fails {
            let room = n.saturating_sub(sent.len()).min(bytes.len());
            sent.extend_from_slice(&bytes[..room]);
            if room < bytes.len() {
                return Err(HttpError::Truncated);
            }
            return Ok(());
        }
        sent.extend_from_slice(bytes);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        if self.at == self.reply.len() {
            return match self.fails {
                Fails::Cut => Err(HttpError::Truncated),
                _ => Ok(0),
            };
        }
        let n = buf.len().min(self.reply.len() - self.at);
        buf[..n].copy_from_slice(&self.reply[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// One host's server: every connection gets `reply`.
struct Server {
    reply: Vec<u8>,
    fails: Fails,
    /// No connection opens: the host cannot be reached.
    down: bool,
    seen: Seen,
}

impl Server {
    fn new(reply: &[u8], fails: Fails) -> Self {
        Self {
            reply: reply.to_vec(),
            fails,
            down: false,
            seen: Rc::default(),
        }
    }

    fn connections(&self) -> usize {
        self.seen.borrow().len()
    }

    fn sent(&self) -> Vec<Vec<u8>> {
        self.seen.borrow().clone()
    }
}

impl Connect for Server {
    type Stream = Scripted;

    fn connect(&mut self, _: &Url) -> Result<Scripted, HttpError> {
        if self.down {
            return Err(HttpError::Connect("no route to the host"));
        }
        self.seen.borrow_mut().push(Vec::new());
        Ok(Scripted {
            reply: self.reply.clone(),
            at: 0,
            fails: self.fails,
            seen: Rc::clone(&self.seen),
        })
    }
}

fn anyone(_: &str, _: u16) -> bool {
    true
}

fn only_a(host: &str, _: u16) -> bool {
    host == "a.test"
}

fn zero() -> u64 {
    0
}

fn fetch(allow: &dyn Fn(&str, u16) -> bool) -> Fetch<'_> {
    Fetch {
        limits: Limits::new(1 << 16, u64::MAX),
        allow,
        clock: &zero,
    }
}

fn post(server: &mut Server, url: &str, body: &[u8]) -> (Result<u16, HttpError>, Vec<u8>) {
    let mut got = Vec::new();
    let answer = fetch(&anyone).post(server, &Url::parse(url).unwrap(), body, &mut |d| {
        got.extend_from_slice(d);
        Ok(())
    });
    (answer.map(|f| f.head.status), got)
}

const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nreceived";

#[test]
fn a_post_sends_its_head_and_body_exactly_once() {
    let mut server = Server::new(OK, Fails::Never);
    let (status, body) = post(&mut server, "https://a.test/hook?x=1", "héllo".as_bytes());
    assert_eq!((status, body.as_slice()), (Ok(200), &b"received"[..]));
    assert_eq!(
        server.sent(),
        [b"POST /hook?x=1 HTTP/1.1\r\nHost: a.test\r\nUser-Agent: kai\r\nAccept-Encoding: identity\r\nConnection: close\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 6\r\n\r\nh\xc3\xa9llo".to_vec()]
    );
}

#[test]
fn a_post_never_follows_a_redirect_303_included() {
    for status in [301u16, 302, 303, 307, 308] {
        let reply = format!(
            "HTTP/1.1 {status} Moved\r\nLocation: /elsewhere\r\nContent-Length: 4\r\n\r\nmove"
        );
        let mut server = Server::new(reply.as_bytes(), Fails::Never);
        let url = Url::parse("https://a.test/hook").unwrap();
        let mut passed = Vec::new();
        let got = fetch(&anyone)
            .post(&mut server, &url, b"hi", &mut |d| {
                passed.extend_from_slice(d);
                Ok(())
            })
            .unwrap();
        assert_eq!(got.head.status, status);
        assert_eq!(got.redirects, 0);
        assert_eq!(
            got.unfollowed.map(|u| (u.host, u.path)),
            Some(("a.test".into(), "/elsewhere".into()))
        );
        assert!(passed.is_empty(), "a redirect's body is not read");
        assert_eq!(server.connections(), 1, "{status} was followed");
    }
}

#[test]
fn another_status_comes_back_with_its_head_and_no_body() {
    let reply = b"HTTP/1.1 500 Oops\r\nContent-Length: 4\r\n\r\nnope";
    let mut server = Server::new(reply, Fails::Never);
    let (status, body) = post(&mut server, "https://a.test/hook", b"hi");
    assert_eq!((status, body.len()), (Ok(500), 0));
    assert_eq!(server.connections(), 1);
}

#[test]
fn a_cut_after_the_body_went_out_is_a_cut_and_never_tried_again() {
    // No answer at all, then a cut.
    let mut server = Server::new(b"", Fails::Cut);
    let (status, _) = post(&mut server, "https://a.test/hook", b"hello");
    assert_eq!(status, Err(HttpError::Cut));
    assert_eq!(server.connections(), 1);
    assert!(server.sent()[0].ends_with(b"\r\n\r\nhello"));
    // A head, then the answer's body cut short: the server has it.
    let mut server = Server::new(
        b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nrec",
        Fails::Cut,
    );
    let (status, _) = post(&mut server, "https://a.test/hook", b"hello");
    assert_eq!(status, Err(HttpError::Cut));
    assert_eq!(server.connections(), 1);
    // A broken head after the request went out.
    let mut server = Server::new(b"SMTP ready\r\n\r\n", Fails::Never);
    let (status, _) = post(&mut server, "https://a.test/hook", b"hello");
    assert_eq!(status, Err(HttpError::Cut));
    assert_eq!(server.connections(), 1);
}

#[test]
fn a_cut_mid_body_is_a_cut_and_never_tried_again() {
    for after in [0, 10, 150] {
        let mut server = Server::new(OK, Fails::SendAfter(after));
        let (status, _) = post(&mut server, "https://a.test/hook", &[b'x'; 400]);
        assert_eq!(status, Err(HttpError::Cut), "cut after {after} bytes");
        assert_eq!(server.connections(), 1);
        assert_eq!(server.sent()[0].len(), after);
    }
}

#[test]
fn a_post_not_sent_says_so() {
    // No connection: nothing went out, so it is not a cut.
    let mut server = Server::new(OK, Fails::Never);
    server.down = true;
    let (status, _) = post(&mut server, "https://a.test/hook", b"hi");
    assert_eq!(status, Err(HttpError::Connect("no route to the host")));
    // A host the caller may not reach is never connected to.
    let mut server = Server::new(OK, Fails::Never);
    let url = Url::parse("https://b.test/hook").unwrap();
    let refused = fetch(&only_a).post(&mut server, &url, b"hi", &mut |_| Ok(()));
    assert_eq!(refused.unwrap_err(), HttpError::Refused("b.test".into()));
    assert_eq!(server.connections(), 0);
    // Past the deadline, nothing is sent.
    let late = Fetch {
        limits: Limits::new(1 << 16, 0),
        allow: &anyone,
        clock: &|| 1,
    };
    let url = Url::parse("https://a.test/hook").unwrap();
    assert_eq!(
        late.post(&mut server, &url, b"hi", &mut |_| Ok(())),
        Err(HttpError::Timeout)
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn a_post_is_never_walked_like_a_get() {
    let mut server = Server::new(OK, Fails::Never);
    let url = Url::parse("https://a.test/hook").unwrap();
    for walked in [
        fetch(&anyone).run(&mut server, Method::Post, &url, None, &mut |_| Ok(())),
        fetch(&anyone).run_within(&mut server, Method::Post, &url, None, &mut |_| Ok(())),
    ] {
        assert_eq!(walked, Err(HttpError::Url("a POST is made by Fetch::post")));
    }
    assert_eq!(server.connections(), 0);
}

#[test]
fn the_fetch_service_posts_once_and_names_a_cut() {
    let mut server = Server::new(
        b"HTTP/1.1 303 See Other\r\nLocation: https://b.test/done\r\nContent-Length: 0\r\n\r\n",
        Fails::Never,
    );
    let served = fetch::post_within(
        "https://a.test/hook",
        b"hi",
        &only_a,
        &mut server,
        &zero,
        u64::MAX,
    )
    .unwrap();
    assert_eq!(served.response.status, 303);
    assert_eq!(served.location.as_deref(), Some("https://b.test/done"));
    assert_eq!((served.hops, server.connections()), (0, 1));
    let mut cut = Server::new(b"", Fails::Cut);
    assert_eq!(
        fetch::post_within(
            "https://a.test/hook",
            b"hi",
            &anyone,
            &mut cut,
            &zero,
            u64::MAX
        ),
        Err(Error::Cut)
    );
    assert_eq!(cut.connections(), 1);
    assert_eq!(
        fetch::post_within(
            "https://b.test/hook",
            b"hi",
            &only_a,
            &mut cut,
            &zero,
            u64::MAX
        ),
        Err(Error::Refused("b.test".into()))
    );
    assert_eq!(cut.connections(), 1);
}
