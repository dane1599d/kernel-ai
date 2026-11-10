//! The `fetch` service's messages (P5 task 12): what a caller sends with the
//! `NetDest` handle it moves in, and what `fetch` answers, so `fetch`,
//! comphost and the shell read and write them with one parser.
//!
//! A request ([`Ask`]) is one line: `get <url> [range=<first>-[<last>]]
//! [hops=<n>]` or `head <url> [hops=<n>]`, where `hops` counts the redirects
//! the caller already took.
//!
//! An answer is `err <kind>[ <why>]` on one line, or `ok <status>` and then
//! one field a line ([`Head`]): `len <n>`; `hops <n>`, the request's and the
//! redirects followed within its handle; `type <content-type>` and
//! `retry <seconds>` when the server sent them; `url <the URL that
//! answered>`; and `location <url>` when a redirect to a host the handle does
//! not reach came back unfollowed, which the caller follows with `hops` + 1. A body of `len` bytes comes in an `Shm`
//! that `fetch` makes and moves back with the answer.

use crate::fetch::{Error, MAX_HOPS, Request, Served, why};
use crate::http::{Method, Range};
use alloc::format;
use alloc::string::{String, ToString};

/// The longest URL a request names, as `kai:net/http` takes it.
pub const MAX_URL: usize = 2048;
/// The longest request line.
pub const MAX_ASK: usize = MAX_URL + 64;
/// The longest answer: two URLs, a content type and the numbers.
pub const MAX_ANSWER: usize = 2 * MAX_URL + 512;
/// What `fetch` answers when a response's head would not fit in
/// [`MAX_ANSWER`]: a server's long content type, or a redirect's long URL.
pub const TOO_LONG: &str = "err http the response's head is too long to pass on";

/// Whether `value` may be a field's value in an answer: no control character,
/// so it stays on its line and reads back the same.
fn printable(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

/// `value` as a field's value: each control character as `?`.
fn field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// A request, as a caller sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub request: Request,
    /// Redirects the caller has taken already, at most [`MAX_HOPS`].
    pub hops: u8,
}

impl Ask {
    /// Its line.
    pub fn text(&self) -> String {
        let method = match self.request.method {
            Method::Get => "get",
            Method::Head => "head",
        };
        let mut text = format!("{method} {}", self.request.url);
        if let Some(range) = self.request.range {
            text += &format!(" range={}-", range.first);
            if let Some(last) = range.last {
                text += &last.to_string();
            }
        }
        if self.hops > 0 {
            text += &format!(" hops={}", self.hops);
        }
        text
    }

    /// The request in `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.len() > MAX_ASK {
            return Err("the request is too long");
        }
        let mut words = text.split(' ');
        let method = match words.next() {
            Some("get") => Method::Get,
            Some("head") => Method::Head,
            _ => return Err("the request is not get or head"),
        };
        let url = words
            .next()
            .filter(|u| !u.is_empty())
            .ok_or("the request names no URL")?;
        if url.len() > MAX_URL || !url.starts_with("https://") {
            return Err("the URL is not an https URL of at most 2,048 bytes");
        }
        let (mut range, mut hops) = (None, 0);
        for word in words {
            match word.split_once('=') {
                Some(("range", value)) if range.is_none() && method == Method::Get => {
                    range = Some(parse_range(value)?);
                }
                Some(("hops", value)) => {
                    hops = value
                        .parse::<u8>()
                        .ok()
                        .filter(|&n| n <= MAX_HOPS && !value.starts_with('+'))
                        .ok_or("hops is not 0 to 5")?;
                }
                _ => return Err("the request has a word it does not take"),
            }
        }
        Ok(Self {
            request: Request {
                url: url.to_string(),
                method,
                range,
            },
            hops,
        })
    }
}

fn parse_range(value: &str) -> Result<Range, &'static str> {
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u64>().ok())
            .flatten()
    };
    let (first, last) = value.split_once('-').ok_or("a range is <first>-[<last>]")?;
    let first = number(first).ok_or("a range's first byte is not a number")?;
    let last = match last {
        "" => None,
        last => Some(number(last).ok_or("a range's last byte is not a number")?),
    };
    if last.is_some_and(|last| last < first) {
        return Err("a range ends before it starts");
    }
    Ok(Range { first, last })
}

