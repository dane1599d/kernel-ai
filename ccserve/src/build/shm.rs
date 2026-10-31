//! `build <component> <version>` (`kai-cc 1`) and the `Shm` it comes with:
//! the shell writes the component's source bundle into it, and cc writes the
//! artifact over it. Each is a little-endian u64 length, then that many bytes.
//!
//! The shell makes the `Shm` [`PAGES`] long and cc maps that much. A shorter
//! one faults cc, which only the shell can call, and cc starts again on its
//! next request.

use super::{Build, BuildError, Kind};
use crate::{Budget, Compiler};
use alloc::format;
use alloc::string::String;
use kabi::live::MAX_ARTIFACT;
use livecore::artifact::{self, MAX_SOURCE};

/// Builds the bundle in `shm` as `component` at `version` and writes the
/// artifact over it. The reply's text: `<kind> <bytes> <provenance>`.
pub fn serve(
    cc: &Compiler,
    component: &str,
    version: &str,
    shm: &mut [u8],
    budget: &mut Budget,
) -> Result<String, BuildError> {
    let refuse = |why: &str| BuildError::Request(why.into());
    // A copy: the shell could write the `Shm` while cc reads it.
    let bundle = take(shm, MAX_SOURCE).map_err(refuse)?.to_vec();
    let files = artifact::source_files(&bundle).map_err(refuse)?;
    let kind = Kind::of(&files);
    let compiler = super::compiler(cc).map_err(|why| BuildError::Module(format!("cc: {why}")))?;
    let build = Build {
        name: component,
        version,
        kind,
        files: &files,
    };
    let built = super::build(&compiler, &build, budget)?;
    put(shm, &built.artifact).map_err(refuse)?;
    Ok(format!(
        "{} {} {}",
        kind.word(),
        built.artifact.len(),
        built.provenance()
    ))
}

/// The `Shm`'s pages: room for the largest artifact and its length.
pub const PAGES: usize = (MAX_ARTIFACT + 8).div_ceil(4096);
/// The `Shm`'s bytes.
pub const BYTES: usize = PAGES * 4096;

/// Writes `bytes`, with their length, at the start of `shm`.
pub fn put(shm: &mut [u8], bytes: &[u8]) -> Result<(), &'static str> {
    let end = bytes.len().checked_add(8).ok_or("too long")?;
    let room = shm.get_mut(..end).ok_or("the Shm is too small")?;
    room[..8].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
    room[8..].copy_from_slice(bytes);
    Ok(())
}

/// The bytes at the start of `shm`, if their length is at most `max` and fits.
pub fn take(shm: &[u8], max: usize) -> Result<&[u8], &'static str> {
    let len: [u8; 8] = shm
        .get(..8)
        .and_then(|b| b.try_into().ok())
        .ok_or("the Shm is too small")?;
    let len = u64::from_le_bytes(len);
    if len > max as u64 {
        return Err("the Shm holds more than a build takes");
    }
    let end = usize::try_from(len)
        .ok()
        .and_then(|len| len.checked_add(8))
        .ok_or("the Shm's length runs past its end")?;
    shm.get(8..end).ok_or("the Shm's length runs past its end")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn bytes_round_trip_and_bad_lengths_are_refused() {
        let mut shm = vec![0u8; 64];
        put(&mut shm, b"kai-source 1\n").unwrap();
        assert_eq!(take(&shm, 100), Ok(&b"kai-source 1\n"[..]));
        assert!(take(&shm, 5).is_err());
        assert!(put(&mut shm, &[1; 57]).is_err());
        shm[..8].copy_from_slice(&57u64.to_le_bytes());
        assert!(take(&shm, 1000).is_err());
        shm[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(take(&shm, usize::MAX).is_err());
        assert!(take(&shm[..7], 100).is_err());
        assert_eq!(PAGES, 257);
    }
}
