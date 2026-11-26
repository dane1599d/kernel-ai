//! The ceilings in all four formats: the magic alone, each ceiling at its
//! limit and one past it, and what a file refused for its size allocated.

use super::crafted::*;
use super::png::PNG_MAGIC;
use super::samples;
use super::webp::riff;
use kaimedia::{Ceiling, Error, Format, Limits};

#[test]
fn bytes_that_pass_only_the_magic_are_refused() {
    let magics: [(&str, Format, Vec<u8>); 4] = [
        ("png", Format::Png, PNG_MAGIC.to_vec()),
        ("jpeg", Format::Jpeg, vec![0xFF, 0xD8, 0xFF]),
        ("gif", Format::Gif, b"GIF89a".to_vec()),
        ("webp", Format::Webp, riff(&[])),
    ];
    for (name, format, magic) in magics {
        for tail in [
            vec![],
            vec![0u8; 64],
            vec![0xFFu8; 64],
            (0..=255u8).collect(),
        ] {
            let mut bytes = magic.clone();
            bytes.extend_from_slice(&tail);
            refused(
                &format!("{name} magic and {} bytes", tail.len()),
                &bytes,
                format,
            );
        }
    }
}

/// A sample and what it decodes to.
struct Sample {
    name: &'static str,
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    frames: u32,
    /// The fewest bytes of ceiling it decodes in, as the budget counts them:
    /// the canvas for a PNG, the canvas and eight bytes a pixel of scratch for
    /// a JPEG, and so on. Pinned, so a ceiling off by one fails here.
    least: u64,
}

fn samples_all() -> Vec<Sample> {
    let (w, h) = (samples::W, samples::H);
    let small = samples::rgba(16, 12, true);
    vec![
        Sample {
            name: "png",
            bytes: samples::pngs().remove(0).1,
            width: w,
            height: h,
            frames: 1,
            least: 12288,
        },
        Sample {
            name: "jpeg",
            bytes: samples::jpeg(),
            width: w,
            height: h,
            frames: 1,
            least: 36864,
        },
        Sample {
            name: "gif",
            bytes: samples::gif_animated(),
            width: u32::from(samples::GIF_W),
            height: u32::from(samples::GIF_H),
            frames: 4,
            least: 21504,
        },
        Sample {
            name: "webp",
            bytes: samples::webp(&samples::rgba(w, h, true), w, h, true),
            width: w,
            height: h,
            frames: 1,
            least: 24576,
        },
        Sample {
            name: "webp-animated",
            bytes: samples::webp_animated(16, 12, &[(small.clone(), 30), (small, 70)]),
            width: 16,
            height: 12,
            frames: 2,
            least: 2304,
        },
    ]
}

/// Ceilings no sample reaches.
const ROOMY: Limits = Limits {
    max_input: 1 << 20,
    max_side: 4_096,
    max_pixels: 1 << 24,
    max_frames: 64,
    max_bytes: 1 << 28,
};

fn ok_under(s: &Sample, limits: &Limits, what: &str) {
    let name = format!("{} {what}", s.name);
    let (got, _) = run_under(&name, &s.bytes, limits);
    let image = got.unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert_eq!((image.width, image.height), (s.width, s.height), "{name}");
    assert_eq!(image.frames.len() as u32, s.frames, "{name}");
}

fn over(s: &Sample, limits: &Limits, ceiling: Ceiling, what: &str) {
    let name = format!("{} {what}", s.name);
    let (got, peak) = run_under(&name, &s.bytes, limits);
    assert_eq!(got.map(dims), Err(Error::TooBig(ceiling)), "{name}");
    // Refused, so what it made is under what the ceiling allowed, and the
    // sample's own size at that.
    assert!(
        peak <= limits.max_bytes as usize + SIZE_PEAK,
        "{name}: {peak} bytes"
    );
}

