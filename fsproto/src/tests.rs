use super::*;
use alloc::vec;

fn stat() -> Stat {
    Stat {
        kind: Kind::File,
        object: u64::MAX >> 9,
        size: u64::MAX,
        mtime: u64::MAX,
        crtime: 1,
        changed: u64::MAX,
    }
}

fn long_name(c: char) -> String {
    core::iter::repeat_n(c, 255).collect()
}

#[test]
fn every_request_round_trips() {
    let requests = [
        Request::Open {
            path: "a/../b/c.txt".into(),
            rights: u32::MAX,
            how: How::CreateFile,
        },
        Request::Stat,
        Request::List {
            after: Some(Cursor {
                hash: 7,
                name: "x".into(),
            }),
            max: MAX_LIST,
        },
        Request::Read { off: 3, len: 9 },
        Request::Write {
            off: u64::MAX,
            data: vec![0xa5; MAX_DATA],
        },
        Request::Truncate { size: 0 },
        Request::Rename {
            from: "a".into(),
            to: "b".into(),
        },
        Request::Trash { name: "t".into() },
        Request::Begin { action: 42 },
        Request::Commit,
        Request::Generation,
        Request::Abort,
        Request::Restore { txn: 5, action: 6 },
        Request::History,
        Request::Changes { since: 1 },
        Request::Sync,
        Request::Df,
    ];
    for request in requests {
        let bytes = encode_request(&request);
        assert!(bytes.len() <= MAX_MESSAGE, "{request:?}");
        assert_eq!(decode_request(&bytes), Ok(request));
    }
}

#[test]
fn every_reply_round_trips_and_the_largest_fit_one_message() {
    let full_list = Reply::Listed {
        names: (0..MAX_LIST)
            .map(|i| Listing {
                name: long_name(char::from(b'a' + (i % 26) as u8)),
                kind: Kind::Dir,
                object: u64::MAX >> 9,
            })
            .collect(),
        more: Some(Cursor {
            hash: u64::MAX,
            name: long_name('z'),
        }),
    };
    let replies = [
        Reply::Done,
        Reply::Opened(stat(), u64::MAX),
        Reply::Stat(stat()),
        full_list,
        Reply::Data(vec![1; MAX_DATA]),
        Reply::Committed { generation: 9 },
        Reply::Generation { generation: 10 },
        Reply::Restored {
            rows: vec![
                (1, Outcome::Back),
                (2, Outcome::Moved { name: "m".into() }),
                (
                    3,
                    Outcome::Beside {
                        object: 4,
                        name: "n (restored)".into(),
                    },
                ),
                (5, Outcome::Removed),
                (6, Outcome::Left),
            ],
            generation: 3,
        },
        Reply::History(vec![
            Version {
                snapshot: 1,
                generation: 2,
                stat: stat(),
            };
            MAX_ROWS as usize
        ]),
        Reply::Changes {
            rows: vec![stat(); MAX_ROWS as usize],
            more: true,
        },
        Reply::Space { free: 1, total: 2 },
        Reply::Failed(Error::Busy),
    ];
    for reply in replies {
        let bytes = encode_reply(&reply);
        assert!(bytes.len() <= MAX_MESSAGE, "{} bytes", bytes.len());
        assert_eq!(decode_reply(&bytes), Ok(reply));
    }
}

#[test]
fn another_version_trailing_bytes_and_oversize_are_bad_requests() {
    let good = encode_request(&Request::Stat);
    let mut other = good.clone();
    other[1] = 2; // [2, ...]: kai-fs 2
    assert_eq!(decode_request(&other), Err(Error::BadRequest));
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(decode_request(&trailing), Err(Error::BadRequest));
    assert_eq!(decode_request(&[]), Err(Error::BadRequest));
    let big = Request::Write {
        off: 0,
        data: vec![0; MAX_MESSAGE],
    };
    assert_eq!(
        decode_request(&encode_request(&big)),
        Err(Error::BadRequest)
    );
}
