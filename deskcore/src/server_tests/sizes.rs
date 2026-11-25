//! The sizes a client names: a buffer's sides are bounded, and a window's
//! second commit is held to the work area as its first is.

use super::*;
use crate::geom::Size;

/// A pool of `size` bytes with a buffer of `width` x `height` at stride `stride`.
fn with_buffer(width: u32, height: u32, stride: u32, size: u32) -> Vec<u8> {
    bytes(&[
        (
            REGISTRY,
            Request::Bind {
                name: 1,
                interface: "pool",
                version: 1,
                id: 2,
            },
        ),
        (2, Request::PoolCreate { size }),
        (
            2,
            Request::PoolBuffer {
                id: 4,
                offset: 4096,
                width,
                height,
                stride,
                format: Format::Xrgb8888,
            },
        ),
    ])
}

#[test]
fn a_buffer_has_px_and_no_more_per_side_than_any_screen_has() {
    let mut desk = Desk::new(1280, 800);
    for (w, h, stride) in [
        (0, 0, 0),
        (0, 4, 0),
        (4, 0, 16),
        (0x1FFF_FFFF, 0, 4),
        (0, i32::MAX as u32, 0),
        (16_385, 1, 65_540),
        (1, 16_385, 4),
    ] {
        let mut client = Client::new(1, "app");
        let sent = client.handle(&mut desk, &with_buffer(w, h, stride, 1 << 20), true);
        assert_eq!(sent, Err(Error::BadArg), "{w}x{h} stride {stride}");
    }
    // The largest side a buffer may have, in a pool that holds it, is taken.
    let mut client = Client::new(1, "app");
    client
        .handle(
            &mut desk,
            &with_buffer(4, 16_384, 16, 4096 + 16 * 16_384),
            true,
        )
        .unwrap();
}

#[test]
fn a_second_commit_is_held_to_the_work_area_as_the_first_is() {
    let mut desk = Desk::new(1280, 800);
    let (window, _) = desk.open(1, "app", "Big", Size { w: 300, h: 200 }, true);
    let area = desk.layout().work_area;
    for size in [
        Size { w: 65_537, h: 94 },
        Size {
            w: i32::MAX,
            h: i32::MAX,
        },
        Size { w: 5000, h: 3000 },
    ] {
        desk.commit(window, size, &[]);
        let content = desk.window(window).unwrap().content();
        assert!(
            content.w <= area.w && content.h <= area.h,
            "{content:?} for {size:?}"
        );
    }
    // And never below the least the desk keeps.
    desk.commit(window, Size { w: 1, h: -9 }, &[]);
    let content = desk.window(window).unwrap().content();
    assert!(content.w >= 120 && content.h >= 60, "{content:?}");
}
