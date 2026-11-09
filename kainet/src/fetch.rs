//! The `fetch` service's logic (plan p5-networking, task 12, host half):
//! an app's `kai:net/http` request (`kainet/wit/kai-net/http.wit`), made on
//! the app's own grant. The OS wiring (the service, the app's `NetDest`
//! handle passed with each call, the grant's ask) waits for tasks 1, 2 and
//! 13; this is what it will run.
//!
//! - **The grant** ([`Grant`]) is the app's host suffixes. A URL's host, and
//!   every redirect's, must be one of them or under one at a label boundary,
//!   on port 443, before it is reached; the kernel enforces the same through
//!   the app's `NetDest` (task 1). TLS then checks the server's certificate
//!   for that host, so the certificate's name is under the grant too.
//! - **A grant may not name a shared suffix**: a top-level domain, or one of
//!   a short list of our own ([`SHARED`]) such as `co.uk` or `github.io`,
//!   under which unrelated parties hold names (P5 question 3: no Public
//!   Suffix List in v0.2).
//! - **A response** is its status, final URL, `Content-Type` and
//!   `Retry-After` if the server gave them, and body, at most [`MAX_BODY`].
//!   A status other than 2xx is a response too, not an error. Errors keep
//!   their names for the app, each with at most [`MAX_WHY`] printable bytes
//!   of why.
//! - **No request body or headers yet**: GET and HEAD, with a byte range.
//! - **A call only queues** ([`Queue`]), as `kai:models` does: at most
//!   [`MAX_WAITING`] requests wait, and each answer comes in a later call.

use crate::http::{Connect, Fetch, HttpError, Limits, Method, Range, Url};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The longest body an app is given.
pub const MAX_BODY: usize = 64 * 1024;
/// The most bytes of why an error gives the app.
pub const MAX_WHY: usize = 200;
/// The most requests one app instance may have waiting.
pub const MAX_WAITING: usize = 4;
/// The most suffixes one grant may hold.
pub const MAX_SUFFIXES: usize = 16;
/// The port apps reach.
pub const PORT: u16 = 443;

/// Suffixes under which unrelated parties hold names, which no grant may
/// name: second-level public suffixes and shared hosting domains. Every
/// top-level domain is refused besides.
pub const SHARED: &[&str] = &[
    "co.uk",
    "org.uk",
    "ac.uk",
    "gov.uk",
    "com.au",
    "net.au",
    "co.nz",
    "co.jp",
    "com.br",
    "co.in",
    "github.io",
    "githubusercontent.com",
    "gitlab.io",
    "pages.dev",
    "workers.dev",
    "netlify.app",
    "vercel.app",
    "web.app",
    "firebaseapp.com",
    "herokuapp.com",
    "appspot.com",
    "blogspot.com",
    "cloudfront.net",
    "azurewebsites.net",
    "s3.amazonaws.com",
    "fly.dev",
    // Parents under which every customer's names sit (C's review of #671).
    "amazonaws.com",
    "googleusercontent.com",
];

/// Why a grant was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantError {
    /// Not a DNS name.
    NotAName(String),
    /// A top-level domain or a shared suffix.
    Shared(String),
    /// More than [`MAX_SUFFIXES`].
    TooMany,
}

/// The hosts one app may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    suffixes: Vec<String>,
}

impl Grant {
    /// A grant for `suffixes`, each lower-cased.
    pub fn new(suffixes: &[&str]) -> Result<Self, GrantError> {
        if suffixes.len() > MAX_SUFFIXES {
            return Err(GrantError::TooMany);
        }
        let mut kept = Vec::with_capacity(suffixes.len());
        for suffix in suffixes {
            let name = suffix.to_ascii_lowercase();
            if Url::parse(&format!("https://{name}/"))
                .map(|u| u.host != name)
                .unwrap_or(true)
            {
                return Err(GrantError::NotAName(name));
            }
            if !name.contains('.') || SHARED.contains(&name.as_str()) {
                return Err(GrantError::Shared(name));
            }
            kept.push(name);
        }
        Ok(Self { suffixes: kept })
    }

    /// Whether `host` on `port` is in the grant: a suffix, or a name under
    /// one at a label boundary, on [`PORT`].
    pub fn allows(&self, host: &str, port: u16) -> bool {
        port == PORT
            && self.suffixes.iter().any(|suffix| {
                host == suffix
                    || host
                        .strip_suffix(suffix.as_str())
                        .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1)
            })
    }
}

/// What an app asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub method: Method,
    pub range: Option<Range>,
}

