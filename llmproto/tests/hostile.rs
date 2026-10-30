//! Hostile network replies and wire messages: HTTP responses, OpenAI JSON, tune
//! requests and completions, replay files and traced transcripts, cut at every
//! byte, swept with extreme lengths and mutated thousands of times, either parse
//! or fail with an error. None panics, none waits forever on a closed
//! connection, and none allocates more than a small multiple of its input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use llmproto::chat::Completion;
use llmproto::http::{self, MAX_RESPONSE, ParseError};
use llmproto::openai::{self, MAX_JSON_DEPTH, ReplyError};
use llmproto::tune::{self, TuneRequest, replay};

const ALLOC_PER_BYTE: usize = 4;
const ALLOC_FLOOR: usize = 16 * 1024;
/// A traced line costs its piece, its number and a map entry: tens of bytes for a
/// line of thirty.
const TRACE_ALLOC_PER_BYTE: usize = 16;

const SAMPLES: [&[u8]; 3] = [
    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"a\":[1,2]}",
    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\n\r\n",
    b"HTTP/1.0 503 Busy\r\nX-Retry: 3\r\n\r\ncome back later",
];

/// Parses `buf` both ways and checks what any caller relies on: a closed
/// connection always gets an answer, a body is never over the limit, and the
/// allocation follows the input.
fn check_http(buf: &[u8]) {
    for closed in [false, true] {
        let (result, peak) = hostile::peak_alloc(|| http::parse_response(buf, closed));
        let cap = ALLOC_PER_BYTE * buf.len() + ALLOC_FLOOR;
        assert!(
            peak <= cap,
            "{peak} bytes allocated for {} bytes",
            buf.len()
        );
        match result {
            Ok(response) => assert!(response.body.len() <= buf.len().min(MAX_RESPONSE)),
            Err(ParseError::Incomplete) => assert!(!closed, "a closed connection waits"),
            Err(_) => {}
        }
    }
}

#[test]
fn http_every_prefix() {
    for sample in SAMPLES {
        let whole = http::parse_response(sample, true).unwrap();
        hostile::truncations(sample, |_, prefix| {
            check_http(prefix);
            // While open, a prefix of a good response is never malformed, and
            // anything it returns is the whole body.
            match http::parse_response(prefix, false) {
                Ok(response) => assert_eq!(response, whole),
                Err(error) => assert_eq!(error, ParseError::Incomplete, "{prefix:?}"),
            }
        });
    }
}

#[test]
fn http_every_field_and_random_mutations() {
    for (n, sample) in SAMPLES.iter().enumerate() {
        hostile::sweep_fields(sample, 0..sample.len(), |_, mutated| check_http(mutated));
        for seed in [13, 0x94d0_49bb_1331_11eb] {
            hostile::fuzz(sample, seed + n as u64, 4000, check_http);
        }
    }
}

