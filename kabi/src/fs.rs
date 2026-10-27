//! Files (ADR 0004 §6-7): what a `Dir` or `File` handle's badge holds. The `fs`
//! service mints both kinds on its own endpoint ([`crate::call::ENDPOINT_MINT`]);
//! the kernel checks kind and rights and passes the badge through, and `fs` reads
//! it. A badge names an object on disk, never server state, so a restarted `fs`
//! serves the badges the last one minted.
//!
//! Layout: bits 56-63 the volume, bit 55 set for a file, bits 0-54 the object id
//! in that volume. Object ids are never reused, so a stale badge finds nothing,
//! never another object.

/// kaifs's volumes, as a badge holds them.
pub const SYSTEM: u8 = 1;
pub const HOME: u8 = 2;
pub const MODELS: u8 = 3;

const FILE_BIT: u64 = 1 << 55;
const OBJECT_MASK: u64 = FILE_BIT - 1;

/// The badge of the `Dir` handle a process gets at startup with
/// [`crate::cap::FILES`]: `/home/user`, which `fs` looks up itself (object 0 is
/// never an object id), so the kernel needs no inode number.
pub const HOME_USER: u64 = badge(HOME, false, 0);

/// The badge for object `object` of `volume`, a file or a folder.
pub const fn badge(volume: u8, file: bool, object: u64) -> u64 {
    ((volume as u64) << 56) | if file { FILE_BIT } else { 0 } | (object & OBJECT_MASK)
}

/// A badge's volume, whether it names a file, and its object id.
pub const fn parse(badge: u64) -> (u8, bool, u64) {
    (
        (badge >> 56) as u8,
        badge & FILE_BIT != 0,
        badge & OBJECT_MASK,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_badge_round_trips_and_home_user_names_no_object() {
        for (volume, file, object) in [
            (SYSTEM, true, 1),
            (HOME, false, OBJECT_MASK),
            (MODELS, true, 7),
        ] {
            assert_eq!(parse(badge(volume, file, object)), (volume, file, object));
        }
        assert_eq!(parse(HOME_USER), (HOME, false, 0));
        assert_ne!(HOME_USER, 0, "0 is a plain route's badge");
    }
}