/// What an app is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// The URL that answered, after any redirects.
    pub url: String,
    /// At most 128 printable bytes (`http::head::MAX_CONTENT_TYPE`).
    pub content_type: Option<String>,
    /// When to come back, in seconds, as a 429 or 503 says.
    pub retry_after: Option<u32>,
    pub body: Vec<u8>,
}

/// Why an app's request failed, as `kai:net/http`'s `error` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Refused(String),
    BadUrl(String),
    Tls(String),
    Http(String),
    TooLarge,
    Timeout,
    Truncated,
}

impl From<HttpError> for Error {
    fn from(error: HttpError) -> Self {
        match error {
            HttpError::Refused(host) => Error::Refused(why(&host)),
            HttpError::Url(text) => Error::BadUrl(why(text)),
            HttpError::Tls(tls) => Error::Tls(why(&format!("{tls:?}"))),
            HttpError::Connect(text) => Error::Tls(why(text)),
            HttpError::TooLarge => Error::TooLarge,
            HttpError::Timeout => Error::Timeout,
            HttpError::Truncated => Error::Truncated,
            HttpError::Head(text) | HttpError::Body(text) => Error::Http(why(text)),
            HttpError::TooManyRedirects => Error::Http(why("more than five redirects")),
            HttpError::RangeIgnored => Error::Http(why("the server ignored the range")),
        }
    }
}

/// `text` as an app is given it: printable ASCII, anything else as `?`, at
/// most [`MAX_WHY`] bytes.
pub fn why(text: &str) -> String {
    text.bytes()
        .take(MAX_WHY)
        .map(|b| match b {
            b' ' => ' ',
            b if b.is_ascii_graphic() => b as char,
            _ => '?',
        })
        .collect()
}

/// Makes `request` for an app holding `grant`, through `connect`, by
/// `deadline_ns` on `clock`.
pub fn serve(
    grant: &Grant,
    request: &Request,
    connect: &mut impl Connect,
    clock: &dyn Fn() -> u64,
    deadline_ns: u64,
) -> Result<Response, Error> {
    let url = Url::parse(&request.url).map_err(|text| Error::BadUrl(why(text)))?;
    if !grant.allows(&url.host, url.port) {
        return Err(Error::Refused(why(&url.host)));
    }
    let allow = |host: &str, port: u16| grant.allows(host, port);
    let fetch = Fetch {
        limits: Limits::new(MAX_BODY as u64, deadline_ns),
        allow: &allow,
        clock,
    };
    let mut body = Vec::new();
    let got = fetch.run(connect, request.method, &url, request.range, &mut |bytes| {
        body.extend_from_slice(bytes);
        Ok(())
    })?;
    Ok(Response {
        status: got.head.status,
        url: format!("https://{}{}", got.url.authority(), got.url.path),
        content_type: got.head.content_type,
        retry_after: got.head.retry_after,
        body,
    })
}

/// The most redirects one request follows, across its calls to the `fetch`
/// service (task 5's limit).
pub const MAX_HOPS: u8 = 5;

/// What the `fetch` service gives its caller for `request` (P5 task 12): as
/// [`serve`], on the caller's own handle, whose `allow` says which hosts it
/// reaches (in the OS, the kernel's RESOLVE_ON). The request's host must be
/// one of them. A redirect to one of them is followed, at most [`MAX_HOPS`]
/// less `hops` of them; a redirect to another comes back unfollowed, as its
/// 3xx with no body and its Location made absolute, for the caller to ask
/// about the new host and call again with a handle for it and `hops` + 1.
pub fn serve_within(
    request: &Request,
    hops: u8,
    allow: &dyn Fn(&str, u16) -> bool,
    connect: &mut impl Connect,
    clock: &dyn Fn() -> u64,
    deadline_ns: u64,
) -> Result<(Response, Option<String>), Error> {
    let url = Url::parse(&request.url).map_err(|text| Error::BadUrl(why(text)))?;
    let Some(left) = MAX_HOPS.checked_sub(hops) else {
        return Err(Error::Http(why("more than five redirects")));
    };
    let mut limits = Limits::new(MAX_BODY as u64, deadline_ns);
    limits.max_redirects = left;
    let fetch = Fetch {
        limits,
        allow,
        clock,
    };
    let mut body = Vec::new();
    let got = fetch.run_within(connect, request.method, &url, request.range, &mut |bytes| {
        body.extend_from_slice(bytes);
        Ok(())
    })?;
    // A redirect that came back is one whose host `allow` refused.
    let location = match got.head.status {
        300..=399 => {
            let location = got.head.location.as_deref().unwrap_or_default();
            let next = got
                .url
                .join(location)
                .map_err(|_| Error::Http(why("a redirect to a URL that is not https")))?;
            Some(format!("https://{}{}", next.authority(), next.path))
        }
        _ => None,
    };
    let response = Response {
        status: got.head.status,
        url: format!("https://{}{}", got.url.authority(), got.url.path),
        content_type: got.head.content_type,
        retry_after: got.head.retry_after,
        body,
    };
    Ok((response, location))
}