/// What an `ok` answer says about the response: all but its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    /// The body's length, in the `Shm` moved with the answer.
    pub len: usize,
    /// The request's hops and the redirects followed within its handle.
    pub hops: u8,
    pub content_type: Option<String>,
    pub retry_after: Option<u32>,
    /// The URL that answered.
    pub url: String,
    /// Where an unfollowed redirect leads: a host the handle does not reach.
    pub location: Option<String>,
}

impl Head {
    /// The head of what `fetch` served.
    pub fn of(served: &Served) -> Self {
        let response = &served.response;
        Self {
            status: response.status,
            len: response.body.len(),
            hops: served.hops,
            content_type: response.content_type.clone(),
            retry_after: response.retry_after,
            url: response.url.clone(),
            location: served.location.clone(),
        }
    }
}

/// An error's name in an answer, and its why if it has one.
fn kind(error: &Error) -> (&'static str, Option<&str>) {
    match error {
        Error::Refused(host) => ("refused", Some(host)),
        Error::BadUrl(why) => ("bad-url", Some(why)),
        Error::Tls(why) => ("tls", Some(why)),
        Error::Http(why) => ("http", Some(why)),
        Error::TooLarge => ("too-large", None),
        Error::Timeout => ("timeout", None),
        Error::Truncated => ("truncated", None),
    }
}

/// The answer to send for `result`: [`TOO_LONG`] if it would not fit.
pub fn answer(result: &Result<Head, Error>) -> String {
    let text = render(result);
    match text.len() > MAX_ANSWER {
        true => String::from(TOO_LONG),
        false => text,
    }
}

fn render(result: &Result<Head, Error>) -> String {
    match result {
        Err(error) => match kind(error) {
            (name, Some(text)) => format!("err {name} {}", why(text)),
            (name, None) => format!("err {name}"),
        },
        Ok(head) => {
            let mut text = format!("ok {}\nlen {}\nhops {}\n", head.status, head.len, head.hops);
            if let Some(content_type) = &head.content_type {
                text += &format!("type {}\n", field(content_type));
            }
            if let Some(retry) = head.retry_after {
                text += &format!("retry {retry}\n");
            }
            text += &format!("url {}\n", field(&head.url));
            if let Some(location) = &head.location {
                text += &format!("location {}\n", field(location));
            }
            text
        }
    }
}

