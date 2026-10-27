//! The header page: the first 4 KiB of every pool's `Shm` (ADR 0005 §3, C's
//! design). Buffers follow it in the same `Shm`. Each field has one writer:
//!
//! | Bytes | Field | Writer |
//! |---|---|---|
//! | 0..4 | magic `KSF1` | client |
//! | 4..8 | version, 1 | client |
//! | 8..12 | pool size in bytes, this page included | client |
//! | 12..16 | stride of the front buffer | client |
//! | 16..20 | format ([`Format`]) | client |
//! | 20..24 | front buffer's offset in the pool | client |
//! | 24..28 | front buffer's width | client |
//! | 28..32 | front buffer's height | client |
//! | 32..36 | `commit_seq`, bumped last, after the fields above and the damage | client |
//! | 36..40 | damage count, at most [`MAX_DAMAGE`] | client |
//! | 40..296 | damage, `u16` x, y, w, h each | client |
//! | 296..300 | `latched_seq`: the last `commit_seq` `desktop` took | desktop |
//! | 300..304 | scale, in 120ths (fractional-scale-v1) | desktop |
//! | 304..308 | event ring head: events written | desktop |
//! | 308..312 | event ring tail: events read | client |
//! | 312..316 | events dropped since the client last caught up | desktop |
//! | 2048..4096 | [`RING`] events of [`EVENT`] bytes | desktop |
//!
//! The functions here read and write a copy of the page. Sharing it between
//! processes (atomic loads of `commit_seq` and the ring indexes, with the right
//! ordering) is the caller's, and every value read from the page is checked as
//! if hostile, since the other side writes it.

use crate::input::InputEvent;
use crate::{Format, Rect};
use alloc::vec::Vec;

pub const PAGE: usize = 4096;
pub const MAGIC: [u8; 4] = *b"KSF1";
pub const VERSION: u32 = 1;
pub const MAX_DAMAGE: usize = 32;
/// Events in the ring.
pub const RING: usize = 64;
/// Bytes per event.
pub const EVENT: usize = 32;
const RING_AT: usize = 2048;

const SIZE: usize = 8;
const STRIDE: usize = 12;
const FORMAT: usize = 16;
const FRONT: usize = 20;
const WIDTH: usize = 24;
const HEIGHT: usize = 28;
const COMMIT_SEQ: usize = 32;
const DAMAGE_COUNT: usize = 36;
const DAMAGE: usize = 40;
const LATCHED_SEQ: usize = 296;
const SCALE: usize = 300;
const HEAD: usize = 304;
const TAIL: usize = 308;
const DROPPED: usize = 312;

fn get(page: &[u8; PAGE], at: usize) -> u32 {
    u32::from_le_bytes([page[at], page[at + 1], page[at + 2], page[at + 3]])
}

fn put(page: &mut [u8; PAGE], at: usize, value: u32) {
    page[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// What the client says about its front buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Front {
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: Format,
}

/// Why `desktop` refused a page. It stops showing the surface's new frames and
/// closes the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    BadMagic,
    BadVersion,
    BadFormat,
    /// The pool size is not the `Shm`'s, or the front buffer does not fit in it
    /// after this page.
    OutOfPool,
    /// A stride under width times 4, or not a multiple of 4.
    BadStride,
    TooMuchDamage,
}

/// A fresh page for a pool of `pool_size` bytes, as the client writes it.
pub fn new_page(pool_size: u32) -> [u8; PAGE] {
    let mut page = [0u8; PAGE];
    page[0..4].copy_from_slice(&MAGIC);
    put(&mut page, 4, VERSION);
    put(&mut page, SIZE, pool_size);
    put(&mut page, SCALE, 120);
    page
}

