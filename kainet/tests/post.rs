//! A POST is made once (`Fetch::post`, `fetch::post_within`): its request is
//! exact, a redirect (303 included) is never followed, nothing is tried again,
//! a failure after a byte may have gone out and before a status line came is a
//! cut the server may have, and once a status line came the answer is it.

use kainet::fetch::{self, Error};
use kainet::http::{Connect, Fetch, HttpError, Limits, Method, Stream, Url};
use std::cell::{Cell, RefCell};
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

/// The status a POST to `url` came to, and its reason.
fn post(server: &mut Server, url: &str, body: &[u8]) -> Result<(u16, Option<String>), HttpError> {
    fetch(&anyone)
        .post(server, &Url::parse(url).unwrap(), body)
        .map(|f| (f.head.status, f.head.reason))
}

const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nreceived";

#[test]
fn a_post_sends_its_head_and_body_exactly_once() {
    let mut server = Server::new(OK, Fails::Never);
    let got = fetch(&anyone)
        .post(
            &mut server,
            &Url::parse("https://a.test/hook?x=1").unwrap(),
            "héllo".as_bytes(),
        )
        .unwrap();
    assert_eq!(
        (got.head.status, got.head.reason.as_deref()),
        (200, Some("OK"))
    );
    assert_eq!(got.received, 0, "a POST's answer body is never read");
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
        let got = fetch(&anyone).post(&mut server, &url, b"hi").unwrap();
        assert_eq!(got.head.status, status);
        assert_eq!((got.redirects, got.received), (0, 0));
        assert_eq!(
            got.unfollowed.map(|u| (u.host, u.path)),
            Some(("a.test".into(), "/elsewhere".into()))
        );
        assert_eq!(server.connections(), 1, "{status} was followed");
    }
}

#[test]
fn another_status_comes_back_with_its_head() {
    let reply = b"HTTP/1.1 500 Oops\r\nContent-Length: 4\r\n\r\nnope";
    let mut server = Server::new(reply, Fails::Never);
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hi"),
        Ok((500, Some("Oops".into())))
    );
    assert_eq!(server.connections(), 1);
}

#[test]
fn a_cut_before_a_status_line_is_a_cut_and_never_tried_again() {
    // No answer at all, then a cut.
    let mut server = Server::new(b"", Fails::Cut);
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hello"),
        Err(HttpError::Cut)
    );
    assert_eq!(server.connections(), 1);
    assert!(server.sent()[0].ends_with(b"\r\n\r\nhello"));
    // Half a status line, then a cut.
    let mut server = Server::new(b"HTTP/1.1 20", Fails::Cut);
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hello"),
        Err(HttpError::Cut)
    );
    // Something that is not HTTP.
    let mut server = Server::new(b"SMTP ready\r\n\r\n", Fails::Never);
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hello"),
        Err(HttpError::Cut)
    );
    assert_eq!(server.connections(), 1);
}

#[test]
fn once_a_status_line_came_the_answer_is_that_status() {
    for (reply, fails, want) in [
        // The answer's body cut short: the body is never read.
        (
            &b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nrec"[..],
            Fails::Cut,
            200,
        ),
        // A webhook that answers with close framing and never a clean end.
        (
            b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nthanks",
            Fails::Cut,
            200,
        ),
        // Cut inside the head, after the status line.
        (b"HTTP/1.1 201 Created\r\nContent-Ty", Fails::Cut, 201),
        // A head that does not read: two lengths.
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
            Fails::Never,
            200,
        ),
        // Interim 100 Continue, then the final answer.
        (
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 204 No Content\r\n\r\n",
            Fails::Cut,
            204,
        ),
        // A blank line before the status line, and lines ending in a bare LF.
        (b"\r\nHTTP/1.1 202 Accepted\nX-A: b\n", Fails::Cut, 202),
        (b"\n\nHTTP/1.1 200\r\n", Fails::Cut, 200),
    ] {
        let mut server = Server::new(reply, fails);
        let got = post(&mut server, "https://a.test/hook", b"hi");
        assert_eq!(got.map(|(status, _)| status), Ok(want), "{reply:?}");
        assert_eq!(server.connections(), 1);
    }
    // A head over the limit still gives its status.
    let huge = format!(
        "HTTP/1.1 200 OK\r\nX-Pad: {}\r\n\r\n",
        "a".repeat(20 * 1024)
    );
    let mut server = Server::new(huge.as_bytes(), Fails::Never);
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hi"),
        Ok((200, Some("OK".into())))
    );
}