/// The answer in `text`: the response's head, or the error `fetch` gave; or
/// why `text` is not an answer.
pub fn parse_answer(text: &str) -> Result<Result<Head, Error>, &'static str> {
    if text.len() > MAX_ANSWER {
        return Err("the answer is too long");
    }
    if let Some(rest) = text.strip_prefix("err ") {
        let (name, why) = match rest.split_once(' ') {
            Some((name, why)) => (name, why.to_string()),
            None => (rest, String::new()),
        };
        // A why is what `fetch::why` makes: printable ASCII, at most its
        // limit; an error with no why has none.
        if why != crate::fetch::why(&why) {
            return Err("an error's why is not printable text of at most 200 bytes");
        }
        let none = |error| match why.is_empty() {
            true => Ok(error),
            false => Err("this error has no why"),
        };
        return Ok(Err(match name {
            "refused" => Error::Refused(why),
            "bad-url" => Error::BadUrl(why),
            "tls" => Error::Tls(why),
            "http" => Error::Http(why),
            "too-large" => none(Error::TooLarge)?,
            "timeout" => none(Error::Timeout)?,
            "truncated" => none(Error::Truncated)?,
            _ => return Err("an error this answer does not name"),
        }));
    }
    let mut lines = text.lines();
    let status = lines
        .next()
        .and_then(|line| line.strip_prefix("ok "))
        .and_then(|n| n.parse::<u16>().ok())
        .filter(|n| (100..=599).contains(n))
        .ok_or("the answer is not ok <status> or err")?;
    let (mut len, mut hops, mut content_type, mut retry_after, mut url, mut location) =
        (None, None, None, None, None, None);
    for line in lines {
        let (key, value) = line.split_once(' ').ok_or("a field with no value")?;
        if !printable(value) {
            return Err("a field's value has a control character");
        }
        let set = match key {
            "len" => len
                .replace(value.parse::<usize>().map_err(|_| "len is not a number")?)
                .is_none(),
            "hops" => hops
                .replace(
                    value
                        .parse::<u8>()
                        .ok()
                        .filter(|&n| n <= MAX_HOPS)
                        .ok_or("hops is not 0 to 5")?,
                )
                .is_none(),
            "type" => content_type.replace(value.to_string()).is_none(),
            "retry" => retry_after
                .replace(value.parse::<u32>().map_err(|_| "retry is not a number")?)
                .is_none(),
            "url" => url.replace(value.to_string()).is_none(),
            "location" => location.replace(value.to_string()).is_none(),
            _ => return Err("a field this answer does not have"),
        };
        if !set {
            return Err("a field given twice");
        }
    }
    Ok(Ok(Head {
        status,
        len: len.ok_or("an answer with no len")?,
        hops: hops.ok_or("an answer with no hops")?,
        content_type,
        retry_after,
        url: url.ok_or("an answer with no url")?,
        location,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(text: &str) -> Ask {
        Ask::parse(text).unwrap()
    }

    #[test]
    fn a_request_reads_back_as_it_was_written() {
        for text in [
            "get https://a.example/x",
            "head https://a.example/",
            "get https://a.example/x range=4-",
            "get https://a.example/x range=4-9 hops=2",
        ] {
            assert_eq!(ask(text).text(), text);
        }
        let got = ask("get https://a.example/x range=4-9 hops=5");
        assert_eq!(
            got.request.range,
            Some(Range {
                first: 4,
                last: Some(9)
            })
        );
        assert_eq!(got.hops, 5);
    }

    #[test]
    fn a_bad_request_is_refused_by_name() {
        for (text, why) in [
            ("post https://a.example/", "the request is not get or head"),
            (
                "get http://a.example/",
                "the URL is not an https URL of at most 2,048 bytes",
            ),
            ("get", "the request names no URL"),
            ("get https://a.example/ hops=6", "hops is not 0 to 5"),
            ("get https://a.example/ hops=+1", "hops is not 0 to 5"),
            (
                "get https://a.example/ range=9-4",
                "a range ends before it starts",
            ),
            (
                "get https://a.example/ range=x-",
                "a range's first byte is not a number",
            ),
            (
                "head https://a.example/ range=0-",
                "the request has a word it does not take",
            ),
            (
                "get https://a.example/ cookie=1",
                "the request has a word it does not take",
            ),
        ] {
            assert_eq!(Ask::parse(text), Err(why), "{text}");
        }
        let long = format!("get https://a.example/{}", "x".repeat(MAX_URL));
        assert!(Ask::parse(&long).is_err());
    }

    #[test]
    fn an_answer_reads_back_as_it_was_written() {
        let head = Head {
            status: 302,
            len: 0,
            hops: 2,
            content_type: Some("text/html; charset=utf-8".into()),
            retry_after: Some(7),
            url: "https://a.example/x".into(),
            location: Some("https://b.example/y".into()),
        };
        let text = answer(&Ok(head.clone()));
        assert_eq!(
            text,
            "ok 302\nlen 0\nhops 2\ntype text/html; charset=utf-8\nretry 7\nurl https://a.example/x\nlocation https://b.example/y\n"
        );
        assert_eq!(parse_answer(&text), Ok(Ok(head)));
        let plain = Head {
            status: 200,
            len: 16,
            hops: 0,
            content_type: None,
            retry_after: None,
            url: "https://a.example/".into(),
            location: None,
        };
        assert_eq!(parse_answer(&answer(&Ok(plain.clone()))), Ok(Ok(plain)));
        for error in [
            Error::Refused("b.example".into()),
            Error::Tls("UnknownIssuer".into()),
            Error::Http("more than five redirects".into()),
            Error::TooLarge,
            Error::Timeout,
            Error::Truncated,
        ] {
            assert_eq!(parse_answer(&answer(&Err(error.clone()))), Ok(Err(error)));
        }
    }

    #[test]
    fn a_broken_answer_is_not_taken() {
        for text in [
            "",
            "ok",
            "ok 99\nlen 0\nhops 0\nurl https://a/",
            "ok 200\nhops 0\nurl https://a/",
            "ok 200\nlen 1\nurl https://a/",
            "ok 200\nlen 1\nhops 6\nurl https://a/",
            "ok 200\nlen 1\nlen 2\nhops 0\nurl https://a/",
            "ok 200\nlen 1\nhops 0\nurl https://a/\ncookie x",
            "err nope",
        ] {
            assert!(parse_answer(text).is_err(), "{text:?}");
        }
    }
}
