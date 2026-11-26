//! Hostile images (P6 groundwork G8): every sample mutated, cut at every byte
//! and swept field by field through the fuzz target's body, which decodes or
//! refuses and never panics; and headers made to claim too much, which are
//! refused by name before the memory they claim is asked for (the counting
//! allocator's peak says so).

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/media_harness.rs"]
mod harness;

mod samples;

use harness::media_decode;
use kaimedia::{Ceiling, Error, Format, Limits, decode, probe};

/// Every sample, by name.
fn all() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = samples::pngs()
        .into_iter()
        .map(|(name, bytes, _)| (format!("png-{name}"), bytes))
        .collect();
    out.push((String::from("jpeg"), samples::jpeg()));
    out.push((String::from("gif-animated"), samples::gif_animated()));
    let rgba = samples::rgba(samples::W, samples::H, true);
    out.push((
        String::from("webp"),
        samples::webp(&rgba, samples::W, samples::H, true),
    ));
    let small = samples::rgba(16, 12, true);
    out.push((
        String::from("webp-animated"),
        samples::webp_animated(16, 12, &[(small.clone(), 30), (small, 70)]),
    ));
    out
}

#[test]
fn mutated_images_decode_or_are_refused() {
    for (seed, (name, bytes)) in all().into_iter().enumerate() {
        eprintln!("{name}");
        hostile::fuzz(&bytes, seed as u64 + 1, 1_500, media_decode);
    }
}

#[test]
fn images_cut_anywhere_decode_or_are_refused() {
    for (name, bytes) in all() {
        eprintln!("{name}");
        hostile::truncations(&bytes, |_, cut| media_decode(cut));
    }
    // Cut in half, a PNG or a WebP is refused as bad, not shown half drawn.
    for (format, bytes) in [
        (Format::Png, samples::pngs().remove(0).1),
        (
            Format::Webp,
            samples::webp(&samples::rgba(8, 8, true), 8, 8, true),
        ),
    ] {
        match decode(&bytes[..bytes.len() / 2], &Limits::DEFAULT) {
            Err(Error::Bad(f, _)) => assert_eq!(f, format),
            other => panic!("{format:?} cut in half: {other:?}"),
        }
    }
}

#[test]
fn every_header_field_at_its_extremes_decodes_or_is_refused() {
    for (name, bytes) in all() {
        eprintln!("{name}");
        let end = bytes.len().min(48);
        hostile::sweep_fields(&bytes, 0..end.saturating_sub(8), |_, swept| {
            media_decode(swept)
        });
    }
}

/// `bytes` refused with `want` under `limits`, having allocated at most
/// `most` bytes on the way.
fn refused(name: &str, bytes: &[u8], limits: &Limits, want: Ceiling, most: usize) {
    let (got, peak) = hostile::peak_alloc(|| decode(bytes, limits));
    assert_eq!(got, Err(Error::TooBig(want)), "{name}");
    assert!(
        peak <= most,
        "{name}: {peak} bytes allocated before the refusal"
    );
}

const MIB: usize = 1 << 20;

#[test]
fn a_png_claiming_too_much_is_refused_before_its_buffer() {
    let d = &Limits::DEFAULT;
    refused(
        "png 100000 x 100000",
        &samples::png_claiming(100_000, 100_000),
        d,
        Ceiling::Side,
        MIB,
    );
    refused(
        "png 16000 x 16000",
        &samples::png_claiming(16_000, 16_000),
        d,
        Ceiling::Pixels,
        MIB,
    );
    let tight = Limits {
        max_bytes: MIB as u64,
        ..Limits::DEFAULT
    };
    refused(
        "png 1000 x 1000 in 1 MiB",
        &samples::png_claiming(1_000, 1_000),
        &tight,
        Ceiling::Bytes,
        2 * MIB,
    );
    assert_eq!(
        probe(&samples::png_claiming(100_000, 100_000), d),
        Err(Error::TooBig(Ceiling::Side))
    );
}

