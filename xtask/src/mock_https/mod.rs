//! xtask's HTTPS mock (plan p5-networking, task 7): the hosts P5 reaches,
//! served over TLS 1.3 from memory with a test CA, and the faults P5's cases
//! need. QEMU's user networking maps the guest's 10.0.2.2 to the host's
//! 127.0.0.1, so a guest reaches it there; the client sends the host's name
//! by SNI and `Host:`, and the mock answers as that host.
//!
//! - **Hosts.** [`REPO`] is the update repository's Pages host
//!   (`kernel-ai-updates`' `metadata/` and `targets/`), [`RELEASES`] is
//!   GitHub's releases host, which redirects each download to [`ASSETS`], the
//!   release-assets host; [`MODELS`] is shaped like Hugging Face
//!   (`/<org>/<repo>/resolve/<rev>/<file>`). [`OFF`] is a host no handle
//!   names, where [`Fault::RedirectOff`] sends a client.
//! - **TLS.** One test CA ([`Mock::ca`]) signs a certificate for each host,
//!   chosen by SNI. The server is `kainet`'s, on our own provider.
//! - **HTTP.** GET and HEAD, one request a connection, with a single
//!   `Range: bytes=<first>-[<last>]` answered 206; 302 for a redirect; 404,
//!   400, 416, 421 (a `Host:` that is not the SNI) and 431 (a head over
//!   [`MAX_HEAD`]).
//! - **Faults** ([`Fault`]), set per host and path: a cut at N bytes of the
//!   body, an expired, wrong-name or unknown-CA certificate, a flipped byte,
//!   429, a slow link, and a redirect off the handle's hosts.
//! - **Bounds.** At most [`MAX_CONNECTIONS`] connections at once; each must
//!   send its request within [`REQUEST_LIMIT`], and no read or write of it may
//!   wait longer than that.
//!
//! The hosts' names are not in `netcore::fixture`'s DNS zone yet: lane N-k
//! adds them there with the first case that resolves them (P5 tasks 1 and 2),
//! and until then a guest reaches the mock at 10.0.2.2 ([`Mock::cmdline`]).

#[cfg(test)]
mod download_tests;
#[cfg(test)]
mod fetch_tests;
mod http;
mod pki;
#[cfg(test)]
mod tests;

use kainet::tls::{Session, Transport};
use rustls::server::UnbufferedServerConnection;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// The update repository's Pages host.
pub const REPO: &str = "updates.kai.test";
/// GitHub's releases host: every download redirects to [`ASSETS`].
pub const RELEASES: &str = "releases.kai.test";
/// GitHub's release-assets host.
pub const ASSETS: &str = "assets.kai.test";
/// A Hugging Face-shaped model host.
pub const MODELS: &str = "models.kai.test";
/// A host no handle names.
pub const OFF: &str = "elsewhere.kai.test";
/// Every host the mock answers for.
pub const HOSTS: &[&str] = &[REPO, RELEASES, ASSETS, MODELS, OFF];

/// The longest request head read.
pub const MAX_HEAD: usize = 8 * 1024;
/// How long a connection may take to send its request, and the longest any
/// one read or write of it may wait.
pub const REQUEST_LIMIT: Duration = Duration::from_secs(30);
/// The most connections served at once; more are closed at once.
pub const MAX_CONNECTIONS: usize = 64;
/// Bytes written at a time on a slow link.
const SLOW_CHUNK: usize = 1024;

/// What goes wrong for a host, or for one path on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// The connection is dropped after `n` bytes of the body, with no
    /// close_notify.
    CutAt(usize),
    /// The host's certificate has expired.
    Expired,
    /// The host's certificate is for another name.
    WrongName,
    /// The host's certificate comes from a CA the client does not trust.
    UnknownCa,
    /// Byte `n` of the file (not of the range) is flipped.
    Flip(usize),
    /// 429 Too Many Requests, with `Retry-After: n`.
    TooMany(u32),
    /// The body goes out at about `n` bytes a second.
    Slow(usize),
    /// 302 to the same path on [`OFF`].
    RedirectOff,
    /// The first response whose range holds the file's byte `n` is cut just
    /// before it, with no close_notify; then the fault is gone. So a
    /// download in ranges is cut once, at that point of the file.
    CutOnce(usize),
}

impl Fault {
    fn is_certificate(&self) -> bool {
        matches!(self, Fault::Expired | Fault::WrongName | Fault::UnknownCa)
    }
}

/// A request the mock answered, for a case to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    pub host: String,
    pub method: String,
    pub path: String,
    /// The `[start, end)` bytes sent, for a file.
    pub range: Option<(usize, usize)>,
    pub status: u16,
}

/// What the mock serves: files by host and path, and redirects.
#[derive(Default, Clone)]
pub struct Content {
    files: BTreeMap<(String, String), Arc<Vec<u8>>>,
    redirects: BTreeMap<(String, String), String>,
}

impl Content {
    pub fn new() -> Self {
        Self::default()
    }

    /// `bytes` at `path` on `host`.
    pub fn file(mut self, host: &str, path: &str, bytes: Vec<u8>) -> Self {
        self.files
            .insert((host.into(), path.into()), Arc::new(bytes));
        self
    }

