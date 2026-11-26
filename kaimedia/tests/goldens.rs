//! Goldens: each format's samples decode to the pixels they were made from
//! (exactly for the lossless ones, closely for JPEG), with the right size,
//! frames and delays, and `probe` reads the same header.

mod samples;

use kaimedia::{Format, Image, Limits, decode, probe, sniff};
use samples::{GIF_COLOURS, GIF_H, GIF_W, H, W};

fn one(bytes: &[u8]) -> Image {
    decode(bytes, &Limits::DEFAULT).unwrap()
}

fn px(image: &Image, frame: usize, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    image.frames[frame].rgba[at..at + 4].try_into().unwrap()
}

#[test]
fn every_png_colour_type_decodes_to_its_pixels() {
    for (name, bytes, want) in samples::pngs() {
        assert_eq!(sniff(&bytes), Some(Format::Png), "{name}");
        let image = one(&bytes);
        assert_eq!(
            (image.width, image.height, image.frames.len()),
            (W, H, 1),
            "{name}"
        );
        assert_eq!(image.frames[0].delay_ms, 0, "{name}");
        assert!(image.frames[0].rgba == want, "{name}: pixels differ");
        let info = probe(&bytes, &Limits::DEFAULT).unwrap();
        assert_eq!(
            (info.format, info.width, info.height, info.frames),
            (Format::Png, W, H, Some(1)),
            "{name}"
        );
    }
}

#[test]
fn a_jpeg_decodes_close_to_its_pixels() {
    let bytes = samples::jpeg();
    assert_eq!(sniff(&bytes), Some(Format::Jpeg));
    let image = one(&bytes);
    assert_eq!((image.width, image.height, image.frames.len()), (W, H, 1));
    let want = samples::rgba(W, H, false);
    let got = &image.frames[0].rgba;
    let (mut total, mut worst) = (0u64, 0u8);
    for (g, w) in got.iter().zip(&want) {
        let d = g.abs_diff(*w);
        total += u64::from(d);
        worst = worst.max(d);
    }
    let mean = total as f64 / got.len() as f64;
    // Quality 92, 4:2:0, smooth gradients: a few levels off at most.
    assert!(mean < 3.0, "mean error {mean}");
    assert!(worst <= 24, "worst error {worst}");
    assert!(got.chunks(4).all(|p| p[3] == 255), "a JPEG is opaque");
    let info = probe(&bytes, &Limits::DEFAULT).unwrap();
    assert_eq!((info.width, info.height, info.frames), (W, H, Some(1)));
}

#[test]
fn a_gif_animation_is_composed_frame_by_frame() {
    let bytes = samples::gif_animated();
    assert_eq!(sniff(&bytes), Some(Format::Gif));
    let image = one(&bytes);
    assert_eq!(
        (image.width, image.height),
        (u32::from(GIF_W), u32::from(GIF_H))
    );
    let delays: Vec<u32> = image.frames.iter().map(|f| f.delay_ms).collect();
    assert_eq!(delays, [50, 100, 200, 0]);
    let [clear, red, green, blue] = GIF_COLOURS;
    let first = |x: u32, y: u32| GIF_COLOURS[((x + y) % 3 + 1) as usize];
    // 1: the whole canvas.
    assert_eq!(px(&image, 0, 0, 0), first(0, 0));
    assert_eq!(px(&image, 0, 0, 0), red);
    assert_eq!(px(&image, 0, 31, 23), first(31, 23));
    // 2: a red checker at (8, 4); its transparent squares leave frame 1.
    assert_eq!(px(&image, 1, 8, 4), red);
    assert_eq!(px(&image, 1, 9, 4), first(9, 4));
    assert_eq!(px(&image, 1, 7, 4), first(7, 4));
    // 3: frame 2 was cleared to transparent; blue from (20, 10), cut at the
    // canvas's edge.
    assert_eq!(px(&image, 2, 8, 4), clear);
    assert_eq!(px(&image, 2, 15, 11), clear);
    assert_eq!(px(&image, 2, 16, 4), first(16, 4));
    assert_eq!(px(&image, 2, 25, 15), blue);
    assert_eq!(px(&image, 2, 31, 23), blue);
    assert_eq!(px(&image, 2, 19, 10), first(19, 10));
    // 4: frame 3 was put back to the canvas before it; a green dot at (0, 0).
    assert_eq!(px(&image, 3, 0, 0), green);
    assert_eq!(px(&image, 3, 25, 15), first(25, 15));
    assert_eq!(px(&image, 3, 8, 4), clear);
    let info = probe(&bytes, &Limits::DEFAULT).unwrap();
    assert_eq!((info.width, info.height, info.frames), (32, 24, None));
}

#[test]
fn a_still_gif_has_no_delay() {
    let image = one(&samples::gif_frames(1));
    assert_eq!((image.width, image.height, image.frames.len()), (1, 1, 1));
    assert_eq!(image.frames[0].delay_ms, 0);
    assert_eq!(px(&image, 0, 0, 0), [0, 0, 0, 255]);
}

#[test]
fn lossless_webps_decode_to_their_pixels() {
    for alpha in [true, false] {
        let rgba = samples::rgba(W, H, alpha);
        let data: Vec<u8> = if alpha {
            rgba.clone()
        } else {
            rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
        };
        let bytes = samples::webp(&data, W, H, alpha);
        assert_eq!(sniff(&bytes), Some(Format::Webp));
        let image = one(&bytes);
        assert_eq!((image.width, image.height, image.frames.len()), (W, H, 1));
        assert!(image.frames[0].rgba == rgba, "alpha {alpha}: pixels differ");
        let info = probe(&bytes, &Limits::DEFAULT).unwrap();
        assert_eq!((info.width, info.height, info.frames), (W, H, Some(1)));
    }
}

#[test]
fn an_animated_webp_gives_each_frame_and_its_time() {
    let (w, h) = (16, 12);
    let a = samples::rgba(w, h, true);
    let b: Vec<u8> = a.chunks(4).flat_map(|p| [p[2], p[0], p[1], 255]).collect();
    let bytes = samples::webp_animated(w, h, &[(a.clone(), 30), (b.clone(), 70)]);
    let image = one(&bytes);
    assert_eq!((image.width, image.height, image.frames.len()), (w, h, 2));
    assert_eq!(image.frames[0].delay_ms, 30);
    assert_eq!(image.frames[1].delay_ms, 70);
    assert!(image.frames[0].rgba == a);
    assert!(image.frames[1].rgba == b);
    let info = probe(&bytes, &Limits::DEFAULT).unwrap();
    assert_eq!(info.frames, Some(2));
}
