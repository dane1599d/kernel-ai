//! The fuzz targets' bodies for `kai-fs 1`'s wire (ADR 0004 §6; ADR 0000
//! P8): `fs` decodes whatever any process with a `Dir` or `File` handle sends,
//! and its clients decode `fs`'s replies. Each `fuzz_targets/*.rs` calls one
//! with libFuzzer's input, and `fsproto/tests/fuzz_harness.rs` includes this
//! file and runs each over hostile mutations of the samples here. Each must
//! return for any input; only a bug in the code under it may panic.

#![allow(dead_code)]

use fsproto::*;

/// One request of each common shape, encoded.
pub fn requests() -> Vec<Vec<u8>> {
    [
        Request::Open {
            path: "notes/plan.txt".into(),
            rights: 0x30,
            how: How::Existing,
        },
        Request::Write {
            off: 4096,
            data: b"buy milk".to_vec(),
        },
        Request::List {
            after: Some(Cursor {
                hash: 1 << 40,
                name: "b".into(),
            }),
            max: 8,
        },
        Request::Begin { action: 7 },
        Request::Restore { txn: 3, action: 8 },
        Request::SealedRemove {
            name: "ab.gguf".into(),
        },
        Request::Commit,
    ]
    .iter()
    .map(encode_request)
    .collect()
}

/// One reply of each common shape, encoded.
pub fn replies() -> Vec<Vec<u8>> {
    let stat = Stat {
        kind: Kind::Dir,
        object: 9,
        size: 4,
        mtime: 1,
        crtime: 2,
        changed: 3,
    };
    [
        Reply::Listed {
            names: vec![
                Listing {
                    name: "a".into(),
                    kind: Kind::File,
                    object: 5,
                };
                3
            ],
            more: None,
        },
        Reply::Restored {
            rows: vec![
                (
                    1,
                    Outcome::Beside {
                        object: 2,
                        name: "x (restored)".into(),
                    },
                ),
                (4, Outcome::Left { folder: true }),
                (5, Outcome::Removed),
            ],
            generation: 7,
        },
        Reply::History(vec![Version {
            snapshot: 1,
            generation: 2,
            stat,
        }]),
        Reply::Committed { generation: 9 },
        Reply::Failed(Error::Dropped(DropCause::NoSpace)),
    ]
    .iter()
    .map(encode_reply)
    .collect()
}

/// A request as `fs` decodes it: refused as BadRequest, or taken, and what it
/// takes encodes and decodes to itself.
pub fn request(bytes: &[u8]) {
    match decode_request(bytes) {
        Ok(request) => {
            let again = encode_request(&request);
            round_trip(&again, decode_request(&again), &request, encode_request);
        }
        Err(error) => assert_eq!(error, Error::BadRequest),
    }
}

/// A reply as a client decodes it, the same way.
pub fn reply(bytes: &[u8]) {
    match decode_reply(bytes) {
        Ok(reply) => {
            let again = encode_reply(&reply);
            round_trip(&again, decode_reply(&again), &reply, encode_reply);
        }
        Err(error) => assert_eq!(error, Error::BadRequest),
    }
}

/// `encoded`, the encoding of `value`, decodes to it again (`decoded`) and
/// encodes to the same bytes. An input may use longer forms than the encoder
/// (indefinite lengths), so one at the size limit may encode just over it:
/// that one is refused as too long, as it would be from any sender.
fn round_trip<T: PartialEq + core::fmt::Debug>(
    encoded: &[u8],
    decoded: Result<T, Error>,
    value: &T,
    encode: impl Fn(&T) -> Vec<u8>,
) {
    if encoded.len() > MAX_MESSAGE {
        assert_eq!(decoded, Err(Error::BadRequest));
        return;
    }
    let decoded = decoded.expect("an encoded message decodes");
    assert_eq!(&decoded, value);
    assert_eq!(encode(&decoded), encoded);
}

/// Each target's starting corpus: the samples.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let requests = requests().into_iter().map(|r| ("fsproto_request", r));
    requests
        .chain(replies().into_iter().map(|r| ("fsproto_reply", r)))
        .collect()
}
