//! Hostile Identify pages (ADR 0007 section 12): the controller hands the
//! kernel these 4 KiB pages, and a broken or malicious one may put anything
//! in them. Every prefix, every field read set to every extreme value, and
//! seeded mutations of an Identify Controller, an Identify Namespace and an
//! active-namespace list: a value or an error, never a panic, values that stay
//! in their ranges, and an allocation bounded by the page.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use nvme::identify::{IDENTIFY_BYTES, active_namespaces};
use nvme::{Controller, Namespace};

const MUTATIONS: usize = 4_000;
/// Nothing here should allocate at all; a page's worth is the bound.
const ALLOC_BOUND: usize = IDENTIFY_BYTES;
/// The largest transfer cap the kernel passes.
const CAP: u64 = 1 << 20;

/// QEMU's controller page, as far as it is read.
fn controller_page() -> Vec<u8> {
    let mut id = vec![0u8; IDENTIFY_BYTES];
    id[4..24].copy_from_slice(b"kai-model           ");
    id[24..64].copy_from_slice(b"QEMU NVMe Ctrl                          ");
    id[77] = 7;
    id[516..520].copy_from_slice(&256u32.to_le_bytes());
    id[525] = 1;
    id[528..530].copy_from_slice(&7u16.to_le_bytes());
    id
}

/// A namespace of 6,145 blocks with two formats, the second (4 KiB) in use.
fn namespace_page() -> Vec<u8> {
    let mut id = vec![0u8; IDENTIFY_BYTES];
    id[0..8].copy_from_slice(&6145u64.to_le_bytes());
    id[25] = 1;
    id[26] = 1;
    id[128..132].copy_from_slice(&(9u32 << 16).to_le_bytes());
    id[132..136].copy_from_slice(&(12u32 << 16).to_le_bytes());
    id
}

/// Namespaces 1 to 3, then the end.
fn list_page() -> Vec<u8> {
    let mut id = vec![0u8; IDENTIFY_BYTES];
    for (i, nsid) in [1u32, 2, 3].into_iter().enumerate() {
        id[4 * i..4 * i + 4].copy_from_slice(&nsid.to_le_bytes());
    }
    id
}

fn check_controller(page: &[u8]) {
    let (result, peak) = hostile::peak_alloc(|| Controller::parse(page));
    assert!(peak <= ALLOC_BOUND, "{peak} bytes for a controller page");
    if let Ok(controller) = result {
        assert!(controller.serial().len() <= 20);
        assert!(controller.model().len() <= 40);
        assert!(controller.atomic_blocks >= 1);
        for page_shift in [12, 16, 27] {
            assert!(controller.max_transfer(page_shift, CAP) <= CAP);
        }
    }
}

fn check_namespace(page: &[u8]) {
    let (result, peak) = hostile::peak_alloc(|| Namespace::parse(page));
    assert!(peak <= ALLOC_BOUND, "{peak} bytes for a namespace page");
    if let Ok(namespace) = result {
        assert!(namespace.block_bytes.is_power_of_two());
        assert!((512..=65536).contains(&namespace.block_bytes));
    }
}

fn check_list(page: &[u8]) {
    let (count, peak) = hostile::peak_alloc(|| active_namespaces(page).count());
    assert!(peak <= ALLOC_BOUND, "{peak} bytes for a namespace list");
    assert!(count <= IDENTIFY_BYTES / 4);
}

#[test]
fn the_samples_parse_as_built() {
    let controller = Controller::parse(&controller_page()).unwrap();
    assert_eq!(controller.model(), b"QEMU NVMe Ctrl");
    let namespace = Namespace::parse(&namespace_page()).unwrap();
    assert_eq!((namespace.blocks, namespace.block_bytes), (6145, 4096));
    assert_eq!(
        active_namespaces(&list_page()).collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn every_prefix_is_a_value_or_an_error() {
    hostile::truncations(&controller_page(), |cut, page| {
        check_controller(page);
        assert_eq!(Controller::parse(page).is_ok(), cut == IDENTIFY_BYTES);
    });
    hostile::truncations(&namespace_page(), |cut, page| {
        check_namespace(page);
        assert_eq!(Namespace::parse(page).is_ok(), cut == IDENTIFY_BYTES);
    });
    hostile::truncations(&list_page(), |_, page| check_list(page));
}

#[test]
fn every_field_read_at_every_extreme_stays_in_range() {
    // The controller page is read up to byte 530, the namespace's up to 384
    // (64 formats of 4 bytes from 128), the list everywhere.
    hostile::sweep_fields(&controller_page(), 0..536, |_, page| check_controller(page));
    hostile::sweep_fields(&namespace_page(), 0..392, |_, page| check_namespace(page));
    hostile::sweep_fields(&list_page(), 0..64, |_, page| check_list(page));
}

#[test]
fn seeded_mutations_stay_in_range() {
    hostile::fuzz(&controller_page(), 0x4E56, MUTATIONS, check_controller);
    hostile::fuzz(&namespace_page(), 0x4E57, MUTATIONS, check_namespace);
    hostile::fuzz(&list_page(), 0x4E58, MUTATIONS, check_list);
}

#[test]
fn every_format_index_and_size_byte() {
    let sample = namespace_page();
    for formats in [0u8, 1, 15, 16, 63, 255] {
        for flbas in 0..=255u8 {
            let mut page = sample.clone();
            page[25] = formats;
            page[26] = flbas;
            check_namespace(&page);
        }
    }
}
