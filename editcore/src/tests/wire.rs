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
        }
    }
}
