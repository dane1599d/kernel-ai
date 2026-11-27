//! JPEG: tables, frame headers, lying lengths and cuts (see `crafted`).

use super::crafted::*;
use super::samples;
use kaimedia::Format;

/// The offset of the first segment with marker `FF marker` in `bytes`.
fn segment(bytes: &[u8], marker: u8) -> usize {
    bytes
        .windows(2)
        .position(|m| m[0] == 0xFF && m[1] == marker)
        .unwrap_or_else(|| panic!("no FF{marker:02X} segment"))
}

/// Where each segment before the scan starts, and how long it says it is.
fn jpeg_segments(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 2;
    while at + 4 <= bytes.len() && bytes[at] == 0xFF && bytes[at + 1] != 0xDA {
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        out.push((at, length));
        at += 2 + length;
    }
    out
}

fn jpeg_settles(name: &str, bytes: &[u8]) -> bool {
    settles(name, bytes, Format::Jpeg, samples::W, samples::H)
}

#[test]
fn a_jpeg_of_no_size_is_refused() {
    for (w, h) in [(0, 0), (0, 8), (8, 0)] {
        refused(
            &format!("jpeg {w}x{h}"),
            &samples::jpeg_claiming(w, h),
            Format::Jpeg,
        );
    }
}

#[test]
fn a_jpeg_with_huffman_tables_that_cannot_be_right_settles() {
    let good = samples::jpeg();
    let dht = segment(&good, 0xC4);
    let length = u16::from_be_bytes([good[dht + 2], good[dht + 3]]) as usize;
    // FF C4, length (2), class and id (1), 16 code counts, then the symbols.
    let counts = dht + 5..dht + 21;
    let body_end = dht + 2 + length;
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    for (what, fill) in [("all 0xFF", 0xFFu8), ("all zero", 0), ("all one", 1)] {
        let mut bytes = good.clone();
        bytes[counts.clone()].fill(fill);
        cases.push((what, bytes));
    }
    // Every code count at its extreme in turn.
    for at in 0..16 {
        for value in [0u8, 1, 0x7F, 0xFF] {
            let mut bytes = good.clone();
            bytes[dht + 5 + at] = value;
            cases.push(("one count", bytes));
        }
    }
    // The class and id nibbles.
    for byte in [0x00, 0x01, 0x04, 0x0F, 0x10, 0x11, 0x14, 0x1F, 0xF0, 0xFF] {
        let mut bytes = good.clone();
        bytes[dht + 4] = byte;
        cases.push(("class and id", bytes));
    }
    // The symbols all alike, and the table's length wrong.
    let mut bytes = good.clone();
    bytes[dht + 21..body_end].fill(0xFF);
    cases.push(("all symbols alike", bytes));
    for claimed in [19u16, 0xFFFF] {
        let mut bytes = good.clone();
        bytes[dht + 2..dht + 4].copy_from_slice(&claimed.to_be_bytes());
        cases.push(("a table of the wrong length", bytes));
    }
    for (name, bytes) in cases {
        jpeg_settles(&format!("jpeg huffman {name}"), &bytes);
    }
    // The all-0xFF and all-zero tables are broken, not just odd.
    for fill in [0xFFu8, 0] {
        let mut bytes = good.clone();
        bytes[counts.clone()].fill(fill);
        refused(&format!("jpeg counts all {fill:#x}"), &bytes, Format::Jpeg);
    }
    // Every table gone: the scan has nothing to decode with.
    let mut bytes = good.clone();
    while let Some(at) = bytes.windows(2).position(|m| m[0] == 0xFF && m[1] == 0xC4) {
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        bytes.drain(at..at + 2 + length);
    }
    refused("jpeg without huffman tables", &bytes, Format::Jpeg);
}

#[test]
fn a_jpeg_frame_header_that_cannot_be_right_settles() {
    let good = samples::jpeg();
    let sof = segment(&good, 0xC0);
    // FF C0, length (2), precision (1), height (2), width (2), components (1),
    // then per component: id, sampling, quantisation table.
    for (what, at, values) in [
        ("precision", sof + 4, vec![0u8, 4, 8, 12, 16, 255]),
        ("components", sof + 9, vec![0, 1, 2, 4, 5, 255]),
        (
            "first sampling",
            sof + 11,
            vec![0x00, 0x01, 0x10, 0x44, 0x55, 0xFF],
        ),
        ("first table", sof + 12, vec![0, 3, 4, 255]),
    ] {
        for value in values {
            let mut bytes = good.clone();
            bytes[at] = value;
            jpeg_settles(&format!("jpeg {what} {value}"), &bytes);
        }
    }
    // Nothing but 1, 3 or 4 components, and 8 bits, is a baseline JPEG here.
    for (at, value) in [
        (sof + 9, 0u8),
        (sof + 9, 2),
        (sof + 9, 5),
        (sof + 4, 0),
        (sof + 4, 16),
    ] {
        let mut bytes = good.clone();
        bytes[at] = value;
        refused(&format!("jpeg byte {at} = {value}"), &bytes, Format::Jpeg);
    }
    // The quantisation tables: all zero and all 0xFF decode to a (wrong)
    // image of the declared size.
    let dqt = segment(&good, 0xDB);
    for (what, fill) in [("zero", 0u8), ("0xFF", 0xFF)] {
        let mut bytes = good.clone();
        let end = dqt + 2 + u16::from_be_bytes([bytes[dqt + 2], bytes[dqt + 3]]) as usize;
        bytes[dqt + 5..end].fill(fill);
        jpeg_settles(&format!("jpeg quantisation {what}"), &bytes);
    }
    // A scan header before any frame header.
    let mut bytes = good.clone();
    let sos = segment(&bytes, 0xDA);
    let scan: Vec<u8> = bytes.drain(sos..).collect();
    bytes.truncate(2);
    bytes.extend_from_slice(&scan);
    refused("jpeg scan before frame", &bytes, Format::Jpeg);
}

