//! A release as the update repository serves it over the network (P5 task 10;
//! ADR 0008 §9, §12): xtask's HTTPS mock as GitHub serves `kernel-ai-updates`.
//! The signed metadata is on the Pages host; `slot.img` and its sibling chunk
//! list `slot.img.chunks` are assets of release `os-<seq>`, named
//! `<sha256>.<target>` (consistent snapshots), behind the releases host's
//! redirect. The metadata is signed by the run's test root, the same one the
//! update disk's is (`update_disk.rs`), so the root a slot carries checks both.

use super::update_disk::{CHUNKS_TARGET, Stamp, TARGET, test_root};
use crate::mock_https::Content;
use kainet::chunks::{CHUNK, Chunks};

/// How the repository goes wrong for a case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// As published.
    None,
    /// One byte of the image the mock serves is flipped after the chunk list
    /// was signed over the right ones, so the chunk it falls in is refused.
    FlippedByte,
    /// The timestamp expired a day ago.
    Expired,
    /// Timestamp and snapshot at version 2: newer than [`Fault::None`]'s, which a
    /// device that trusted them then refuses to go back from.
    Newer,
    /// A timestamp at version 3 naming snapshot 1, older than [`Fault::Newer`]'s.
    RolledBack,
    /// The connection is cut once, inside chunk 3 of the image ([`CUT_AT`]): the
    /// three chunks before it are written and recorded, and the next run resumes.
    CutOnce,
}

/// Where [`Fault::CutOnce`] cuts: a page into chunk 3, so chunks 0 to 2 were
/// whole.
pub const CUT_AT: usize = 3 * CHUNK as usize + 4096;

impl Fault {
    /// The versions and lifetime of the timestamp and snapshot this fault serves.
    fn stamp(self) -> Stamp {
        match self {
            Fault::Expired => Stamp::expired(),
            Fault::Newer => Stamp::newer(),
            Fault::RolledBack => Stamp::rolled_back(),
            _ => Stamp::default(),
        }
    }
}

/// The GitHub Release a release's assets are under.
pub fn tag(release_seq: u32) -> String {
    format!("os-{release_seq}")
}

/// The path of `image` on the release-assets host, where a fault on the file is
/// set (`mock.set_fault`).
pub fn image_path(image: &[u8], release_seq: u32) -> String {
    format!(
        "/assets/{}/{}.{TARGET}",
        tag(release_seq),
        crate::sha256_hex(image)
    )
}

