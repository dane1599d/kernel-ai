//! Hostile `kai-fs 1` messages: `fs` decodes whatever any process with a `Dir` or
//! `File` handle sends, and its clients decode `fs`'s replies. Every valid
//! sample, cut at every byte, swept with extreme lengths and mutated thousands of
//! times, decodes or fails with BadRequest. None panics, and none allocates more
//! than a small multiple of its input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use fsproto::*;

const ALLOC_PER_BYTE: usize = 8;
const ALLOC_FLOOR: usize = 4 * 1024;

fn samples() -> Vec<Vec<u8>> {
    let stat = Stat {
        kind: Kind::Dir,
        object: 9,
        size: 4,
        mtime: 1,
        crtime: 2,
        changed: 3,
    };
    vec![
        encode_request(&Request::Open {
            path: "notes/plan.txt".into(),
            rights: 0x30,
            how: How::Existing,
        }),
        encode_request(&Request::Write {
            off: 4096,
            data: b"buy milk".to_vec(),
        }),
        encode_request(&Request::List {
            after: Some(Cursor {
                hash: 1 << 40,
                name: "b".into(),
            }),
            max: 8,
        }),
        encode_reply(&Reply::Listed {
            names: vec![
                Listing {
                    name: "a".into(),
                    kind: Kind::File,
                    object: 5,
                };
                3
            ],
            more: None,
        }),
        encode_reply(&Reply::Restored {
            rows: vec![(
                1,
                Outcome::Beside {
                    object: 2,
                    name: "x (restored)".into(),
                },
            )],
            generation: 7,
        }),
        encode_reply(&Reply::History(vec![Version {
            snapshot: 1,
            generation: 2,
            stat,
        }])),
    ]
}

fn check(bytes: &[u8]) {
    let (result, peak) = hostile::peak_alloc(|| (decode_request(bytes), decode_reply(bytes)));
    let cap = ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR;
    assert!(peak <= cap, "{peak} bytes allocated for {}", bytes.len());
    for error in [result.0.err(), result.1.err()].into_iter().flatten() {
        assert_eq!(error, Error::BadRequest);
    }
}

#[test]
fn cut_messages_decode_or_fail() {
    for sample in samples() {
        hostile::truncations(&sample, |_, cut| check(cut));
    }
}

#[test]
fn extreme_fields_decode_or_fail() {
    for sample in samples() {
        hostile::sweep_fields(&sample, 0..sample.len().min(48), |_, bytes| check(bytes));
    }
}

#[test]
fn mutated_messages_decode_or_fail() {
    for (seed, sample) in samples().iter().enumerate() {
        hostile::fuzz(sample, 0x6b61_6966_7331 + seed as u64, 4000, check);
    }
}

#[test]
fn huge_claimed_counts_do_not_allocate() {
    // [1, [3, [array of 2^32 - 1 listings...]]] with nothing behind the header.
    let bytes = [0x82, 0x01, 0x82, 0x03, 0x9a, 0xff, 0xff, 0xff, 0xff];
    check(&bytes);
    // A byte string claiming 2^63 bytes.
    let bytes = [0x82, 0x01, 0x82, 0x04, 0x5b, 0x80, 0, 0, 0, 0, 0, 0, 0];
    check(&bytes);
}