#[test]
fn a_jpeg_segment_that_lies_about_its_length_is_refused_but_for_an_app_marker() {
    let good = samples::jpeg();
    let segments = jpeg_segments(&good);
    assert!(
        segments.len() >= 4,
        "the sample has {} segments",
        segments.len()
    );
    let mut decoded = Vec::new();
    for (at, length) in segments {
        let marker = good[at + 1];
        for claimed in [
            0,
            1,
            2,
            3,
            length.saturating_sub(1),
            length + 1,
            length + 1_000,
            0xFFFF,
        ] {
            let mut bytes = good.clone();
            bytes[at + 2..at + 4].copy_from_slice(&(claimed as u16).to_be_bytes());
            if jpeg_settles(&format!("jpeg FF{marker:02X} claims {claimed}"), &bytes) {
                decoded.push((marker, claimed));
            }
        }
    }
    // Only the application segments (JFIF, Exif, Adobe), which a decoder skips
    // by their length, survive a length that is a byte off or merely short;
    // the tables, the frame header and every length that runs off are refused.
    let mut want = vec![
        (0xE0, 2),
        (0xE0, 3),
        (0xE0, 15),
        (0xE0, 17),
        (0xE1, 2),
        (0xE1, 3),
        (0xE1, 75),
        (0xE1, 77),
        (0xED, 2),
        (0xED, 3),
        (0xED, 55),
    ];
    decoded.sort_unstable();
    want.sort_unstable();
    assert_eq!(decoded, want, "the lengths that still decode");
    // The scan header's own length: every lie is refused.
    let sos = segment(&good, 0xDA);
    for claimed in [0u16, 1, 2, 3, 0xFFFF] {
        let mut bytes = good.clone();
        bytes[sos + 2..sos + 4].copy_from_slice(&claimed.to_be_bytes());
        refused(&format!("jpeg SOS claims {claimed}"), &bytes, Format::Jpeg);
    }
}

#[test]
fn a_jpeg_cut_inside_each_segment_is_refused() {
    let good = samples::jpeg();
    for (at, length) in jpeg_segments(&good) {
        for inside in [1, 2, 3, 4, 2 + length / 2, 1 + length] {
            let cut = at + inside;
            assert!(cut < good.len());
            refused(&format!("jpeg cut at {cut}"), &good[..cut], Format::Jpeg);
        }
    }
}

#[test]
fn a_jpeg_cut_inside_its_scan_data_settles() {
    let good = samples::jpeg();
    let sos = segment(&good, 0xDA);
    let scan = sos + 2 + u16::from_be_bytes([good[sos + 2], good[sos + 3]]) as usize;
    assert!(
        good.len() - scan > 100,
        "{} bytes of scan data",
        good.len() - scan
    );
    // Every cut from the scan header to the end: a decode is the declared
    // canvas, a refusal a bad JPEG, never a ceiling or a panic.
    for cut in sos..good.len() {
        jpeg_settles(&format!("jpeg cut in scan at {cut}"), &good[..cut]);
    }
    // Cut inside the scan header, it is refused.
    for cut in [sos + 2, sos + 4, scan - 1] {
        refused(
            &format!("jpeg cut inside the scan header at {cut}"),
            &good[..cut],
            Format::Jpeg,
        );
    }
    // With no scan data at all, or any part of it, zune pads the rest and the
    // file decodes: a flat or part-drawn image of the declared size. Every
    // other format refuses a cut file (see `images_cut_anywhere_...`), so this
    // is pinned here, and reported, rather than passed over: turning on zune's
    // strict mode would refuse it, at the price of any file it warns about.
    for cut in [scan, scan + 1, (scan + good.len()) / 2] {
        let image = body(&format!("jpeg cut in scan at {cut}"), &good[..cut]);
        assert!(image.is_ok(), "jpeg cut in scan at {cut}: {image:?}");
    }
}
