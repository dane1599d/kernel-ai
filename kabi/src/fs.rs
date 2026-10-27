//! Files (ADR 0004 §6-7): what a `Dir` or `File` handle's badge holds. The `fs`
//! service mints both kinds on its own endpoint ([`crate::call::ENDPOINT_MINT`]);
//! the kernel checks kind and rights and passes the badge through, and `fs` reads
//! it. A badge names an object on disk, never server state, so a restarted `fs`
//! serves the badges the last one minted.
//!
//! Layout: bits 56-63 the volume, bit 55 set for a file, bits 40-54 the disk's
//! epoch, bits 0-39 the object id in that volume. Object ids are never reused on
//! one disk, and the epoch is drawn when `fs` formats a disk, so a stale badge,
//! from before a restore or from another disk, finds nothing, never another
//! object.

/// kaifs's volumes, as a badge holds them.
pub const SYSTEM: u8 = 1;
pub const HOME: u8 = 2;
pub const MODELS: u8 = 3;

const FILE_BIT: u64 = 1 << 55;
const EPOCH_SHIFT: u32 = 40;
/// The epoch's bits.
pub const EPOCH_MASK: u16 = 0x7fff;
/// The largest object id a badge holds.
pub const OBJECT_MAX: u64 = (1 << EPOCH_SHIFT) - 1;

/// What a `Dir` or `File` badge names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Badge {
    pub volume: u8,
    pub file: bool,
    /// The disk's epoch; 0 only in [`HOME_USER`].
    pub epoch: u16,
    pub object: u64,
}

impl Badge {
    pub const fn raw(self) -> u64 {
        ((self.volume as u64) << 56)
            | if self.file { FILE_BIT } else { 0 }
            | (((self.epoch & EPOCH_MASK) as u64) << EPOCH_SHIFT)
            | (self.object & OBJECT_MAX)
    }

    pub const fn parse(raw: u64) -> Badge {
        Badge {
            volume: (raw >> 56) as u8,
            file: raw & FILE_BIT != 0,
            epoch: ((raw >> EPOCH_SHIFT) as u16) & EPOCH_MASK,
            object: raw & OBJECT_MAX,
        }
    }
}

/// The badge of the `Dir` handle a process gets at startup with
/// [`crate::cap::FILES`]: `/home/user`, which `fs` looks up itself (object 0 is
/// never an object id), so the kernel needs neither an inode number nor the
/// disk's epoch.
pub const HOME_USER: u64 = Badge {
    volume: HOME,
    file: false,
    epoch: 0,
    object: 0,
}
.raw();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_badge_round_trips_and_home_user_names_no_object() {
        for badge in [
            Badge {
                volume: SYSTEM,
                file: true,
                epoch: 1,
                object: 1,
            },
            Badge {
                volume: HOME,
                file: false,
                epoch: EPOCH_MASK,
                object: OBJECT_MAX,
            },
            Badge {
                volume: MODELS,
                file: true,
                epoch: 77,
                object: 7,
            },
        ] {
            assert_eq!(Badge::parse(badge.raw()), badge);
        }
        assert_eq!(Badge::parse(HOME_USER).object, 0);
        assert_ne!(HOME_USER, 0, "0 is a plain route's badge");
    }
}
