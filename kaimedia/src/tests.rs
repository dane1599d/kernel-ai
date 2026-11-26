use super::*;

#[test]
fn sniff_knows_each_format_by_its_first_bytes() {
    assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some(Format::Png));
    assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
    assert_eq!(sniff(b"GIF87a"), Some(Format::Gif));
    assert_eq!(sniff(b"GIF89a"), Some(Format::Gif));
    assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::Webp));
    assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), None);
    assert_eq!(sniff(b"GIF8"), None);
    assert_eq!(sniff(b""), None);
}

#[test]
fn nothing_known_is_unknown_and_too_long_is_refused_first() {
    assert_eq!(decode(b"hello", &Limits::DEFAULT), Err(Error::Unknown));
    let limits = Limits {
        max_input: 4,
        ..Limits::DEFAULT
    };
    assert_eq!(
        decode(b"GIF89a", &limits),
        Err(Error::TooBig(Ceiling::Input))
    );
    assert_eq!(
        probe(b"GIF89a", &limits),
        Err(Error::TooBig(Ceiling::Input))
    );
}

#[test]
fn sizes_are_held_to_the_side_and_pixel_ceilings() {
    let limits = Limits {
        max_side: 100,
        max_pixels: 5_000,
        ..Limits::DEFAULT
    };
    assert_eq!(check_size(&limits, 100, 50), Ok(()));
    assert_eq!(
        check_size(&limits, 101, 1),
        Err(Error::TooBig(Ceiling::Side))
    );
    assert_eq!(
        check_size(&limits, 1, 101),
        Err(Error::TooBig(Ceiling::Side))
    );
    assert_eq!(
        check_size(&limits, 100, 51),
        Err(Error::TooBig(Ceiling::Pixels))
    );
    // No overflow at the widest a format can say.
    let open = Limits {
        max_side: u32::MAX,
        max_pixels: u64::MAX,
        ..Limits::DEFAULT
    };
    assert_eq!(check_size(&open, u32::MAX, u32::MAX), Ok(()));
}

#[test]
fn the_budget_refuses_before_it_allocates() {
    let limits = Limits {
        max_bytes: 100,
        max_frames: 2,
        ..Limits::DEFAULT
    };
    let mut budget = Budget::new(&limits);
    assert_eq!(budget.buffer(60).map(|b| b.len()), Ok(60));
    assert_eq!(budget.left(), 40);
    assert_eq!(budget.buffer(41), Err(Error::TooBig(Ceiling::Bytes)));
    assert_eq!(budget.reserve(40), Ok(()));
    assert_eq!(budget.reserve(1), Err(Error::TooBig(Ceiling::Bytes)));
    assert_eq!(budget.buffer(u64::MAX), Err(Error::TooBig(Ceiling::Bytes)));
    assert_eq!(budget.frame(), Ok(()));
    assert_eq!(budget.frame(), Ok(()));
    assert_eq!(budget.frame(), Err(Error::TooBig(Ceiling::Frames)));
}

#[test]
fn every_channel_count_widens_to_rgba() {
    let mut out = [0u8; 8];
    to_rgba(&[7, 9], 1, &mut out);
    assert_eq!(out, [7, 7, 7, 255, 9, 9, 9, 255]);
    to_rgba(&[7, 1, 9, 2], 2, &mut out);
    assert_eq!(out, [7, 7, 7, 1, 9, 9, 9, 2]);
    to_rgba(&[1, 2, 3, 4, 5, 6], 3, &mut out);
    assert_eq!(out, [1, 2, 3, 255, 4, 5, 6, 255]);
    to_rgba(&[1, 2, 3, 4, 5, 6, 7, 8], 4, &mut out);
    assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8]);
}