/// What the mock serves for `image` as release `release_seq`, with `fault`.
pub fn content(image: &[u8], release_seq: u32, fault: Fault) -> Result<Content, String> {
    let root = test_root()?;
    let list = Chunks::of(image, CHUNK).render().into_bytes();
    let tag = tag(release_seq);
    let files = root.repository(image, &list, release_seq, &tag, fault.stamp())?;
    let mut served = image.to_vec();
    if fault == Fault::FlippedByte {
        let middle = served.len() / 2;
        served[middle] ^= 0xFF;
    }
    let image_name = format!("{}.{TARGET}", crate::sha256_hex(image));
    let list_name = format!("{}.{CHUNKS_TARGET}", crate::sha256_hex(&list));
    Ok(Content::new()
        .repository(files)
        .release_asset(&tag, &image_name, served)
        .release_asset(&tag, &list_name, list))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_https::{ASSETS, REPO};

    fn bytes() -> Vec<u8> {
        (0..3 * CHUNK as usize + 12_345)
            .map(|i| (i * 7 % 251) as u8)
            .collect()
    }

    /// The first body the mock would serve at `host` and `path`.
    fn served(content: &Content, host: &str, path: &str) -> Option<Vec<u8>> {
        content.get(host, path).map(<[u8]>::to_vec)
    }

    #[test]
    fn the_release_is_the_metadata_the_image_and_its_chunk_list_under_their_hashes() {
        let image = bytes();
        let content = content(&image, 2, Fault::None).unwrap();
        for name in [
            "1.root.json",
            "1.targets.json",
            "1.snapshot.json",
            "timestamp.json",
        ] {
            assert!(
                served(&content, REPO, &format!("/metadata/{name}")).is_some(),
                "{name}"
            );
        }
        let asset = |name: &str| format!("/assets/os-2/{name}");
        let sha = crate::sha256_hex(&image);
        assert_eq!(
            served(&content, ASSETS, &asset(&format!("{sha}.slot.img"))),
            Some(image.clone())
        );
        let list = Chunks::of(&image, CHUNK).render().into_bytes();
        let list_sha = crate::sha256_hex(&list);
        assert_eq!(
            served(
                &content,
                ASSETS,
                &asset(&format!("{list_sha}.slot.img.chunks"))
            ),
            Some(list)
        );
    }

    #[test]
    fn a_flipped_byte_is_in_the_served_image_and_not_in_the_signed_list() {
        let image = bytes();
        let content = content(&image, 2, Fault::FlippedByte).unwrap();
        let sha = crate::sha256_hex(&image);
        let got = served(&content, ASSETS, &format!("/assets/os-2/{sha}.slot.img")).unwrap();
        let different: Vec<usize> = (0..image.len()).filter(|&i| got[i] != image[i]).collect();
        assert_eq!(different, [image.len() / 2]);
        // The list is the right image's: chunk 1 is where the flip falls.
        let list = Chunks::of(&image, CHUNK);
        assert_eq!(list.size, image.len() as u64);
    }

    #[test]
    fn an_expired_repository_has_the_same_files_with_a_timestamp_that_has_lapsed() {
        let image = bytes();
        let good = content(&image, 2, Fault::None).unwrap();
        let old = content(&image, 2, Fault::Expired).unwrap();
        let path = "/metadata/timestamp.json";
        assert_ne!(served(&good, REPO, path), served(&old, REPO, path));
        assert_eq!(
            served(&good, REPO, "/metadata/1.root.json"),
            served(&old, REPO, "/metadata/1.root.json")
        );
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;
    use crate::mock_https::REPO;

    fn version(content: &Content, path: &str) -> u64 {
        let bytes = content.get(REPO, path).expect(path);
        let json: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        json["signed"]["version"].as_u64().unwrap()
    }

    fn image() -> Vec<u8> {
        vec![7; 100]
    }

    #[test]
    fn newer_and_rolled_back_metadata_name_the_versions_a_device_that_trusted_newer_refuses() {
        let published = content(&image(), 2, Fault::None).unwrap();
        assert_eq!(version(&published, "/metadata/timestamp.json"), 1);
        assert_eq!(version(&published, "/metadata/1.snapshot.json"), 1);
        let newer = content(&image(), 2, Fault::Newer).unwrap();
        assert_eq!(version(&newer, "/metadata/timestamp.json"), 2);
        assert_eq!(version(&newer, "/metadata/2.snapshot.json"), 2);
        assert!(newer.get(REPO, "/metadata/1.snapshot.json").is_none());
        // Version 3 of the timestamp, naming the snapshot at version 1: older than
        // the 2 a device that took `Newer` holds.
        let old = content(&image(), 2, Fault::RolledBack).unwrap();
        assert_eq!(version(&old, "/metadata/timestamp.json"), 3);
        assert_eq!(version(&old, "/metadata/1.snapshot.json"), 1);
    }

    #[test]
    fn the_cut_lies_inside_chunk_three_and_the_image_path_is_where_the_mock_serves_it() {
        assert_eq!(CUT_AT / CHUNK as usize, 3);
        assert_eq!(CUT_AT % CHUNK as usize, 4096);
        let image = image();
        let content = content(&image, 2, Fault::CutOnce).unwrap();
        let path = image_path(&image, 2);
        assert_eq!(
            content.get(crate::mock_https::ASSETS, &path),
            Some(&image[..])
        );
    }
}
