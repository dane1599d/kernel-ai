//! Hostile tests of `kainet::wire`, the `fetch` service's messages (plan
//! p5-networking §1's host gates: hostile tests and a fuzz target for every
//! new parser). A caller's request line reaches `fetch` from another process,
//! and `fetch`'s answer reaches comphost and the shell: each side reads the
//! other's bytes. What reads must read back the same once written again, keep
//! within its buffer, and never put a control character on a line; what does
//! not read is refused by name.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kainet::fetch::{Error, Request, Response, Served};
use kainet::http::{Method, Range};
use kainet::wire::{self, Ask, Head, MAX_ANSWER, MAX_ASK, MAX_URL, TOO_LONG};

const CASES: usize = 5_000;

/// What reads, written again, reads back the same.
fn ask_holds(text: &str) {
    if let Ok(ask) = Ask::parse(text) {
        assert!(ask.request.url.len() <= MAX_URL);
        assert!(ask.hops <= kainet::fetch::MAX_HOPS);
        let again = ask.text();
        assert!(again.len() <= MAX_ASK, "{again:?}");
        assert_eq!(Ask::parse(&again), Ok(ask), "{text:?}");
    }
}

fn answer_holds(text: &str) {
    if let Ok(result) = wire::parse_answer(text) {
        if let Ok(head) = &result {
            for value in [
                Some(&head.url),
                head.location.as_ref(),
                head.content_type.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                assert!(!value.chars().any(char::is_control), "{value:?}");
            }
        }
        let again = wire::answer(&result);
        assert!(again.len() <= MAX_ANSWER);
        assert_eq!(wire::parse_answer(&again), Ok(result), "{text:?}");
    }
}

#[test]
fn hostile_requests_are_refused_or_read_back_the_same() {
    for sample in [
        "get https://releases.kai.test/dl/a.kpkg range=0-2097151 hops=2",
        "get https://a.test/x range=7-",
        "head https://updates.kai.test/metadata/timestamp.json hops=5",
    ] {
        hostile::fuzz(sample.as_bytes(), 0x5_a5c0, CASES, |bytes| {
            if let Ok(text) = core::str::from_utf8(bytes) {
                ask_holds(text);
            }
        });
        hostile::truncations(sample.as_bytes(), |_, bytes| {
            ask_holds(core::str::from_utf8(bytes).unwrap_or(""));
        });
    }
}

#[test]
fn hostile_answers_are_refused_or_read_back_the_same() {
    for sample in [
        "ok 200\nlen 5\nhops 1\ntype text/plain\nretry 30\nurl https://a.test/x\n",
        "ok 301\nlen 0\nhops 2\nurl https://a.test/x\nlocation https://b.test/y\n",
        "err refused elsewhere.kai.test",
        "err tls InvalidCertificate(Expired)",
        "err timeout",
    ] {
        hostile::fuzz(sample.as_bytes(), 0x5_a5c1, CASES, |bytes| {
            if let Ok(text) = core::str::from_utf8(bytes) {
                answer_holds(text);
            }
        });
        hostile::truncations(sample.as_bytes(), |_, bytes| {
            answer_holds(core::str::from_utf8(bytes).unwrap_or(""));
        });
    }
}

#[test]
fn hostile_request_lines_are_refused_by_name() {
    let long = format!("get https://a.test/{}", "x".repeat(MAX_URL));
    for (text, why) in [
        ("put https://a.test/x", "the request is not get or head"),
        (
            "get http://a.test/x",
            "the URL is not an https URL of at most 2,048 bytes",
        ),
        (
            long.as_str(),
            "the URL is not an https URL of at most 2,048 bytes",
        ),
        ("get", "the request names no URL"),
        ("get  https://a.test/x", "the request names no URL"),
        ("get https://a.test/x hops=6", "hops is not 0 to 5"),
        ("get https://a.test/x hops=+1", "hops is not 0 to 5"),
        (
            "get https://a.test/x range=5-4",
            "a range ends before it starts",
        ),
        (
            "get https://a.test/x range=-4",
            "a range's first byte is not a number",
        ),
        (
            "get https://a.test/x range=+1-",
            "a range's first byte is not a number",
        ),
        (
            "get https://a.test/x range=0- range=1-",
            "the request has a word it does not take",
        ),
        (
            "head https://a.test/x range=0-",
            "the request has a word it does not take",
        ),
        (
            "get https://a.test/x ",
            "the request has a word it does not take",
        ),
    ] {
        assert_eq!(Ask::parse(text), Err(why), "{text:?}");
    }
    assert_eq!(
        Ask::parse(&"g".repeat(MAX_ASK + 1)),
        Err("the request is too long")
    );
}

