use super::*;
use crate::wire::*;
use kabi::live::Reason;
use livecore::uploads::Outcome;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn build() -> Vec<u8> {
    encode_build(&BuildRequest {
        component: "notes",
        label: "e1",
        base: "2",
        bundle: b"kai-source 1\nfile a.rs 3\nabc",
    })
}

fn built() -> Vec<u8> {
    encode_built(&Built {
        result: BuildResult::Ok,
        base: "2",
        commit: COMMIT,
        log: b"warning: x",
        artifact: b"\0asm",
    })
}

fn sync() -> SyncRequest<'static> {
    SyncRequest {
        component: "notes",
        label: "e2",
        outcome: Outcome::Rollback(Reason::Trap),
        generation: 1,
        source: HASH,
        commit: COMMIT,
    }
}

#[test]
fn every_format_round_trips() {
    let request = build();
    let decoded = decode_build(&request).unwrap();
    assert_eq!((decoded.label, decoded.base), ("e1", "2"));
    assert_eq!(encode_build(&decoded), request);
    let reply = built();
    assert_eq!(encode_built(&decode_built(&reply).unwrap()), reply);
    let failed = Built {
        result: BuildResult::Failed,
        base: "",
        commit: "",
        log: b"error[E0277]",
        artifact: &[],
    };
    assert_eq!(decode_built(&encode_built(&failed)), Ok(failed));
    let bytes = encode_sync(&sync());
    assert_eq!(decode_sync(&bytes), Ok(sync()));
    let boot = SyncRequest {
        source: "",
        commit: "",
        outcome: Outcome::Commit,
        ..sync()
    };
    let bytes = encode_sync(&boot);
    assert!(String::from_utf8_lossy(&bytes).contains("source -\ncommit -\n"));
    assert_eq!(decode_sync(&bytes), Ok(boot));
    for synced in [
        Synced::Moved {
            reference: "refs/heads/kai/notes",
            sha: COMMIT,
        },
        Synced::Recorded {
            reference: "refs/kai/rollbacks/notes/e2-0123456789ab",
            sha: COMMIT,
        },
        Synced::Untracked,
    ] {
        assert_eq!(decode_synced(&encode_synced(&synced)), Ok(synced));
    }
}

