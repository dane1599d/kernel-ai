//! The request and reply text, read as hostile: what reads writes back byte for
//! byte, and anything else is refused whole.

use crate::text::{Hit, MAX_QUERY, MAX_SCORE, MAX_WHY, Reply, Request};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use semindex::search::MAX_HITS;

fn requests() -> Vec<Request> {
    let full = Request {
        query: "hotel in lisbon".to_string(),
        since_ns: Some(1_700_000_000_000_000_000),
        folder: Some("/home/user/notes".to_string()),
        limit: Some(5),
    };
    vec![
        Request {
            query: "x".to_string(),
            since_ns: None,
            folder: None,
            limit: None,
        },
        Request {
            since_ns: Some(0),
            folder: None,
            ..full.clone()
        },
        Request {
            limit: Some(MAX_HITS),
            ..full.clone()
        },
        Request {
            query: "q".repeat(MAX_QUERY),
            ..full.clone()
        },
        full,
    ]
}

fn hit(path: &str, score: u32) -> Hit {
    Hit {
        score,
        modified_ns: 1_700_000_000_000_000_000,
        offset: 400,
        len: 1200,
        path: path.to_string(),
    }
}

fn replies() -> Vec<Reply> {
    vec![
        Reply::Hits(vec![]),
        Reply::Hits(vec![
            hit("/home/user/notes/trip.txt", MAX_SCORE),
            hit("/home/user/my notes/a b.txt", 0),
        ]),
        Reply::Hits(
            (0..MAX_HITS)
                .map(|n| hit(&format!("/home/user/{n}"), 500))
                .collect(),
        ),
        Reply::Err("the folder is not under /home/user".to_string()),
    ]
}

#[test]
fn every_request_and_reply_reads_back_as_it_was_written() {
    assert_eq!(
        requests()[4].encode(),
        "search 1\nquery hotel in lisbon\nsince 1700000000000000000\n\
         folder /home/user/notes\nlimit 5"
    );
    for request in requests() {
        let text = request.encode();
        assert_eq!(Request::parse(&text), Some(request), "{text:?}");
    }
    for reply in replies() {
        let text = reply.encode();
        assert_eq!(Reply::parse(&text), Some(reply), "{text:?}");
    }
}

#[test]
fn a_request_out_of_shape_is_refused_whole() {
    for text in [
        "",
        "search 1",
        "search 1\n",
        "search 2\nquery x",
        "Search 1\nquery x",
        "search 1\r\nquery x",
        "search 1\nquery ",
        "search 1\nquery  x",
        "search 1\nquery x ",
        "search 1\nquery x\ty",
        "search 1\nquery caf\u{e9}",
        "search 1\nquery x\n",
        "search 1\nquery x\nsince ",
        "search 1\nquery x\nsince 01",
        "search 1\nquery x\nsince -1",
        "search 1\nquery x\nsince 18446744073709551616",
        "search 1\nquery x\nfolder /home/user/../system",
        "search 1\nquery x\nfolder /system",
        "search 1\nquery x\nfolder /home/username",
        "search 1\nquery x\nfolder home/user",
        "search 1\nquery x\nfolder /home/user/",
        "search 1\nquery x\nlimit 0",
        "search 1\nquery x\nlimit 21",
        "search 1\nquery x\nlimit 5\nsince 1",
        "search 1\nquery x\nfolder /home/user\nsince 1",
        "search 1\nquery x\nsince 1\nsince 2",
        "search 1\nquery x\nmodel other",
    ] {
        assert_eq!(Request::parse(text), None, "{text:?}");
    }
    let long = format!("search 1\nquery {}", "q".repeat(MAX_QUERY + 1));
    assert_eq!(Request::parse(&long), None);
}

#[test]
fn a_reply_out_of_shape_is_refused_whole() {
    let good = "hit 500 1 0 10 /home/user/a.txt";
    for text in [
        "",
        "ok",
        "ok ",
        "ok 01",
        "ok 1",
        "ok 0\n",
        format!("ok 2\n{good}").as_str(),
        format!("ok 1\n{good}\n").as_str(),
        "ok 1\nhit 1001 1 0 10 /home/user/a.txt",
        "ok 1\nhit 500 1 0 10 /home/user/../a.txt",
        "ok 1\nhit 500 1 0 10 /system/a.txt",
        "ok 1\nhit 500 1 0 10 /home/user",
        "ok 1\nhit 500 1 0 10 /home/user/",
        "ok 1\nhit 500 1 0 10",
        "ok 1\nhit 500 1 0 4294967296 /home/user/a.txt",
        "ok 1\nhit  500 1 0 10 /home/user/a.txt",
        "err ",
        "err  padded",
        "err line\nok 0",
        "error",
    ] {
        assert_eq!(Reply::parse(text), None, "{text:?}");
    }
    let many: Vec<Hit> = (0..=MAX_HITS)
        .map(|n| hit(&format!("/home/user/{n}"), 1))
        .collect();
    assert_eq!(Reply::parse(&Reply::Hits(many).encode()), None);
}

#[test]
fn a_why_is_written_as_one_line() {
    let text = Reply::Err("bad\nok 1\nhit 1 1 1 1 /home/user/x\u{1b}".repeat(20)).encode();
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.len() <= "err ".len() + MAX_WHY);
    assert!(matches!(Reply::parse(&text), Some(Reply::Err(_))));
    assert_eq!(Reply::Err(String::new()).encode(), "err no reason given");
}

/// Damage to any byte of a good request or reply is refused, or reads as one that
/// writes back as those very bytes.
#[test]
fn damaged_text_is_refused_or_reads_back_unchanged() {
    let texts: Vec<String> = requests()
        .iter()
        .map(Request::encode)
        .chain(replies().iter().map(Reply::encode))
        .filter(|text| text.len() < 2_000)
        .collect();
    for text in &texts {
        for at in 0..text.len() {
            for with in [" ", "\n", "0", "9", "/", ".", "x", ""] {
                let damaged = format!("{}{with}{}", &text[..at], &text[at + 1..]);
                if let Some(request) = Request::parse(&damaged) {
                    assert_eq!(request.encode(), damaged);
                }
                if let Some(reply) = Reply::parse(&damaged) {
                    assert_eq!(reply.encode(), damaged);
                }
            }
        }
    }
}
