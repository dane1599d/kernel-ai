//! libFuzzer's `kaifs_image` target (P6 G29): the input's first byte picks how
//! the rest becomes a disk (raw, or edits to a good image), and every reader of
//! kaifs then runs over it. The body is kaifs/tests/hostile/image.rs, which
//! kaifs's own hostile tests also run (`image.rs` declares its `map` module).

#![no_main]

#[path = "../../kaifs/tests/hostile/image.rs"]
mod image;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    image::kaifs_image(data);
});
