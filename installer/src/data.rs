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
/// The same arguments write the same bytes. An error names the step that
/// failed: `mkfs`, a folder's path, or `commit`.
pub fn format_data<D: BlockDev>(
    dev: D,
    uuid: [u8; 16],
    label: &str,
    now: u64,
) -> Result<u64, (&'static str, Error)> {
    let mut fs = Fs::mkfs(dev, uuid, label).map_err(|e| ("mkfs", e))?;
    fs.clock = now;
    for (vol, path) in FOLDERS {
        fs.set_volume(vol).map_err(|e| (path, e))?;
        fs.mkdir(ROOT, &path[1..]).map_err(|e| (path, e))?;
    }
    fs.commit().map_err(|e| ("commit", e))
}
