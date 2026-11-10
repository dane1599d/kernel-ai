//! What the HTTPS mock serves `net-download` (P5 task 6's case, `dlcheck`):
//! the targets, made at test time from fixed seeds and never committed, their
//! `kai-chunks 1` lists, and two job lists, pinned in `netcore::fixture`.
//!
//! Every file is an asset of release `net-download`, downloaded from the
//! releases host through GitHub's redirect to the assets host, as `pkg` and
//! `updated` reach theirs. The certificate cases' targets are on the three
//! other hosts, each given a bad certificate for the boot.

use crate::mock_https::{ASSETS, Content, Fault, MODELS, OFF, RELEASES, REPO};
use kainet::chunks::{CHUNK, Chunks, MIN_CHUNK};
use netcore::fixture::{DOWNLOAD_JOBS, DOWNLOAD_JOBS_SMALL, DOWNLOAD_RELEASE};
use sha2::{Digest, Sha256};

/// The release the files are assets of.
const TAG: &str = "net-download";
/// Chunks in the target: the cuts at 40% and 80% fall in chunks 12 and 25,
/// and the reboot inside chunk 16, whatever the chunk's size.
const CHUNKS: usize = 32;
/// The KVM boot's target: 32 chunks of 2 MiB, 64 MiB.
const BLOB: usize = CHUNKS * CHUNK as usize;
/// The TCG boot's: the same 32 chunks at 64 KiB.
const BLOB_SMALL: usize = CHUNKS * MIN_CHUNK as usize;
/// The other jobs' targets: 4 chunks of 64 KiB.
const LITTLE: usize = 4 * MIN_CHUNK as usize;
/// `flipped`'s flipped byte, in its chunk 2.
const FLIPPED_AT: usize = 2 * MIN_CHUNK as usize + 17;
/// Where the certificate jobs' targets are, on hosts with a bad certificate.
const CERT_PATH: &str = "/net-download/cert";
/// The assets host's paths of the targets the faults are on.
const BLOB_ASSET: &str = "/assets/net-download/blob";
const BLOB_SMALL_ASSET: &str = "/assets/net-download/blob-small";
const FLIPPED_ASSET: &str = "/assets/net-download/flipped";

/// `kai-data`'s size for the case: the 64 MiB target, the others, and room.
pub const DATA_MIB: u64 = 192;

/// Boot 1's faults: the cut at 40%, the flipped byte, and the three bad
/// certificates.
pub const FIRST_FAULTS: &[(&str, &str, Fault)] = &[
    (ASSETS, BLOB_ASSET, Fault::CutsAt(&[40])),
    (ASSETS, BLOB_SMALL_ASSET, Fault::CutsAt(&[40])),
    (ASSETS, FLIPPED_ASSET, Fault::Flip(FLIPPED_AT)),
    (REPO, CERT_PATH, Fault::Expired),
    (MODELS, CERT_PATH, Fault::WrongName),
    (OFF, CERT_PATH, Fault::UnknownCa),
];

/// Boot 2's: the cut at 80%.
pub const SECOND_FAULTS: &[(&str, &str, Fault)] = &[
    (ASSETS, BLOB_ASSET, Fault::CutsAt(&[80])),
    (ASSETS, BLOB_SMALL_ASSET, Fault::CutsAt(&[80])),
];