    /// `path` on `host` answers 302 to `location`, a whole URL.
    pub fn redirect(mut self, host: &str, path: &str, location: &str) -> Self {
        self.redirects
            .insert((host.into(), path.into()), location.into());
        self
    }

    /// A GitHub release asset: `name` of release `tag`, downloaded from
    /// [`RELEASES`] and served by [`ASSETS`], as GitHub does.
    pub fn release_asset(self, tag: &str, name: &str, bytes: Vec<u8>) -> Self {
        let asset = format!("/assets/{tag}/{name}");
        let download = format!("/dane1599d/kernel-ai-updates/releases/download/{tag}/{name}");
        self.file(ASSETS, &asset, bytes).redirect(
            RELEASES,
            &download,
            &format!("https://{ASSETS}{asset}"),
        )
    }

    /// A model file on [`MODELS`], at Hugging Face's `resolve` path.
    pub fn model(self, repo: &str, revision: &str, file: &str, bytes: Vec<u8>) -> Self {
        self.file(MODELS, &format!("/{repo}/resolve/{revision}/{file}"), bytes)
    }

    /// The update repository's files on [`REPO`], each at its path.
    pub fn repository(mut self, files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        for (path, bytes) in files {
            self = self.file(REPO, &format!("/{path}"), bytes);
        }
        self
    }
}

/// The faults set, by host and then path; "*" is every path.
type Faults = Arc<Mutex<BTreeMap<(String, String), Fault>>>;

/// A running mock.
pub struct Mock {
    pub port: u16,
    ca: Vec<u8>,
    faults: Faults,
    served: Arc<Mutex<Vec<Served>>>,
}

impl Mock {
    /// The test CA's certificate in DER: the root a client trusts.
    pub fn ca(&self) -> &[u8] {
        &self.ca
    }

    /// The kernel command-line word that points the guest here.
    pub fn cmdline(&self) -> String {
        format!("https=10.0.2.2:{}", self.port)
    }

    /// From now on, `path` on `host` ("*" for all of them) goes wrong as
    /// `fault` says. A certificate fault applies to the whole host.
    pub fn set_fault(&self, host: &str, path: &str, fault: Fault) {
        let path = if fault.is_certificate() { "*" } else { path };
        lock(&self.faults).insert((host.into(), path.into()), fault);
    }

    /// Takes every fault away.
    pub fn clear_faults(&self) {
        lock(&self.faults).clear();
    }

    /// The requests answered so far, in order.
    pub fn served(&self) -> Vec<Served> {
        lock(&self.served).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The fault for `path` on `host`: the path's own, else the host's.
fn fault_for(faults: &Faults, host: &str, path: &str) -> Option<Fault> {
    let faults = lock(faults);
    faults
        .get(&(host.into(), path.into()))
        .or_else(|| faults.get(&(host.into(), "*".into())))
        .cloned()
}

/// Starts the mock on a free local port. It runs until xtask exits.
pub fn start(content: Content) -> Result<Mock, String> {
    let pki = Arc::new(pki::Pki::new()?);
    let faults: Faults = Arc::default();
    let served = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot start the HTTPS mock: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("the HTTPS mock has no address: {e}"))?
        .port();
    let content = Arc::new(content);
    let (accept_faults, accept_served) = (Arc::clone(&faults), Arc::clone(&served));
    let accept_pki = Arc::clone(&pki);
    let open = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                open.fetch_sub(1, Ordering::SeqCst);
                drop(stream);
                continue;
            }
            let (pki, content) = (Arc::clone(&accept_pki), Arc::clone(&content));
            let (faults, served) = (Arc::clone(&accept_faults), Arc::clone(&accept_served));
            let open = Arc::clone(&open);
            thread::spawn(move || {
                let _closes = Closes(open);
                let _ = stream.set_read_timeout(Some(REQUEST_LIMIT));
                let _ = stream.set_write_timeout(Some(REQUEST_LIMIT));
                let sni = pki::Sni::default();
                let Ok(config) = pki.server_config(Arc::clone(&faults), Arc::clone(&sni)) else {
                    return;
                };
                let Ok(conn) = UnbufferedServerConnection::new(config) else {
                    return;
                };
                // A refused handshake is what a certificate fault is for.
                let mut session = Session::new(conn, Tcp(stream));
                if session.handshake().is_ok() {
                    let host = lock(&sni).unwrap_or_default();
                    serve(&mut session, host, &content, &faults, &served);
                }
            });
        }
    });
    Ok(Mock {
        port,
        ca: pki.ca_der().to_vec(),
        faults,
        served,
    })
}

/// Counts a connection closed when its thread ends, however it ends.
struct Closes(Arc<AtomicUsize>);

impl Drop for Closes {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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

type Conn = Session<UnbufferedServerConnection, Tcp>;

/// Reads the request head: up to the blank line, at most [`MAX_HEAD`].
fn read_head(session: &mut Conn) -> Result<Vec<u8>, u16> {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        if head.len() > MAX_HEAD {
            return Err(431);
        }
        let n = session.read(&mut buf).map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400);
        }
        head.extend_from_slice(&buf[..n]);
    }
    if head.len() > MAX_HEAD {
        return Err(431);
    }
    Ok(head)
}