#[test]
fn an_interim_answer_is_no_answer() {
    // A 1xx says the server is reading, not that it took the request: a cut
    // after one is a cut, however many came.
    for reply in [
        &b"HTTP/1.1 100 Continue\r\n\r\n"[..],
        b"HTTP/1.1 103 Early Hints\r\nLink: </a>\r\n\r\n",
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 20",
        b"HTTP/1.1 100 Continue\r\nX-A: b",
    ] {
        let mut server = Server::new(reply, Fails::Cut);
        assert_eq!(
            post(&mut server, "https://a.test/hook", b"hi"),
            Err(HttpError::Cut),
            "{reply:?}"
        );
        assert_eq!(server.connections(), 1);
    }
    // A 101 is no answer to a POST.
    let mut server = Server::new(
        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: x\r\n\r\n",
        Fails::Never,
    );
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hi"),
        Err(HttpError::Cut)
    );
    // Any number of 1xx before the final answer are skipped.
    let reply = format!(
        "{}HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 100 Continue\r\n\r\n".repeat(5)
    );
    let mut server = Server::new(reply.as_bytes(), Fails::Never);
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hi"),
        Ok((200, Some("OK".into())))
    );
    // 1xx heads past the head limit in all are a cut, never a 1xx answer.
    let endless = "HTTP/1.1 100 Continue\r\n\r\n".repeat(1000);
    let mut server = Server::new(endless.as_bytes(), Fails::Never);
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hi"),
        Err(HttpError::Cut)
    );
}

#[test]
fn a_cut_mid_body_is_a_cut_and_never_tried_again() {
    for after in [0, 10, 150] {
        let mut server = Server::new(OK, Fails::SendAfter(after));
        assert_eq!(
            post(&mut server, "https://a.test/hook", &[b'x'; 400]),
            Err(HttpError::Cut),
            "cut after {after} bytes"
        );
        assert_eq!(server.connections(), 1);
        assert_eq!(server.sent()[0].len(), after);
    }
}

#[test]
fn a_post_not_sent_says_so() {
    // No connection: nothing went out, so it is not a cut.
    let mut server = Server::new(OK, Fails::Never);
    server.down = true;
    assert_eq!(
        post(&mut server, "https://a.test/hook", b"hi"),
        Err(HttpError::Connect("no route to the host"))
    );
    // A host the caller may not reach is never connected to.
    let mut server = Server::new(OK, Fails::Never);
    let url = Url::parse("https://b.test/hook").unwrap();
    let refused = fetch(&only_a).post(&mut server, &url, b"hi");
    assert_eq!(refused.unwrap_err(), HttpError::Refused("b.test".into()));
    assert_eq!(server.connections(), 0);
    // Past the deadline before the connection, nothing is sent.
    let url = Url::parse("https://a.test/hook").unwrap();
    let late = Fetch {
        limits: Limits::new(1 << 16, 0),
        allow: &anyone,
        clock: &|| 1,
    };
    assert_eq!(late.post(&mut server, &url, b"hi"), Err(HttpError::Timeout));
    assert_eq!(server.connections(), 0);
    // Past it once the connection opened (a slow TLS handshake): still
    // nothing written.
    let ticks = Cell::new(0u64);
    let clock = || {
        ticks.set(ticks.get() + 1);
        ticks.get()
    };
    let slow = Fetch {
        limits: Limits::new(1 << 16, 1),
        allow: &anyone,
        clock: &clock,
    };
    assert_eq!(slow.post(&mut server, &url, b"hi"), Err(HttpError::Timeout));
    assert_eq!(server.connections(), 1);
    assert!(server.sent()[0].is_empty(), "nothing was written");
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

fn post_within(
    url: &str,
    allow: &dyn Fn(&str, u16) -> bool,
    server: &mut Server,
) -> Result<fetch::Served, Error> {
    fetch::post_within(url, b"hi", allow, server, &zero, u64::MAX)
}

#[test]
fn the_fetch_service_posts_once_and_names_a_cut() {
    let mut server = Server::new(
        b"HTTP/1.1 303 See Other\r\nLocation: https://b.test/done\r\nContent-Length: 0\r\n\r\n",
        Fails::Never,
    );
    let served = post_within("https://a.test/hook", &only_a, &mut server).unwrap();
    assert_eq!(served.response.status, 303);
    assert_eq!(served.response.reason.as_deref(), Some("See Other"));
    assert!(served.response.body.is_empty());
    assert_eq!(served.location.as_deref(), Some("https://b.test/done"));
    assert_eq!((served.hops, server.connections()), (0, 1));
    let mut cut = Server::new(b"", Fails::Cut);
    assert_eq!(
        post_within("https://a.test/hook", &anyone, &mut cut),
        Err(Error::Cut)
    );
    assert_eq!(cut.connections(), 1);
    assert_eq!(
        post_within("https://b.test/hook", &only_a, &mut cut),
        Err(Error::Refused("b.test".into()))
    );
    assert_eq!(cut.connections(), 1);
}

#[test]
fn a_long_location_is_dropped_not_an_error() {
    let reply = format!(
        "HTTP/1.1 302 Found\r\nLocation: https://b.test/{}\r\n\r\n",
        "x".repeat(4096)
    );
    let mut server = Server::new(reply.as_bytes(), Fails::Never);
    let served = post_within("https://a.test/hook", &anyone, &mut server).unwrap();
    assert_eq!(
        (served.response.status, served.location.as_deref()),
        (302, None)
    );
    let head = kainet::wire::Head::of(&served);
    let answer = kainet::wire::answer(&Ok(head));
    assert_ne!(answer, kainet::wire::TOO_LONG);
    assert_eq!(
        kainet::wire::PostOutcome::read(&answer),
        kainet::wire::PostOutcome::Sent {
            status: 302,
            line: "302 Found".into()
        }
    );
}