#[test]
fn each_ceiling_holds_at_its_limit_and_just_past_it() {
    for s in samples_all() {
        let (w, h) = (s.width, s.height);
        // The longest side.
        let at = Limits {
            max_side: w.max(h),
            ..ROOMY
        };
        ok_under(&s, &at, "at the side ceiling");
        over(
            &s,
            &Limits {
                max_side: w.max(h) - 1,
                ..ROOMY
            },
            Ceiling::Side,
            "past the side ceiling",
        );
        // The pixels.
        let at = Limits {
            max_pixels: u64::from(w) * u64::from(h),
            ..ROOMY
        };
        ok_under(&s, &at, "at the pixel ceiling");
        let past = Limits {
            max_pixels: u64::from(w) * u64::from(h) - 1,
            ..ROOMY
        };
        over(&s, &past, Ceiling::Pixels, "past the pixel ceiling");
        // The input's length.
        let at = Limits {
            max_input: s.bytes.len(),
            ..ROOMY
        };
        ok_under(&s, &at, "at the input ceiling");
        let past = Limits {
            max_input: s.bytes.len() - 1,
            ..ROOMY
        };
        over(&s, &past, Ceiling::Input, "past the input ceiling");
        // The frames.
        ok_under(
            &s,
            &Limits {
                max_frames: s.frames,
                ..ROOMY
            },
            "at the frame ceiling",
        );
        let past = Limits {
            max_frames: s.frames - 1,
            ..ROOMY
        };
        over(&s, &past, Ceiling::Frames, "past the frame ceiling");
    }
}

#[test]
fn the_byte_ceiling_holds_at_its_limit_and_just_past_it() {
    for s in samples_all() {
        let canvas = u64::from(s.width) * u64::from(s.height) * 4;
        let decodes = |max_bytes: u64| {
            let limits = Limits { max_bytes, ..ROOMY };
            run_under(s.name, &s.bytes, &limits).0
        };
        // The least the decode needs: found, then held to as the ceiling.
        let (mut low, mut high) = (0, 1u64 << 28);
        while low + 1 < high {
            let mid = (low + high) / 2;
            match decodes(mid) {
                Ok(_) => high = mid,
                Err(Error::TooBig(Ceiling::Bytes)) => low = mid,
                Err(other) => panic!("{} at {mid} bytes: {other:?}", s.name),
            }
        }
        let least = high;
        assert_eq!(least, s.least, "{}: the fewest bytes it decodes in", s.name);
        assert!(least >= canvas, "{}: the image itself is counted", s.name);
        let at = Limits {
            max_bytes: least,
            ..ROOMY
        };
        ok_under(&s, &at, "at the byte ceiling");
        let past = Limits {
            max_bytes: least - 1,
            ..ROOMY
        };
        over(&s, &past, Ceiling::Bytes, "past the byte ceiling");
        // At the ceiling the decode held no more than the ceiling and the input.
        let (_, peak) = run_under(s.name, &s.bytes, &at);
        assert!(
            peak as u64 <= least + s.bytes.len() as u64 + SIZE_PEAK as u64,
            "{}: peak {peak} at a ceiling of {least}",
            s.name
        );
    }
}

#[test]
fn a_file_refused_for_its_size_allocated_next_to_nothing() {
    for (name, bytes, ceiling) in [
        (
            "png",
            samples::png_claiming(100_000, 100_000),
            Ceiling::Side,
        ),
        ("png", samples::png_claiming(1_024, 1_025), Ceiling::Side),
        (
            "jpeg",
            samples::jpeg_claiming(65_000, 65_000),
            Ceiling::Side,
        ),
        ("jpeg", samples::jpeg_claiming(1_025, 1), Ceiling::Side),
        ("gif", samples::gif_claiming(65_535, 65_535), Ceiling::Side),
        ("gif", samples::gif_claiming(1_025, 1), Ceiling::Side),
        (
            "webp",
            samples::webp_claiming(20_000, 20_000),
            Ceiling::Side,
        ),
        ("webp", samples::webp_claiming(1_025, 1), Ceiling::Side),
    ] {
        refused_big(&format!("{name} {ceiling:?}"), &bytes, &LIMITS, ceiling);
    }
    // At the pixel ceiling under LIMITS: sides within it, the area past it.
    let tight = Limits {
        max_pixels: 100 * 100,
        ..LIMITS
    };
    for (name, bytes) in [
        ("png", samples::png_claiming(101, 100)),
        ("jpeg", samples::jpeg_claiming(101, 100)),
        ("gif", samples::gif_claiming(101, 100)),
        ("webp", samples::webp_claiming(101, 100)),
    ] {
        refused_big(&format!("{name} pixels"), &bytes, &tight, Ceiling::Pixels);
    }
}