fn serve(
    session: &mut Conn,
    sni: &str,
    content: &Content,
    faults: &Faults,
    served: &Mutex<Vec<Served>>,
) {
    let request = match read_head(session).and_then(|head| http::parse(&head)) {
        Ok(request) => request,
        Err(status) => {
            let _ = respond(session, status, &[], b"");
            return;
        }
    };
    let mut logged = Served {
        host: sni.to_string(),
        method: request.method.clone(),
        path: request.path.clone(),
        range: None,
        status: 0,
    };
    let answer = answer(&request, sni, content, faults);
    logged.status = answer.status;
    logged.range = answer.range;
    lock(served).push(logged);
    let _ = send(session, &request, answer);
}

/// What a request gets, before it is sent.
struct Answer {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    range: Option<(usize, usize)>,
    /// Where the body's first byte sits in the file, for [`Fault::Flip`].
    offset: usize,
    fault: Option<Fault>,
}

impl Answer {
    fn plain(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            range: None,
            offset: 0,
            fault: None,
        }
    }
}

fn answer(request: &http::Request, sni: &str, content: &Content, faults: &Faults) -> Answer {
    if request.host != sni {
        return Answer::plain(421);
    }
    let key = (sni.to_string(), request.path.clone());
    let fault = fault_for(faults, sni, &request.path);
    match &fault {
        Some(Fault::TooMany(seconds)) => {
            let mut answer = Answer::plain(429);
            answer.headers.push(("Retry-After", seconds.to_string()));
            return answer;
        }
        Some(Fault::RedirectOff) => {
            let mut answer = Answer::plain(302);
            let location = format!("https://{OFF}{}", request.path);
            answer.headers.push(("Location", location));
            return answer;
        }
        _ => {}
    }
    if let Some(location) = content.redirects.get(&key) {
        let mut answer = Answer::plain(302);
        answer.headers.push(("Location", location.clone()));
        return answer;
    }
    let Some(file) = content.files.get(&key) else {
        return Answer::plain(404);
    };
    let (start, end, status) = match request.range {
        None => (0, file.len(), 200),
        Some(range) => match http::resolve(range, file.len()) {
            Some((start, end)) => (start, end, 206),
            None => {
                let mut answer = Answer::plain(416);
                answer
                    .headers
                    .push(("Content-Range", format!("bytes */{}", file.len())));
                return answer;
            }
        },
    };
    let mut answer = Answer::plain(status);
    if status == 206 {
        answer.headers.push((
            "Content-Range",
            format!("bytes {start}-{}/{}", end - 1, file.len()),
        ));
    }
    answer.headers.push(("Accept-Ranges", "bytes".into()));
    answer.body = file[start..end].to_vec();
    answer.range = Some((start, end));
    answer.offset = start;
    answer.fault = match fault {
        Some(Fault::CutOnce(at)) if (start..end).contains(&at) => {
            lock(faults).remove(&key);
            Some(Fault::CutAt(at - start))
        }
        Some(Fault::CutOnce(_)) => None,
        other => other,
    };
    answer
}

fn send(session: &mut Conn, request: &http::Request, answer: Answer) -> Result<(), ()> {
    let Answer {
        status,
        headers,
        mut body,
        offset,
        fault,
        ..
    } = answer;
    if let Some(Fault::Flip(at)) = fault
        && let Some(byte) = at.checked_sub(offset).and_then(|i| body.get_mut(i))
    {
        *byte ^= 1;
    }
    let length = body.len();
    let body = if request.method == "HEAD" {
        Vec::new()
    } else {
        body
    };
    match fault {
        Some(Fault::CutAt(n)) => {
            let head = http::head(status, &headers, length);
            session.write_all(&head).map_err(|_| ())?;
            let n = n.min(body.len());
            session.write_all(&body[..n]).map_err(|_| ())?;
            // Dropped with no close_notify: the client must see a cut.
            Ok(())
        }
        Some(Fault::Slow(rate)) => {
            session
                .write_all(&http::head(status, &headers, length))
                .map_err(|_| ())?;
            let pause = Duration::from_secs_f64(SLOW_CHUNK as f64 / rate.max(1) as f64);
            for chunk in body.chunks(SLOW_CHUNK) {
                session.write_all(chunk).map_err(|_| ())?;
                thread::sleep(pause);
            }
            session.close().map_err(|_| ())
        }
        _ => respond_with(session, status, &headers, &body, length),
    }
}

fn respond(
    session: &mut Conn,
    status: u16,
    headers: &[(&'static str, String)],
    body: &[u8],
) -> Result<(), ()> {
    respond_with(session, status, headers, body, body.len())
}

fn respond_with(
    session: &mut Conn,
    status: u16,
    headers: &[(&'static str, String)],
    body: &[u8],
    length: usize,
) -> Result<(), ()> {
    let mut out = http::head(status, headers, length);
    out.extend_from_slice(body);
    session.write_all(&out).map_err(|_| ())?;
    session.close().map_err(|_| ())
}