#[test]
fn hostile_answers_are_refused_by_name() {
    let long_why = format!("err http {}", "w".repeat(201));
    for (text, why) in [
        (
            "ok 200\nlen 5\nhops 1\nurl https://a.test/x\r",
            "a field's value has a control character",
        ),
        (
            "ok 200\nlen 5\nhops 1\ntype a\tb\nurl https://a.test/x",
            "a field's value has a control character",
        ),
        (
            "ok 200\nlen 5\nlen 6\nhops 1\nurl https://a.test/x",
            "a field given twice",
        ),
        (
            "ok 200\nlen 5\nhops 6\nurl https://a.test/x",
            "hops is not 0 to 5",
        ),
        (
            "ok 200\nlen -1\nhops 1\nurl https://a.test/x",
            "len is not a number",
        ),
        (
            "ok 200\nlen 5\nhops 1\nbody x\nurl https://a.test/x",
            "a field this answer does not have",
        ),
        (
            "ok 200\n\nlen 5\nhops 1\nurl https://a.test/x",
            "a field with no value",
        ),
        ("ok 200\nlen 5\nhops 1", "an answer with no url"),
        (
            "ok 600\nlen 5\nhops 1\nurl https://a.test/x",
            "the answer is not ok <status> or err",
        ),
        (
            "err refused a\nb",
            "an error's why is not printable text of at most 200 bytes",
        ),
        (
            "err tls caf\u{e9}",
            "an error's why is not printable text of at most 200 bytes",
        ),
        (
            long_why.as_str(),
            "an error's why is not printable text of at most 200 bytes",
        ),
        ("err too-large 5", "this error has no why"),
        ("err gone", "an error this answer does not name"),
    ] {
        assert_eq!(wire::parse_answer(text), Err(why), "{text:?}");
    }
    assert_eq!(
        wire::parse_answer(&"o".repeat(MAX_ANSWER + 1)),
        Err("the answer is too long")
    );
}

fn served(content_type: &str, url: &str, location: Option<&str>) -> Served {
    Served {
        response: Response {
            status: if location.is_some() { 302 } else { 200 },
            url: url.into(),
            content_type: Some(content_type.into()),
            retry_after: None,
            body: vec![7; 3],
        },
        location: location.map(Into::into),
        hops: 1,
    }
}

/// A server's content type with a line break cannot add a field: it stays on
/// its line, its control characters as `?`.
#[test]
fn a_served_heads_control_characters_never_leave_their_line() {
    let head = Head::of(&served("text/plain\nlen 999", "https://a.test/x", None));
    let text = wire::answer(&Ok(head));
    let Ok(Ok(back)) = wire::parse_answer(&text) else {
        panic!("{text:?} does not read");
    };
    assert_eq!(back.len, 3);
    assert_eq!(back.content_type.as_deref(), Some("text/plain?len 999"));
}

/// A head too long to send whole is an error the caller reads, not an answer
/// cut short.
#[test]
fn a_head_too_long_to_send_becomes_an_error() {
    let long = format!("https://b.test/{}", "y".repeat(MAX_ANSWER));
    for head in [
        Head::of(&served(&"t".repeat(MAX_ANSWER), "https://a.test/x", None)),
        Head::of(&served("text/plain", "https://a.test/x", Some(&long))),
    ] {
        let text = wire::answer(&Ok(head));
        assert_eq!(text, TOO_LONG);
        assert!(matches!(wire::parse_answer(&text), Ok(Err(Error::Http(_)))));
    }
}

/// Each of `fetch`'s own errors, with the longest why it gives, reads back.
#[test]
fn every_error_fetch_gives_reads_back() {
    let why = kainet::fetch::why(&"z".repeat(500));
    for error in [
        Error::Refused(why.clone()),
        Error::BadUrl(why.clone()),
        Error::Tls(why.clone()),
        Error::Http(why),
        Error::TooLarge,
        Error::Timeout,
        Error::Truncated,
    ] {
        let result = Err(error);
        assert_eq!(wire::parse_answer(&wire::answer(&result)), Ok(result));
    }
}

/// Reading a message allocates a bounded amount, whatever its bytes say.
#[test]
fn reading_a_message_allocates_little() {
    let request = format!(
        "get https://a.test/{} range=0-1 hops=1",
        "p".repeat(MAX_URL - 20)
    );
    let answer = format!(
        "ok 200\nlen 18446744073709551615\nhops 5\nurl https://a.test/{}\n",
        "q".repeat(MAX_URL)
    );
    let (_, peak) = hostile::peak_alloc(|| {
        let _ = Ask::parse(&request);
        let _ = wire::parse_answer(&answer);
    });
    assert!(peak <= 4 * (request.len() + answer.len()), "{peak} bytes");
    let ask = Ask {
        request: Request {
            url: "https://a.test/x".into(),
            method: Method::Get,
            range: Some(Range {
                first: 1,
                last: None,
            }),
        },
        hops: 0,
    };
    assert_eq!(ask.text(), "get https://a.test/x range=1-");
}