#[test]
fn a_jpeg_claiming_too_much_is_refused_before_its_buffer() {
    let d = &Limits::DEFAULT;
    refused(
        "jpeg 65000 x 65000",
        &samples::jpeg_claiming(65_000, 65_000),
        d,
        Ceiling::Side,
        MIB,
    );
    refused(
        "jpeg 10000 x 10000",
        &samples::jpeg_claiming(10_000, 10_000),
        d,
        Ceiling::Pixels,
        MIB,
    );
    let tight = Limits {
        max_bytes: MIB as u64,
        ..Limits::DEFAULT
    };
    // 500 x 500 is 1 MB of RGBA, and zune's scratch is counted too.
    refused(
        "jpeg 500 x 500 in 1 MiB",
        &samples::jpeg_claiming(500, 500),
        &tight,
        Ceiling::Bytes,
        MIB,
    );
}

#[test]
fn a_gif_claiming_too_much_is_refused_before_its_canvas() {
    let d = &Limits::DEFAULT;
    refused(
        "gif 65535 x 65535",
        &samples::gif_claiming(65_535, 65_535),
        d,
        Ceiling::Side,
        MIB,
    );
    refused(
        "gif 16000 x 16000",
        &samples::gif_claiming(16_000, 16_000),
        d,
        Ceiling::Pixels,
        MIB,
    );
    // Frames past the ceiling stop the decode at the first one past it.
    let few = Limits {
        max_frames: 10,
        ..Limits::DEFAULT
    };
    refused(
        "gif of 11 frames",
        &samples::gif_frames(11),
        &few,
        Ceiling::Frames,
        MIB,
    );
    refused(
        "gif of 1001 frames",
        &samples::gif_frames(1_001),
        d,
        Ceiling::Frames,
        MIB,
    );
    // A canvas and every frame's copy count against the bytes decoded.
    let canvas = u64::from(samples::GIF_W) * u64::from(samples::GIF_H) * 4;
    let tight = Limits {
        max_bytes: canvas * 3,
        ..Limits::DEFAULT
    };
    refused(
        "gif animation in 3 canvases",
        &samples::gif_animated(),
        &tight,
        Ceiling::Bytes,
        MIB,
    );
}

#[test]
fn a_webp_claiming_too_much_is_refused_before_its_frames() {
    let d = &Limits::DEFAULT;
    refused(
        "webp 20000 x 20000",
        &samples::webp_claiming(20_000, 20_000),
        d,
        Ceiling::Side,
        MIB,
    );
    refused(
        "webp 16000 x 16000",
        &samples::webp_claiming(16_000, 16_000),
        d,
        Ceiling::Pixels,
        MIB,
    );
    let dot = vec![9, 8, 7, 255];
    let many: Vec<(Vec<u8>, u32)> = (0..1_001).map(|_| (dot.clone(), 10)).collect();
    refused(
        "webp of 1001 frames",
        &samples::webp_animated(1, 1, &many),
        d,
        Ceiling::Frames,
        4 * MIB,
    );
    // Every frame's RGBA is counted before the first is decoded.
    let frames: Vec<(Vec<u8>, u32)> = (0..4).map(|_| (samples::rgba(64, 64, true), 10)).collect();
    let tight = Limits {
        max_bytes: 64 * 64 * 4 * 3,
        ..Limits::DEFAULT
    };
    refused(
        "webp of 4 frames in 3 frames' bytes",
        &samples::webp_animated(64, 64, &frames),
        &tight,
        Ceiling::Bytes,
        MIB,
    );
}

/// Writes every sample to `$FUZZ_SEED_DIR/media_decode/` for the fuzz
/// workflow's corpus.
#[test]
#[ignore]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    let folder = std::path::Path::new(&dir).join("media_decode");
    std::fs::create_dir_all(&folder).unwrap();
    for (name, bytes) in all() {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
}