#[test]
fn http_hostile_lengths() {
    let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
    // Tens of thousands of one-byte chunks: each is read once.
    let many = format!("{head}{}0\r\n\r\n", "1\r\nx\r\n".repeat(40_000));
    assert_eq!(
        http::parse_response(many.as_bytes(), true).map(|r| r.body.len()),
        Ok(40_000)
    );
    // Signs, spaces and digits past any integer in the size fields.
    for size in ["-1", "+5", " ", "", "1".repeat(40).as_str(), "0x10"] {
        let raw = format!("{head}{size}\r\nhello\r\n0\r\n\r\n");
        check_http(raw.as_bytes());
    }
    for length in ["-1", "+3", "99999999999999999999999", "", "0x10", "3, 3"] {
        let raw = format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\nabc");
        check_http(raw.as_bytes());
    }
    // Two lengths that disagree, and a status line of nothing but the version.
    check_http(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 999\r\n\r\nabc");
    check_http(b"HTTP/1.1\r\n\r\n");
    check_http(b"\r\n\r\n");
}

fn completion_body(content: &str) -> String {
    format!(
        r#"{{"id":"x","choices":[{{"index":0,"message":{{"role":"assistant","content":{}}},"finish_reason":"stop"}}],"usage":{{"total_tokens":3}}}}"#,
        serde_json::to_string(content).unwrap()
    )
}

#[test]
fn openai_replies_every_prefix_and_mutation() {
    let body = completion_body("Hi \u{e9} [not] {nested} \"quoted\"");
    let whole = openai::parse_completion(body.as_bytes()).unwrap();
    assert_eq!(whole.text, "Hi \u{e9} [not] {nested} \"quoted\"");
    hostile::truncations(body.as_bytes(), |cut, prefix| {
        let result = openai::parse_completion(prefix);
        assert_eq!(result.is_ok(), cut == body.len(), "cut at {cut}");
    });
    for seed in [17, 0xbf58_476d_1ce4_e5b9] {
        hostile::fuzz(body.as_bytes(), seed, 4000, |mutated| {
            let (_, peak) = hostile::peak_alloc(|| openai::parse_completion(mutated));
            assert!(peak <= ALLOC_PER_BYTE * mutated.len() + ALLOC_FLOOR);
        });
    }
}

#[test]
fn openai_deep_nesting_is_refused_before_parsing() {
    let nested = |depth: usize| {
        let mut body = completion_body("ok");
        body.pop();
        body.push_str(&format!(
            r#","extra":{}{}}}"#,
            "[".repeat(depth),
            "]".repeat(depth)
        ));
        body
    };
    // The body itself nests one level, so `extra` may nest one less than the cap.
    assert!(openai::parse_completion(nested(MAX_JSON_DEPTH - 1).as_bytes()).is_ok());
    for depth in [MAX_JSON_DEPTH, 200, 100_000] {
        assert_eq!(
            openai::parse_completion(nested(depth).as_bytes()),
            Err(ReplyError::NotJson),
            "{depth}"
        );
    }
    // Brackets inside strings, escaped quotes included, are text.
    let text = format!("\\\"{}", "[{".repeat(1000));
    let reply = openai::parse_completion(completion_body(&text).as_bytes()).unwrap();
    assert_eq!(reply.text, text);
}

#[test]
fn tune_requests_and_completions() {
    let wire = tune::example().encode();
    hostile::fuzz(wire.as_bytes(), 19, 4000, |mutated| {
        let text = String::from_utf8_lossy(mutated);
        // Whatever decodes encodes back to a request that decodes the same.
        if let Some(request) = TuneRequest::decode(&text) {
            assert_eq!(TuneRequest::decode(&request.encode()), Some(request));
        }
    });
    hostile::truncations(wire.as_bytes(), |_, prefix| {
        let _ = TuneRequest::decode(&String::from_utf8_lossy(prefix));
    });
    for hostile_wire in [
        "tune ",
        "tune \n",
        "tune ,,,\nx",
        "tune a,,b\nx",
        "tune A\nx",
        "tune a\n \n\t",
        "tune \u{e9}\nx",
    ] {
        assert_eq!(TuneRequest::decode(hostile_wire), None, "{hostile_wire:?}");
    }
    let completion = Completion {
        text: String::from("{\"reason\":\"\u{e9}\"}"),
        cut_off: true,
    };
    let encoded = tune::encode_completion(&completion);
    hostile::fuzz(encoded.as_bytes(), 23, 4000, |mutated| {
        let text = String::from_utf8_lossy(mutated);
        if let Some(back) = tune::decode_completion(&text) {
            assert_eq!(tune::encode_completion(&back), text);
        }
    });
    for wire in ["", "?", "\u{e9}x", "=", "!"] {
        let _ = tune::decode_completion(wire);
    }
}

#[test]
fn replay_files() {
    let request = tune::example();
    let recorded = Completion {
        text: String::from("{\"reason\":\"a \\\\ b\nc\"}"),
        cut_off: false,
    };
    let file = replay::encode(&[(replay::prompt_hash(&request), recorded.clone())]);
    assert_eq!(replay::answer(file.as_bytes(), &request), Ok(recorded));
    hostile::truncations(file.as_bytes(), |_, prefix| {
        let _ = replay::answer(prefix, &request);
    });
    for seed in [29, 0x6a09_e667_f3bc_c909] {
        hostile::fuzz(file.as_bytes(), seed, 3000, |mutated| {
            let _ = replay::answer(mutated, &request);
        });
    }
    // Escapes cut in half, hashes too long, signs and a line of nothing.
    let hash = format!("{:016x}", replay::prompt_hash(&request));
    for line in [
        format!("{hash} =ab\\"),
        format!("{hash} =\\q"),
        hash.clone(),
        format!("{hash}{hash} =x"),
        format!("+{hash} =x"),
        String::from(" "),
    ] {
        let file = format!("{}{line}\n", replay::MAGIC);
        let _ = replay::answer(file.as_bytes(), &request);
    }
}

#[test]
fn traces_rebuild_or_drop_whatever_the_transcript_says() {
    let mut lines = replay::trace("request", 1, "hello, world");
    lines.extend(replay::trace("request", 2, &"\u{e9}x".repeat(200)));
    let transcript = lines.join("\n");
    let back = replay::untrace(transcript.lines(), "request");
    assert_eq!(back.len(), 2);
    for seed in [31, 0x3c6e_f372_fe94_f82b] {
        hostile::fuzz(transcript.as_bytes(), seed, 3000, |mutated| {
            let text = String::from_utf8_lossy(mutated);
            let (_, peak) = hostile::peak_alloc(|| replay::untrace(text.lines(), "request"));
            assert!(peak <= TRACE_ALLOC_PER_BYTE * mutated.len() + ALLOC_FLOOR);
        });
    }
}

#[test]
fn traces_that_promise_many_pieces_cost_only_their_bytes() {
    // Thousands of messages, each claiming 10,000 pieces and giving one: kept as
    // lines, not as 10,000 slots each.
    let transcript: String = (0..20_000)
        .map(|seq| format!("tune-trace request {seq} 1/10000 61\n"))
        .collect();
    let (back, peak) = hostile::peak_alloc(|| replay::untrace(transcript.lines(), "request"));
    assert!(back.is_empty());
    assert!(
        peak <= TRACE_ALLOC_PER_BYTE * transcript.len() + ALLOC_FLOOR,
        "{peak} bytes for {} bytes",
        transcript.len()
    );
    // Numbers past the count, zero, and counts past the limit are ignored.
    let odd = "tune-trace request 1 0/1 61\ntune-trace request 1 2/1 61\n\
               tune-trace request 2 1/10001 61\ntune-trace request 3 1/1 6\n\
               tune-trace request 4 1/1 zz\ntune-trace request 5 1/1 \u{e9}\n";
    assert!(replay::untrace(odd.lines(), "request").is_empty());
    // A repeated piece: the latest wins.
    let twice = "tune-trace request 1 1/1 61\ntune-trace request 1 1/1 62\n";
    assert_eq!(
        replay::untrace(twice.lines(), "request"),
        [(1, String::from("b"))]
    );
}

/// The `act` wire and the call reader (P1 task 6): whatever decodes encodes
/// back the same, every prefix and mutation either reads or is refused, and
/// a call is read in memory that follows its input.
#[test]
fn act_requests_and_calls_parse_or_fail() {
    use llmproto::act::{self, ActRequest};
    let request = ActRequest {
        line: String::from("stop this, then \u{e9}"),
        state: String::from("focused=build-3 here=/home"),
        timeout_ms: 90_000,
        read: None,
        apps: String::new(),
    };
    let wire = request.encode();
    hostile::fuzz(wire.as_bytes(), 29, 4000, |mutated| {
        let text = String::from_utf8_lossy(mutated);
        if let Some(back) = ActRequest::decode(&text) {
            assert_eq!(ActRequest::decode(&back.encode()), Some(back));
        }
    });
    hostile::truncations(wire.as_bytes(), |_, prefix| {
        let _ = ActRequest::decode(&String::from_utf8_lossy(prefix));
    });
    let answer = act::encode_answer(&act::parse(
        r#"{"name":"clarify","arguments":{"question":"Which?","options":["a","b"]}}"#,
    ));
    assert!(answer.starts_with("ok "));
    hostile::fuzz(answer.as_bytes(), 31, 4000, |mutated| {
        let text = String::from_utf8_lossy(mutated);
        let (call, peak) = hostile::peak_alloc(|| act::decode_answer(&text));
        assert!(peak <= 64 * text.len() + ALLOC_FLOOR, "{peak} bytes");
        if let Ok(call) = call {
            assert_eq!(act::parse(&call.text()), Ok(call));
        }
    });
    hostile::truncations(answer.as_bytes(), |_, prefix| {
        let _ = act::decode_answer(&String::from_utf8_lossy(prefix));
    });
    let deep = format!("ok {{\"name\":\"x\",\"arguments\":{}}}", "[".repeat(10_000));
    assert!(act::decode_answer(&deep).is_err());
}