#[test]
fn ipc_lines_round_trip() {
    for message in [
        Ipc::Open {
            component: "notes",
            label: None,
        },
        Ipc::Open {
            component: "notes",
            label: Some("e1"),
        },
        Ipc::Cmd("s |a|b|"),
        Ipc::Cmd(""),
        Ipc::Sync {
            seq: 7,
            job: sync(),
        },
    ] {
        assert_eq!(decode_ipc(&encode_ipc(&message)), Ok(message));
    }
    for bad in [
        &b"open"[..],
        b"open Notes",
        b"open notes ../x",
        b"sync x\n",
        b"what",
        b"\xff",
    ] {
        assert!(decode_ipc(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn malformed_messages_are_refused() {
    let request = String::from_utf8(build()).unwrap();
    for bad in [
        request.replace("kai-build 1", "kai-build 2"),
        request.replace("label e1\nbase 2\n", "base 2\nlabel e1\n"),
        request.replace("bundle 28", "bundle 99"),
        request.replace("bundle 28", "bundle 27"),
        request.replace("label e1", "label E1"),
        request.replace("component notes", "component ../x"),
        request.clone() + "extra",
    ] {
        assert!(decode_build(bad.as_bytes()).is_err(), "{bad:?}");
    }
    let huge = encode_build(&BuildRequest {
        component: "notes",
        label: "e1",
        base: "2",
        bundle: &alloc::vec![b'x'; crate::MAX_BUNDLE + 1],
    });
    assert!(decode_build(&huge).is_err());
    let reply = String::from_utf8_lossy(&built()).into_owned();
    for bad in [
        reply.replace(COMMIT, "abc"),
        reply.replace("result ok", "result maybe"),
        reply.replace("log 10", "log 999999"),
    ] {
        assert!(decode_built(bad.as_bytes()).is_err(), "{bad:?}");
    }
    let sync = String::from_utf8(encode_sync(&sync())).unwrap();
    for bad in [
        sync.replace("rollback trap", "rollback sorry"),
        sync.replace(HASH, "123"),
        sync.replace("generation 1", "generation x"),
    ] {
        assert!(decode_sync(bad.as_bytes()).is_err(), "{bad:?}");
    }
    for bad in [
        "kai-synced 1\nresult moved refs/../x 0123456789abcdef0123456789abcdef01234567\n",
        "kai-synced 1\nresult moved heads/x 0123456789abcdef0123456789abcdef01234567\n",
        "kai-synced 1\nresult moved refs/x short\n",
        "kai-synced 1\nresult gone\n",
    ] {
        assert!(decode_synced(bad.as_bytes()).is_err(), "{bad:?}");
    }
}

#[test]
fn flipped_bytes_never_panic() {
    let samples = [
        build(),
        built(),
        encode_sync(&sync()),
        encode_ipc(&Ipc::Sync {
            seq: 1,
            job: sync(),
        }),
    ];
    let mut seed: u32 = 99;
    for sample in &samples {
        for _ in 0..400 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let mut bytes = sample.clone();
            let at = (seed as usize >> 8) % bytes.len();
            bytes[at] ^= (seed >> 24) as u8 | 1;
            if seed.is_multiple_of(3) {
                bytes.truncate(at);
            }
            let _ = decode_build(&bytes);
            let _ = decode_built(&bytes);
            let _ = decode_sync(&bytes);
            let _ = decode_synced(&bytes);
            let _ = decode_ipc(&bytes);
            let _ = http_body(&bytes);
        }
    }
}

#[test]
fn acknowledgements_name_the_job_and_a_commit() {
    let synced = format!("ok synced 4 {COMMIT}");
    assert_eq!(parse_ack(synced.as_bytes(), 4), Ok(COMMIT.into()));
    assert!(parse_ack(synced.as_bytes(), 5).is_err());
    assert_eq!(parse_ack(b"ok untracked 4", 4), Ok(String::new()));
    assert!(parse_ack(b"ok synced 4 abc", 4).is_err());
    assert!(parse_ack(b"err build service: HTTP 403", 4).is_err());
}

fn http(body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

#[test]
fn an_upload_must_be_the_build_reply_exactly() {
    let reply = http(&built());
    assert_eq!(check_built_response(&reply, b"\0asm", "2", COMMIT), Ok(()));
    assert!(check_built_response(&reply, b"\0asm2", "2", COMMIT).is_err());
    assert!(check_built_response(&reply, b"\0as", "2", COMMIT).is_err());
    let other = "1111111111111111111111111111111111111111";
    assert!(check_built_response(&reply, b"\0asm", "2", other).is_err());
    let failed = encode_built(&Built {
        result: BuildResult::Failed,
        base: "",
        commit: "",
        log: b"x",
        artifact: &[],
    });
    assert!(check_built_response(&http(&failed), b"", "", "").is_err());
    // The body must be exactly what the length says: no forged tail.
    let mut padded = reply.clone();
    padded.extend_from_slice(b"\0asm");
    assert!(check_built_response(&padded, b"\0asm", "2", COMMIT).is_err());
    let not_ok = String::from_utf8_lossy(&reply).replacen("200 OK", "403 Forbidden", 1);
    assert!(check_built_response(not_ok.as_bytes(), b"\0asm", "2", COMMIT).is_err());
    let chunked = String::from_utf8_lossy(&reply).replacen(
        "Content-Type: x",
        "Transfer-Encoding: chunked",
        1,
    );
    assert!(check_built_response(chunked.as_bytes(), b"\0asm", "2", COMMIT).is_err());
}

#[test]
fn an_upload_must_name_the_base_it_was_built_from() {
    let reply = http(&built());
    assert!(check_built_response(&reply, b"\0asm", "e1", COMMIT).is_err());
}