/// `len` bytes of xorshift64 from `seed`: the same on every run.
fn seeded(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

/// The URL `dlcheck` downloads asset `name` from.
fn release_url(name: &str) -> String {
    format!("https://{RELEASES}{DOWNLOAD_RELEASE}{name}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The chunk lists, by name: rendered text.
fn lists(blob: &[u8], little: &[u8]) -> Vec<(&'static str, String)> {
    let mut wrong = Chunks::of(little, MIN_CHUNK);
    wrong.blob[0] ^= 1;
    vec![
        ("blob", Chunks::of(blob, CHUNK).render()),
        (
            "blob-small",
            Chunks::of(&blob[..BLOB_SMALL], MIN_CHUNK).render(),
        ),
        ("flipped", Chunks::of(little, MIN_CHUNK).render()),
        ("whole", wrong.render()),
        ("cert", Chunks::of(little, MIN_CHUNK).render()),
    ]
}

/// A job list (`dlcheck`'s `kai-dl-jobs 1`) whose `blob` job is the target
/// `blob` names.
fn jobs(lists: &[(&str, String)], blob: &str) -> String {
    let sha = |name: &str| {
        let (_, text) = lists
            .iter()
            .find(|(list, _)| *list == name)
            .expect("every job's list is made");
        hex(&Sha256::digest(text.as_bytes()))
    };
    let line = |job: &str, list: &str, target: String| {
        let url = release_url(&format!("{list}.chunks"));
        format!("{job} {url} {} {target}\n", sha(list))
    };
    let mut text = String::from("kai-dl-jobs 1\n");
    text += &line("blob", blob, release_url(blob));
    text += &line("flipped", "flipped", release_url("flipped"));
    text += &line("whole", "whole", release_url("whole"));
    for (job, host) in [
        ("expired", REPO),
        ("wrong-name", MODELS),
        ("unknown-ca", OFF),
    ] {
        text += &line(job, "cert", format!("https://{host}{CERT_PATH}"));
    }
    text
}

/// Both job lists: the KVM boot's and the TCG boot's.
#[cfg(test)]
fn job_lists() -> (String, String) {
    let blob = seeded(BLOB, 0x6b61_692d_646c_0001);
    let little = seeded(LITTLE, 0x6b61_692d_646c_0002);
    let lists = lists(&blob, &little);
    (jobs(&lists, "blob"), jobs(&lists, "blob-small"))
}

/// Everything the mock serves the case.
pub fn content() -> Content {
    let blob = seeded(BLOB, 0x6b61_692d_646c_0001);
    let little = seeded(LITTLE, 0x6b61_692d_646c_0002);
    let lists = lists(&blob, &little);
    let (big, small) = (jobs(&lists, "blob"), jobs(&lists, "blob-small"));
    let mut content = Content::new()
        .release_asset(TAG, "blob-small", blob[..BLOB_SMALL].to_vec())
        .release_asset(TAG, "blob", blob)
        .release_asset(TAG, "flipped", little.clone())
        .release_asset(TAG, "whole", little.clone())
        .release_asset(TAG, DOWNLOAD_JOBS, big.into_bytes())
        .release_asset(TAG, DOWNLOAD_JOBS_SMALL, small.into_bytes());
    for host in [REPO, MODELS, OFF] {
        content = content.file(host, CERT_PATH, little.clone());
    }
    for (name, text) in lists {
        content = content.release_asset(TAG, &format!("{name}.chunks"), text.into_bytes());
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use netcore::fixture::{DOWNLOAD_JOBS_SHA256, DOWNLOAD_JOBS_SMALL_SHA256};

    /// The pins the kernel hands `dlcheck` are the job lists the mock serves.
    /// After a change to what is served, set them to what this prints.
    #[test]
    fn the_fixture_pins_the_job_lists() {
        let (big, small) = job_lists();
        let (big, small) = (
            hex(&Sha256::digest(big.as_bytes())),
            hex(&Sha256::digest(small.as_bytes())),
        );
        assert_eq!(
            (DOWNLOAD_JOBS_SHA256, DOWNLOAD_JOBS_SMALL_SHA256),
            (big.as_str(), small.as_str()),
            "netcore::fixture's DOWNLOAD_JOBS_SHA256 and DOWNLOAD_JOBS_SMALL_SHA256"
        );
    }

    /// The release URL `dlcheck` gets is where `Content::release_asset` puts
    /// an asset of [`TAG`].
    #[test]
    fn the_release_path_is_the_mocks() {
        assert_eq!(
            DOWNLOAD_RELEASE,
            format!("/dane1599d/kernel-ai-updates/releases/download/{TAG}/")
        );
        assert_eq!(BLOB_ASSET, format!("/assets/{TAG}/blob"));
        assert_eq!(BLOB_SMALL_ASSET, format!("/assets/{TAG}/blob-small"));
        assert_eq!(FLIPPED_ASSET, format!("/assets/{TAG}/flipped"));
    }

    /// Each job's list names its target, chunk by chunk; `whole`'s alone is
    /// wrong, and only in its whole-file hash.
    #[test]
    fn every_list_fits_its_target_but_whole() {
        let blob = seeded(BLOB, 0x6b61_692d_646c_0001);
        let little = seeded(LITTLE, 0x6b61_692d_646c_0002);
        for (name, text) in lists(&blob, &little) {
            let list = Chunks::parse(text.as_bytes()).unwrap();
            assert_eq!(
                list.count(),
                if name.starts_with("blob") { CHUNKS } else { 4 }
            );
            let right = match name {
                "blob" => Chunks::of(&blob, CHUNK),
                "blob-small" => Chunks::of(&blob[..BLOB_SMALL], MIN_CHUNK),
                _ => Chunks::of(&little, MIN_CHUNK),
            };
            assert_eq!(list.hashes, right.hashes, "{name}");
            assert_eq!(list.blob == right.blob, name != "whole", "{name}");
        }
    }

    /// The cuts and the reboot fall in the chunks the case's lines name, at
    /// either size.
    #[test]
    fn the_cuts_fall_in_chunks_12_and_25() {
        for (size, chunk) in [(BLOB, CHUNK as usize), (BLOB_SMALL, MIN_CHUNK as usize)] {
            assert_eq!(size * 40 / 100 / chunk, 12);
            assert_eq!(size * 80 / 100 / chunk, 25);
            assert_eq!(CHUNKS / 2, 16);
        }
        assert_eq!(FLIPPED_AT / MIN_CHUNK as usize, 2);
    }
}
