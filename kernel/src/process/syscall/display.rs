//! The screen and input (lane G-core, ADR 0005 §2): DISPLAY_CLAIM,
//! DISPLAY_FLUSH, DISPLAY_CURSOR, DISPLAY_CURSOR_MOVE and INPUT_READ. The first
//! needs a `Display` handle, INPUT_READ an `Input` handle; the kernel gives both
//! only to the service `desktop` running the program `desktop`. Flushing and the
//! cursor need the claim itself: a process that lost the screen to the console
//! changes nothing on it.

use super::need;
use crate::display::{self, claim};
use crate::input;
use crate::process::uaccess::{copy_from_user, copy_to_user};
use alloc::vec::Vec;
use gfxcore::Rect;
use kabi::display::{CURSOR, DisplayInfo, FlushRect, MAX_FLUSH};
use kabi::input::InputEvent;
use kabi::{Error, call, cap};

const CALLS: &[u64] = &[
    call::DISPLAY_CLAIM,
    call::DISPLAY_FLUSH,
    call::DISPLAY_CURSOR,
    call::DISPLAY_CURSOR_MOVE,
    call::INPUT_READ,
];

/// The most events one INPUT_READ returns.
const MAX_READ: usize = 64;
/// The longest INPUT_READ waits, so a holder still notices its own exit.
const MAX_WAIT_MS: u64 = 1000;

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::DISPLAY_CLAIM => claim_call(slot, a[0]),
        call::DISPLAY_FLUSH => flush(slot, a[0], a[1]),
        call::DISPLAY_CURSOR => cursor(slot, a[0], a[1], a[2]),
        call::DISPLAY_CURSOR_MOVE => cursor_move(slot, a[0], a[1]),
        _ => read(slot, a[0], a[1], a[2]),
    })
}

fn claim_call(slot: usize, info_ptr: u64) -> Result<u64, Error> {
    need(slot, cap::DISPLAY)?;
    let mut info = DisplayInfo::default();
    let handle = claim::claim(slot, &mut info)?;
    // SAFETY: DisplayInfo is plain `repr(C)` words.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&raw const info).cast::<u8>(),
            core::mem::size_of::<DisplayInfo>(),
        )
    };
    copy_to_user(slot, info_ptr, bytes)?;
    Ok(handle)
}

/// The claim's holder, showing now.
fn showing(slot: usize) -> Result<display::Surface, Error> {
    need(slot, cap::DISPLAY)?;
    if !claim::shows(slot) {
        return Err(Error::Permission);
    }
    display::surface().ok_or(Error::NotFound)
}

fn flush(slot: usize, rects_ptr: u64, n: u64) -> Result<u64, Error> {
    let surface = showing(slot)?;
    let n = usize::try_from(n)
        .ok()
        .filter(|&n| n <= MAX_FLUSH)
        .ok_or(Error::TooBig)?;
    let mut raw = [0u8; MAX_FLUSH * 16];
    copy_from_user(slot, rects_ptr, &mut raw[..n * 16])?;
    let (width, height) = (surface.width as u32, surface.height as u32);
    for chunk in raw[..n * 16].as_chunks::<16>().0 {
        let word = |at: usize| {
            u32::from_le_bytes([chunk[at], chunk[at + 1], chunk[at + 2], chunk[at + 3]])
        };
        let rect = FlushRect {
            x: word(0),
            y: word(4),
            w: word(8),
            h: word(12),
        };
        let x2 = rect.x.saturating_add(rect.w).min(width);
        let y2 = rect.y.saturating_add(rect.h).min(height);
        if rect.x < x2 && rect.y < y2 {
            // A busy display sends the whole screen next time; not an error here.
            let _ = display::flush(Rect::new(rect.x, rect.y, x2 - rect.x, y2 - rect.y));
            claim::flushed();
        }
    }
    Ok(0)
}

fn cursor(slot: usize, image_ptr: u64, hot_x: u64, hot_y: u64) -> Result<u64, Error> {
    showing(slot)?;
    let side = CURSOR as u64;
    if hot_x >= side || hot_y >= side {
        return Err(Error::Invalid);
    }
    let mut image = Vec::new();
    image
        .try_reserve_exact(CURSOR * CURSOR * 4)
        .map_err(|_| Error::NoMemory)?;
    image.resize(CURSOR * CURSOR * 4, 0);
    copy_from_user(slot, image_ptr, &mut image)?;
    let pixel = |x: u32, y: u32| {
        let at = (y as usize * CURSOR + x as usize) * 4;
        u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]])
    };
    display::set_cursor(pixel, (hot_x as u32, hot_y as u32), claim::cursor())
        .map_err(|_| Error::NotFound)?;
    Ok(0)
}

fn cursor_move(slot: usize, x: u64, y: u64) -> Result<u64, Error> {
    let surface = showing(slot)?;
    let at = (
        (x.min(surface.width as u64 - 1)) as u32,
        (y.min(surface.height as u64 - 1)) as u32,
    );
    claim::set_cursor_at(at);
    display::move_cursor(at).map_err(|_| Error::NotFound)?;
    Ok(0)
}

fn read(slot: usize, buf_ptr: u64, cap_events: u64, timeout_ms: u64) -> Result<u64, Error> {
    need(slot, cap::INPUT)?;
    let n = usize::try_from(cap_events)
        .unwrap_or(usize::MAX)
        .min(MAX_READ);
    let mut events = [InputEvent {
        time_ns: 0,
        kind: kabi::input::Kind::Screen,
        code: 0,
        value: 0,
    }; MAX_READ];
    let got = input::read(&mut events[..n], timeout_ms.min(MAX_WAIT_MS));
    let mut bytes = [0u8; MAX_READ * InputEvent::SIZE];
    for (event, out) in events[..got]
        .iter()
        .zip(bytes.as_chunks_mut::<{ InputEvent::SIZE }>().0)
    {
        *out = event.encode();
    }
    copy_to_user(slot, buf_ptr, &bytes[..got * InputEvent::SIZE])?;
    Ok(got as u64)
}
