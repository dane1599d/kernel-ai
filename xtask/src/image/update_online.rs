//! A release as the update repository serves it over the network (P5 task 10;
//! ADR 0008 §9, §12): xtask's HTTPS mock as GitHub serves `kernel-ai-updates`.
//! The signed metadata is on the Pages host; `slot.img` and its sibling chunk
//! list `slot.img.chunks` are assets of release `os-<seq>`, named
//! `<sha256>.<target>` (consistent snapshots), behind the releases host's
//! redirect. The metadata is signed by the run's test root, the same one the
//! update disk's is (`update_disk.rs`), so the root a slot carries checks both.

use super::update_disk::{CHUNKS_TARGET, TARGET, test_root};
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
}

/// The GitHub Release a release's assets are under.
pub fn tag(release_seq: u32) -> String {
    format!("os-{release_seq}")
}

/// What the mock serves for `image` as release `release_seq`, with `fault`.
pub fn content(image: &[u8], release_seq: u32, fault: Fault) -> Result<Content, String> {
    let root = test_root()?;
    let list = Chunks::of(image, CHUNK).render().into_bytes();
    let tag = tag(release_seq);
    let files = match fault {
        Fault::Expired => root.repository_expired(image, &list, release_seq, &tag)?,
        _ => root.repository(image, &list, release_seq, &tag)?,
    };
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
