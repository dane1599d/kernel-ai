//! The volume names ADR 0013 reserves for after the v0.1 tag (`memory` and `index`):
//! nothing may make a volume with one, and reading one is not refused (whether an
//! older slot mounts such a disk is for the format's compat bits, ADR 0004).

use kaifs::Error;
use kaifs::format::BlockPtr;
use kaifs::vol::{
    RESERVED_NAME, RESERVED_VOLUMES, Snap, VOLUMES, VolEntry, check_new_volume, decode_volume,
    encode_volume,
};

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
        assert_eq!(check_new_volume(name), Ok(()), "{name}");
    }
}

#[test]
fn making_a_volume_named_for_a_later_format_is_refused_by_name() {
    for name in RESERVED_VOLUMES {
        assert_eq!(
            check_new_volume(name),
            Err(Error::Unsupported(RESERVED_NAME)),
            "{name}"
        );
    }
    // Only the exact names: a volume may be made with a name that merely contains one.
    for name in ["memories", "index2", "my-index", "Memory", ""] {
        assert_eq!(check_new_volume(name), Ok(()), "{name:?}");
    }
}

#[test]
fn reading_an_entry_named_for_a_later_format_is_not_refused() {
    for name in RESERVED_VOLUMES {
        let back = decode_volume(9, &encode_volume(&entry(name))).unwrap();
        assert_eq!(back.name, name);
    }
}

#[test]
fn the_volumes_format_1_has_still_read_back() {
    for (id, name) in VOLUMES {
        let back = decode_volume(id, &encode_volume(&entry(name))).unwrap();
        assert_eq!(back.name, name);
    }
}