/// The client's commit: the front buffer and its damage, then `commit_seq`
/// bumped. Damage past [`MAX_DAMAGE`] rectangles becomes their bounding box.
pub fn commit(page: &mut [u8; PAGE], front: Front, damage: &[Rect]) {
    put(page, FRONT, front.offset);
    put(page, WIDTH, front.width);
    put(page, HEIGHT, front.height);
    put(page, STRIDE, front.stride);
    put(page, FORMAT, front.format as u32);
    let boxed;
    let damage = if damage.len() > MAX_DAMAGE {
        boxed = [bounding(damage)];
        &boxed[..]
    } else {
        damage
    };
    put(page, DAMAGE_COUNT, damage.len() as u32);
    for (n, rect) in damage.iter().enumerate() {
        let at = DAMAGE + n * 8;
        for (i, v) in [rect.x, rect.y, rect.w, rect.h].into_iter().enumerate() {
            let v = v.min(u32::from(u16::MAX)) as u16;
            page[at + i * 2..at + i * 2 + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    put(page, COMMIT_SEQ, get(page, COMMIT_SEQ).wrapping_add(1));
}

fn bounding(rects: &[Rect]) -> Rect {
    let x = rects.iter().map(|r| r.x).min().unwrap_or(0);
    let y = rects.iter().map(|r| r.y).min().unwrap_or(0);
    let x2 = rects
        .iter()
        .map(|r| r.x.saturating_add(r.w))
        .max()
        .unwrap_or(0);
    let y2 = rects
        .iter()
        .map(|r| r.y.saturating_add(r.h))
        .max()
        .unwrap_or(0);
    Rect {
        x,
        y,
        w: x2 - x,
        h: y2 - y,
    }
}

/// A commit as `desktop` latches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Latched {
    pub seq: u32,
    pub front: Front,
    /// The damage, each rectangle cut to the buffer; empty rectangles dropped.
    pub damage: Vec<Rect>,
}

/// What `desktop` reads from a page whose `Shm` is `pool_size` bytes: `None`
/// when `commit_seq` has not moved since `latched_seq`. Every field is checked:
/// the client owns the page and may write anything to it at any time.
pub fn latch(page: &[u8; PAGE], pool_size: u32) -> Result<Option<Latched>, Error> {
    if page[0..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    if get(page, 4) != VERSION {
        return Err(Error::BadVersion);
    }
    let seq = get(page, COMMIT_SEQ);
    if seq == get(page, LATCHED_SEQ) {
        return Ok(None);
    }
    if get(page, SIZE) != pool_size {
        return Err(Error::OutOfPool);
    }
    let front = Front {
        offset: get(page, FRONT),
        width: get(page, WIDTH),
        height: get(page, HEIGHT),
        stride: get(page, STRIDE),
        format: Format::from_raw(get(page, FORMAT)).ok_or(Error::BadFormat)?,
    };
    if !front.stride.is_multiple_of(4)
        || front
            .width
            .checked_mul(4)
            .is_none_or(|row| front.stride < row)
    {
        return Err(Error::BadStride);
    }
    if !front.offset.is_multiple_of(4) {
        return Err(Error::OutOfPool);
    }
    let end = u64::from(front.stride) * u64::from(front.height) + u64::from(front.offset);
    if (front.offset as usize) < PAGE || end > u64::from(pool_size) {
        return Err(Error::OutOfPool);
    }
    let count = get(page, DAMAGE_COUNT) as usize;
    if count > MAX_DAMAGE {
        return Err(Error::TooMuchDamage);
    }
    let damage = (0..count)
        .filter_map(|n| {
            let at = DAMAGE + n * 8;
            let v =
                |i: usize| u32::from(u16::from_le_bytes([page[at + i * 2], page[at + i * 2 + 1]]));
            Rect {
                x: v(0),
                y: v(1),
                w: v(2),
                h: v(3),
            }
            .clip(front.width, front.height)
        })
        .collect();
    Ok(Some(Latched { seq, front, damage }))
}

/// `desktop` marks `seq` latched, so the client knows its buffer is free.
pub fn set_latched(page: &mut [u8; PAGE], seq: u32) {
    put(page, LATCHED_SEQ, seq);
}

/// `desktop` sets the scale it wants, in 120ths.
pub fn set_scale(page: &mut [u8; PAGE], scale: u32) {
    put(page, SCALE, scale);
}

pub fn scale(page: &[u8; PAGE]) -> u32 {
    get(page, SCALE)
}

/// An event in the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The commit `seq` was shown at `time_ns`.
    FrameDone { seq: u32, time_ns: u64 },
    /// `desktop` is done reading the buffer at `offset`.
    BufferRelease { offset: u32 },
    /// Input for this surface, at `x`, `y` in surface-local logical units (24.8),
    /// and for a key the text it types (up to 4 bytes of UTF-8, `len` of them),
    /// so a native app needs no keymap.
    Input {
        event: InputEvent,
        x: i32,
        y: i32,
        text: [u8; 4],
        len: u8,
    },
}

impl Event {
    pub fn encode(&self) -> [u8; EVENT] {
        let mut out = [0u8; EVENT];
        match *self {
            Event::FrameDone { seq, time_ns } => {
                out[0] = 1;
                out[4..8].copy_from_slice(&seq.to_le_bytes());
                out[16..24].copy_from_slice(&time_ns.to_le_bytes());
            }
            Event::BufferRelease { offset } => {
                out[0] = 2;
                out[4..8].copy_from_slice(&offset.to_le_bytes());
            }
            Event::Input {
                event,
                x,
                y,
                text,
                len,
            } => {
                out[0] = 3;
                out[1] = len.min(4);
                out[4..8].copy_from_slice(&x.to_le_bytes());
                out[8..12].copy_from_slice(&y.to_le_bytes());
                out[12..16].copy_from_slice(&text);
                out[16..32].copy_from_slice(&event.encode());
            }
        }
        out
    }

    /// `None` for an unknown kind, or key text that is not UTF-8.
    pub fn decode(bytes: &[u8; EVENT]) -> Option<Event> {
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        match bytes[0] {
            1 => Some(Event::FrameDone {
                seq: word(4),
                time_ns: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            }),
            2 => Some(Event::BufferRelease { offset: word(4) }),
            3 => {
                let len = bytes[1];
                let text: [u8; 4] = bytes[12..16].try_into().ok()?;
                core::str::from_utf8(text.get(..usize::from(len))?).ok()?;
                Some(Event::Input {
                    event: InputEvent::decode(bytes[16..32].try_into().ok()?)?,
                    x: word(4) as i32,
                    y: word(8) as i32,
                    text,
                    len,
                })
            }
            _ => None,
        }
    }
}

/// `desktop` posts `event`. A full ring drops it and counts the drop, so a
/// client that stops reading costs `desktop` nothing.
pub fn post(page: &mut [u8; PAGE], event: &Event) -> bool {
    let head = get(page, HEAD);
    let tail = get(page, TAIL);
    // The client writes the tail: one past the head is nonsense, read as full.
    if head.wrapping_sub(tail) as usize >= RING {
        put(page, DROPPED, get(page, DROPPED).saturating_add(1));
        return false;
    }
    let at = RING_AT + (head as usize % RING) * EVENT;
    page[at..at + EVENT].copy_from_slice(&event.encode());
    put(page, HEAD, head.wrapping_add(1));
    true
}

/// The client takes the next event: `Ok(None)` when the ring is empty, and an
/// event it cannot decode is skipped as `Err(())`.
#[allow(clippy::result_unit_err)]
pub fn take(page: &mut [u8; PAGE]) -> Result<Option<Event>, ()> {
    let head = get(page, HEAD);
    let tail = get(page, TAIL);
    if head == tail {
        return Ok(None);
    }
    // `desktop` writes the head; a head more than a ring ahead resynchronises.
    let tail = match head.wrapping_sub(tail) as usize > RING {
        true => head.wrapping_sub(RING as u32),
        false => tail,
    };
    let at = RING_AT + (tail as usize % RING) * EVENT;
    let bytes: &[u8; EVENT] = page[at..at + EVENT].try_into().map_err(|_| ())?;
    let event = Event::decode(bytes);
    put(page, TAIL, tail.wrapping_add(1));
    event.map(Some).ok_or(())
}

/// Events dropped because the ring was full.
pub fn dropped(page: &[u8; PAGE]) -> u32 {
    get(page, DROPPED)
}
