//! The volume names ADR 0013 reserves for after the v0.1 tag (`memory` and `index`):
//! `mkfs` makes neither, and an entry that names one is refused.

use kaifs::Error;
use kaifs::format::BlockPtr;
use kaifs::vol::{RESERVED_VOLUMES, Snap, VOLUMES, VolEntry, decode_volume, encode_volume};

fn entry(name: &str) -> VolEntry {
    VolEntry {
        live: Snap::named(BlockPtr::NULL, 1, 2),
        name: name.into(),
        key_id: 0,
    }
}

#[test]
fn mkfs_makes_none_of_the_reserved_volumes() {
    for (_, name) in VOLUMES {
        assert!(!RESERVED_VOLUMES.contains(&name), "{name}");
    }
}

#[test]
fn an_entry_named_for_a_later_format_is_refused() {
    for name in RESERVED_VOLUMES {
        let written = encode_volume(&entry(name));
        assert!(
            matches!(decode_volume(9, &written), Err(Error::Unsupported(_))),
            "{name}"
        );
    }
}

#[test]
fn the_volumes_format_1_has_still_read_back() {
    for (id, name) in VOLUMES {
        let back = decode_volume(id, &encode_volume(&entry(name))).unwrap();
        assert_eq!(back.name, name);
    }
}
