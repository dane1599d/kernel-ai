//! A new `kai-data`: what a release image's and an install's both start with.

use kaifs::fs::{HOME, SYSTEM};
use kaifs::{BlockDev, Error, Fs, ROOT};

/// The folders every data disk starts with (P2 plan decision 5), by volume.
pub const FOLDERS: [(u64, &str); 5] = [
    (HOME, "/user"),
    (HOME, "/.trash"),
    (SYSTEM, "/settings"),
    (SYSTEM, "/state"),
    (SYSTEM, "/pkg"),
];

/// Formats kaifs over all of `dev` with its volumes `system`, `home` and
/// `models`, the [`FOLDERS`] and the clock at `now`; the generation it commits.
/// The same arguments write the same bytes.
pub fn format_data<D: BlockDev>(
    dev: D,
    uuid: [u8; 16],
    label: &str,
    now: u64,
) -> Result<u64, Error> {
    let mut fs = Fs::mkfs(dev, uuid, label)?;
    fs.clock = now;
    for (vol, path) in FOLDERS {
        fs.set_volume(vol)?;
        fs.mkdir(ROOT, &path[1..])?;
    }
    fs.commit()
}
