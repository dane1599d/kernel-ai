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

/// The badge of the `Dir` handle [`crate::cap::SYSTEM_STATE`] gives at startup:
/// `/system/state`, which `fs` looks up itself, as for [`HOME_USER`].
pub const SYSTEM_STATE: u64 = Badge {
    volume: SYSTEM,
    file: false,
    epoch: 0,
    object: 0,
}
.raw();

/// The badge of the startup `Dir` a plan line's `dir=pkg` grants (ADR 0011 §12):
/// `/system/pkg`, which `fs` looks up itself. Epoch 0 marks it fixed, as for
/// [`HOME_USER`] and [`SYSTEM_STATE`]; object 1 tells it from `/system/state`.
pub const SYSTEM_PKG: u64 = Badge {
    volume: SYSTEM,
    file: false,
    epoch: 0,
    object: 1,
}
.raw();

/// The badges of the startup `Dir`s `updated`'s plan line grants (ADR 0008
/// §7, §10): `dir=trust` gives `/system/trust`, the cached TUF metadata, and
/// `dir=updates` gives `/system/updates`, the staged releases and the handle
/// the `update` snapshot is taken on. Fixed, as [`SYSTEM_PKG`] is.
pub const SYSTEM_TRUST: u64 = Badge {
    volume: SYSTEM,
    file: false,
    epoch: 0,
    object: 2,
}
.raw();
pub const SYSTEM_UPDATES: u64 = Badge {
    volume: SYSTEM,
    file: false,
    epoch: 0,
    object: 3,
}
.raw();
/// The badge of the startup `Dir` a plan line's `dir=apps` grants: `/system/apps`,
/// where comphost keeps each app's saved state in a folder named for the app
/// (P4, app storage that survives a reboot). Fixed, as [`SYSTEM_PKG`]; object 4
/// (2 and 3 are `/system/trust` and `/system/updates`, #288).
pub const SYSTEM_APPS: u64 = Badge {
    volume: SYSTEM,
    file: false,
    epoch: 0,
    object: 4,
}
.raw();

/// The badge of the startup `Dir` a plan line's `dir=settings` grants:
/// `/system/settings`, the Settings app's files (ADR 0004 §5), for a service
/// that reads a setting. The kernel gives it READ and ENUMERATE only, and `fs`
/// refuses any change through it. Fixed, as [`SYSTEM_PKG`] is; object 4 is
/// lane C's `SYSTEM_APPS` (#350).
pub const SYSTEM_SETTINGS: u64 = Badge {
    volume: SYSTEM,
    file: false,
    epoch: 0,
    object: 5,
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
        // The fixed badges are three, and none is another's.
        assert_eq!(Badge::parse(SYSTEM_PKG).epoch, 0);
        const { assert!(SYSTEM_PKG != SYSTEM_STATE && SYSTEM_PKG != HOME_USER) };
        let fixed = [
            HOME_USER,
            SYSTEM_STATE,
            SYSTEM_PKG,
            SYSTEM_TRUST,
            SYSTEM_UPDATES,
            SYSTEM_SETTINGS,
            SYSTEM_APPS,
        ];
        for (i, a) in fixed.iter().enumerate() {
            assert_eq!(Badge::parse(*a).epoch, 0);
            assert!(fixed[i + 1..].iter().all(|b| b != a));
        }
        assert_ne!(SYSTEM_STATE, HOME_USER);
        assert_eq!(Badge::parse(SYSTEM_STATE).volume, SYSTEM);
    }
}
