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
        Request::TrashList { after: None },
        Request::TrashList {
            after: Some(u64::MAX),
        },
        Request::TrashRestore {
            path: "notes/plan.txt".into(),
        },
        Request::EmptyTrash,
        Request::Abort,
        Request::Restore { txn: 5, action: 6 },
        Request::History,
        Request::Changes { since: 1 },
        Request::Sync,
        Request::Df,
        Request::ReadShm {
            off: u64::MAX,
            len: MAX_SHM_DATA as u32,
        },
        Request::WriteShm { off: 0, len: 1 },
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
        full_trash(),
        Reply::Emptied { count: u64::MAX },
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
        Reply::Copied { len: u64::MAX },
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

/// The largest [`Reply::Trash`] fsserve sends: its path text at the budget,
/// spread over the most items.
fn full_trash() -> Reply {
    let each = MAX_TRASH_TEXT / MAX_TRASH_ITEMS;
    Reply::Trash {
        items: (0..MAX_TRASH_ITEMS)
            .map(|_| TrashItem {
                id: u64::MAX,
                path: "p".repeat(each),
                kind: Kind::Dir,
                deleted: u64::MAX,
                action: u64::MAX,
            })
            .collect(),
        more: true,
    }
}

#[test]
fn one_path_at_the_whole_trash_budget_fits_one_message() {
    let reply = Reply::Trash {
        items: vec![TrashItem {
            id: u64::MAX,
            path: "p".repeat(MAX_TRASH_TEXT),
            kind: Kind::File,
            deleted: u64::MAX,
            action: u64::MAX,
        }],
        more: true,
    };
    assert!(encode_reply(&reply).len() <= MAX_MESSAGE);
}

/// The bulk requests are new variants of `kai-fs 1`: their numbers are fixed,
/// so an `fs` built before them refuses them as a bad request, and the client
/// can tell that from a refusal of the read itself.
#[test]
fn the_bulk_requests_keep_their_numbers() {
    let read = encode_request(&Request::ReadShm { off: 1, len: 2 });
    assert_eq!(read, [0x82, 0x01, 0x82, 0x18, 28, 0x82, 0x01, 0x02]);
    let write = encode_request(&Request::WriteShm { off: 1, len: 2 });
    assert_eq!(write, [0x82, 0x01, 0x82, 0x18, 29, 0x82, 0x01, 0x02]);
    let copied = encode_reply(&Reply::Copied { len: 3 });
    assert_eq!(copied, [0x82, 0x01, 0x82, 0x10, 0x81, 0x03]);
}

#[test]
fn only_a_dropped_action_ends_one_and_a_refusal_or_failure_does_not() {
    use crate::{DropCause, Error};
    for e in [
        Error::Dropped(DropCause::NoSpace),
        Error::Dropped(DropCause::Io),
    ] {
        assert!(e.ends_action(), "{e:?}");
    }
    // fs answers an in-action Io as Dropped(Io); a bare Io is a read's or a
    // change's outside any action, which leaves an open one where it was.
    for e in [
        Error::Io,
        Error::NoSpace,
        Error::Permission,
        Error::Exists,
        Error::Busy,
    ] {
        assert!(!e.ends_action(), "{e:?}");
    }
}

/// H3-7, H3-10: [`restored`] keeps a Restored reply within one message
/// whatever the names: one row that is not Left, then the Left rows.
#[test]
fn a_restored_reply_fits_one_message_whatever_the_names() {
    let beside = |i: u64| {
        (
            i,
            Outcome::Beside {
                object: u64::MAX,
                name: long_name('n'),
            },
        )
    };
    let mut rows: Vec<(u64, Outcome)> = (0..u64::from(MAX_ROWS)).map(beside).collect();
    rows.push((u64::MAX, Outcome::Left));
    let bytes = encode_reply(&restored(rows, u64::MAX));
    assert!(bytes.len() <= MAX_MESSAGE, "{} bytes", bytes.len());
    let Ok(Reply::Restored { rows, generation }) = decode_reply(&bytes) else {
        panic!("not a Restored reply");
    };
    assert_eq!(generation, u64::MAX);
    assert_eq!(rows[0], beside(0));
    assert_eq!(rows[1], (u64::MAX, Outcome::Left));
    assert!(
        rows.len() > 20 && rows.len() < MAX_ROWS as usize,
        "{}",
        rows.len()
    );
    // Short rows stop only at MAX_ROWS.
    let short = (0..100).map(|i| (i, Outcome::Removed)).collect();
    let Reply::Restored { rows, .. } = restored(short, 1) else {
        panic!("not a Restored reply");
    };
    assert_eq!(rows.len(), MAX_ROWS as usize);
}