/// One app instance's waiting requests, by ticket. A ticket is never given
/// twice: once the last `u32` is given, every later call traps.
#[derive(Debug, Default)]
pub struct Queue {
    next: u32,
    spent: bool,
    waiting: VecDeque<(u32, Request)>,
}

impl Queue {
    /// Queues `request`; its ticket, or `None` when [`MAX_WAITING`] wait or
    /// the tickets are spent (the call then traps).
    pub fn push(&mut self, request: Request) -> Option<u32> {
        if self.waiting.len() >= MAX_WAITING || self.spent {
            return None;
        }
        let ticket = self.next;
        match self.next.checked_add(1) {
            Some(next) => self.next = next,
            None => self.spent = true,
        }
        self.waiting.push_back((ticket, request));
        Some(ticket)
    }

    /// The oldest waiting request.
    pub fn take(&mut self) -> Option<(u32, Request)> {
        self.waiting.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_takes_names_and_refuses_shared_suffixes() {
        let grant = Grant::new(&["API.Weather.example", "models.kai.test"]).unwrap();
        assert!(grant.allows("api.weather.example", 443));
        assert!(grant.allows("eu.api.weather.example", 443));
        assert!(grant.allows("models.kai.test", 443));
        for (host, port) in [
            ("api.weather.example", 8443),
            ("evilapi.weather.example", 443),
            ("weather.example", 443),
            ("api.weather.example.evil", 443),
            (".api.weather.example", 443),
            ("kai.test", 443),
        ] {
            assert!(!grant.allows(host, port), "{host}:{port}");
        }
        for (suffix, error) in [
            ("com", GrantError::Shared("com".into())),
            ("co.uk", GrantError::Shared("co.uk".into())),
            ("github.io", GrantError::Shared("github.io".into())),
            (
                "*.example.com",
                GrantError::NotAName("*.example.com".into()),
            ),
            ("10.0.2.2", GrantError::NotAName("10.0.2.2".into())),
            ("a..b", GrantError::NotAName("a..b".into())),
            ("", GrantError::NotAName(String::new())),
            ("host:443", GrantError::NotAName("host:443".into())),
            ("a.test/path", GrantError::NotAName("a.test/path".into())),
        ] {
            assert_eq!(Grant::new(&[suffix]), Err(error), "{suffix:?}");
        }
        let many: Vec<String> = (0..=MAX_SUFFIXES).map(|i| format!("h{i}.test")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(Grant::new(&many), Err(GrantError::TooMany));
        assert!(Grant::new(&[]).is_ok_and(|g| !g.allows("a.test", 443)));
    }

    #[test]
    fn at_most_four_requests_wait_and_tickets_count_up() {
        let request = Request {
            url: "https://a.test/".into(),
            method: Method::Get,
            range: None,
        };
        let mut queue = Queue::default();
        let tickets: Vec<_> = (0..5).map(|_| queue.push(request.clone())).collect();
        assert_eq!(tickets, [Some(0), Some(1), Some(2), Some(3), None]);
        assert_eq!(queue.take().map(|(t, _)| t), Some(0));
        assert_eq!(queue.push(request.clone()), Some(4));
        // The last ticket is given once, and then the queue is spent.
        let mut last = Queue {
            next: u32::MAX - 1,
            ..Queue::default()
        };
        assert_eq!(last.push(request.clone()), Some(u32::MAX - 1));
        assert_eq!(last.push(request.clone()), Some(u32::MAX));
        last.take();
        last.take();
        assert_eq!(last.push(request), None);
    }

    #[test]
    fn an_apps_why_is_printable_and_short() {
        assert_eq!(why("a bad\ncertificate\u{e9}"), "a bad?certificate??");
        assert_eq!(why(&"x".repeat(500)).len(), MAX_WHY);
        assert_eq!(
            Error::from(HttpError::Head("a protocol switch")),
            Error::Http("a protocol switch".into())
        );
    }

    #[test]
    fn a_grant_may_not_name_a_hosting_parent() {
        for parent in ["amazonaws.com", "googleusercontent.com"] {
            assert_eq!(
                Grant::new(&[parent]),
                Err(GrantError::Shared(parent.into()))
            );
        }
        assert!(Grant::new(&["my-bucket.s3.amazonaws.com"]).is_ok());
    }
}
