//! HID reports: boot keyboard reports turned into key presses and releases (and
//! those into PS/2 scancode set 1, which the kernel's keyboard path already
//! decodes), boot mouse reports, and enough of the report descriptor to find a
//! pointer's buttons, X, Y and wheel, absolute (a tablet) or relative.

/// Usage IDs of the keyboard's eight modifier keys start here (Left Control).
pub const FIRST_MODIFIER: u8 = 0xE0;
/// A boot report with this in every key slot means too many keys are down.
const ROLLOVER: u8 = 0x01;

/// The last boot keyboard report, to tell presses and releases apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    last: [u8; 8],
}

impl Keyboard {
    pub const fn new() -> Self {
        Self { last: [0; 8] }
    }

    /// Takes one boot report (modifiers, reserved, six key slots) and calls `key`
    /// with each usage that changed and whether it is now down: releases first,
    /// then presses. A rollover report changes nothing.
    pub fn report(&mut self, report: &[u8], mut key: impl FnMut(u8, bool)) {
        let mut now = [0u8; 8];
        let len = report.len().min(8);
        now[..len].copy_from_slice(&report[..len]);
        if now[2..].contains(&ROLLOVER) {
            return;
        }
        let (was, is) = (self.last[0], now[0]);
        for bit in 0..8 {
            if was & (1 << bit) != 0 && is & (1 << bit) == 0 {
                key(FIRST_MODIFIER + bit, false);
            }
        }
        for &usage in &self.last[2..] {
            if usage != 0 && !now[2..].contains(&usage) {
                key(usage, false);
            }
        }
        for bit in 0..8 {
            if was & (1 << bit) == 0 && is & (1 << bit) != 0 {
                key(FIRST_MODIFIER + bit, true);
            }
        }
        for &usage in &now[2..] {
            if usage != 0 && !self.last[2..].contains(&usage) {
                key(usage, true);
            }
        }
        self.last = now;
    }
}

/// One key's PS/2 set 1 bytes: an optional 0xE0 prefix and the code, with bit 7
/// set on release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scancode {
    bytes: [u8; 2],
    len: usize,
}

impl Scancode {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Marks a set 1 code that comes after an 0xE0 prefix.
const E0: u16 = 0x100;

/// Set 1 codes for usages 0x04 (A) to 0x65 (Application).
#[rustfmt::skip]
const SET1: [u16; 0x62] = [
    // A to Z.
    0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32,
    0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    // 1 to 9, 0.
    0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
    // Enter, Escape, Backspace, Tab, Space, - = [ ] \ (non-US #) ; ' ` , . /
    0x1C, 0x01, 0x0E, 0x0F, 0x39, 0x0C, 0x0D, 0x1A, 0x1B, 0x2B, 0x2B, 0x27, 0x28,
    0x29, 0x33, 0x34, 0x35,
    // Caps Lock, F1 to F12.
    0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40, 0x41, 0x42, 0x43, 0x44, 0x57, 0x58,
    // Print Screen, Scroll Lock, Pause (both left out: multi-byte sequences).
    0x00, 0x46, 0x00,
    // Insert, Home, Page Up, Delete, End, Page Down, Right, Left, Down, Up.
    E0 | 0x52, E0 | 0x47, E0 | 0x49, E0 | 0x53, E0 | 0x4F, E0 | 0x51, E0 | 0x4D,
    E0 | 0x4B, E0 | 0x50, E0 | 0x48,
    // Num Lock, keypad / * - + Enter 1 2 3 4 5 6 7 8 9 0 .
    0x45, E0 | 0x35, 0x37, 0x4A, 0x4E, E0 | 0x1C, 0x4F, 0x50, 0x51, 0x4B, 0x4C,
    0x4D, 0x47, 0x48, 0x49, 0x52, 0x53,
    // Non-US \, Application.
    0x56, E0 | 0x5D,
];

/// Set 1 codes for the modifiers, usages 0xE0 to 0xE7: left Control, Shift,
/// Alt, GUI, then the right ones.
const MODIFIERS: [u16; 8] = [
    0x1D,
    0x2A,
    0x38,
    E0 | 0x5B,
    E0 | 0x1D,
    0x36,
    E0 | 0x38,
    E0 | 0x5C,
];

/// The set 1 bytes for a usage going down or up; None for keys set 1 has no
/// plain code for.
pub fn set1(usage: u8, pressed: bool) -> Option<Scancode> {
    let code = match usage {
        0x04..=0x65 => SET1[usize::from(usage - 0x04)],
        FIRST_MODIFIER..=0xE7 => MODIFIERS[usize::from(usage - FIRST_MODIFIER)],
        _ => 0,
    };
    if code == 0 {
        return None;
    }
    let byte = code as u8 | if pressed { 0 } else { 0x80 };
    Some(match code & E0 != 0 {
        true => Scancode {
            bytes: [0xE0, byte],
            len: 2,
        },
        false => Scancode {
            bytes: [byte, 0],
            len: 1,
        },
    })
}

/// One pointer report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pointer {
    /// Bit n is button n + 1 (left, right, middle, ...).
    pub buttons: u8,
    /// Movement for a relative pointer; the position from 0 for an absolute one.
    pub x: i32,
    pub y: i32,
    pub wheel: i32,
    /// For an absolute pointer, the largest X and Y it reports.
    pub range: Option<(u32, u32)>,
}

/// A boot mouse report: buttons, then X and Y movement as signed bytes, and on
/// many mice a wheel byte.
pub fn boot_mouse(report: &[u8]) -> Option<Pointer> {
    if report.len() < 3 {
        return None;
    }
    Some(Pointer {
        buttons: report[0] & 0x1F,
        x: i32::from(report[1] as i8),
        y: i32::from(report[2] as i8),
        wheel: report.get(3).map_or(0, |&w| i32::from(w as i8)),
        range: None,
    })
}

/// One field of a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// Bits from the start of the report's data, after any report ID byte.
    pub offset: u32,
    pub size: u8,
    pub min: i32,
    pub max: i32,
    pub relative: bool,
}

impl Field {
    /// The field's value in `data`, sign-extended when the logical range goes
    /// below zero. None when the report is too short.
    pub fn read(&self, data: &[u8]) -> Option<i32> {
        let size = u32::from(self.size);
        if size == 0 || size > 32 || (self.offset + size).div_ceil(8) as usize > data.len() {
            return None;
        }
        let mut value: u64 = 0;
        for bit in 0..size {
            let at = self.offset + bit;
            let set = data[(at / 8) as usize] >> (at % 8) & 1;
            value |= u64::from(set) << bit;
        }
        if self.min < 0 && size < 32 && value & (1 << (size - 1)) != 0 {
            value |= !0u64 << size;
        }
        Some(value as u32 as i32)
    }
}

/// Where a pointer's report keeps its fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerLayout {
    /// The report ID byte the report starts with, if the descriptor uses IDs.
    pub report_id: Option<u8>,
    pub buttons: [Option<u32>; 8],
    pub x: Option<Field>,
    pub y: Option<Field>,
    pub wheel: Option<Field>,
}

impl PointerLayout {
    /// Whether X and Y are positions, as on a tablet.
    pub fn absolute(&self) -> bool {
        matches!((self.x, self.y), (Some(x), Some(y)) if !x.relative && !y.relative)
    }

    /// Reads one report. None for a report with another ID or too short.
    pub fn read(&self, report: &[u8]) -> Option<Pointer> {
        let data = match self.report_id {
            Some(id) => report.strip_prefix(&[id])?,
            None => report,
        };
        let (x, y) = (self.x?, self.y?);
        let mut buttons = 0;
        for (index, offset) in self.buttons.iter().enumerate() {
            if let Some(offset) = offset
                && data
                    .get((offset / 8) as usize)
                    .is_some_and(|b| b >> (offset % 8) & 1 != 0)
            {
                buttons |= 1 << index;
            }
        }
        let range = |field: Field| field.max.saturating_sub(field.min).max(0) as u32;
        let absolute = self.absolute();
        let position = |field: Field| -> Option<i32> {
            let value = field.read(data)?;
            Some(match absolute {
                true => value
                    .saturating_sub(field.min)
                    .clamp(0, range(field) as i32),
                false => value,
            })
        };
        Some(Pointer {
            buttons,
            x: position(x)?,
            y: position(y)?,
            wheel: self.wheel.and_then(|w| w.read(data)).unwrap_or(0),
            range: absolute.then(|| (range(x), range(y))),
        })
    }
}

const PAGE_DESKTOP: u32 = 0x01;
const PAGE_BUTTON: u32 = 0x09;
const USAGE_POINTER: u32 = 0x01;
const USAGE_MOUSE: u32 = 0x02;
const USAGE_X: u32 = 0x30;
const USAGE_Y: u32 = 0x31;
const USAGE_WHEEL: u32 = 0x38;
/// Report IDs whose bit offsets the parser tracks; more are ignored.
const MAX_IDS: usize = 8;
/// Usages one main item may list; more are ignored.
const MAX_USAGES: usize = 16;

/// The parser's global state (HID 1.11 section 6.2.2.7), pushed and popped as a
/// whole.
#[derive(Clone, Copy, Default)]
struct Globals {
    page: u32,
    min: i32,
    max: i32,
    size: u32,
    count: u32,
    id: u8,
}

#[derive(Default)]
struct Locals {
    usages: [u32; MAX_USAGES],
    len: usize,
    min: Option<u32>,
    max: Option<u32>,
}

impl Locals {
    /// The full (page << 16 | id) usage of the item's `index`-th field.
    fn usage(&self, index: u32, page: u32) -> Option<u32> {
        let full = |usage: u32| match usage > 0xFFFF {
            true => usage,
            false => (page << 16) | usage,
        };
        if self.len > 0 {
            return Some(full(self.usages[(index as usize).min(self.len - 1)]));
        }
        let (min, max) = (self.min?, self.max?);
        Some(full(min.saturating_add(index).min(max)))
    }
}

/// Finds the first pointer (a Generic Desktop Mouse or Pointer application
/// collection) whose input report has X and Y. None when there is none.
pub fn pointer_layout(descriptor: &[u8]) -> Option<PointerLayout> {
    let mut globals = Globals::default();
    let mut stack = [Globals::default(); 4];
    let mut depth_pushed = 0;
    let mut locals = Locals::default();
    let mut offsets = [(0u8, 0u32); MAX_IDS];
    let mut ids = 0;
    let mut collections = 0u32;
    let mut in_pointer = false;
    let mut found: Option<PointerLayout> = None;
    let mut at = 0;
    while at < descriptor.len() {
        let prefix = descriptor[at];
        if prefix == 0xFE {
            // A long item: its data size follows; nothing here uses one.
            let size = usize::from(*descriptor.get(at + 1)?);
            at += 3 + size;
            continue;
        }
        let size = match prefix & 3 {
            3 => 4,
            n => usize::from(n),
        };
        let bytes = descriptor.get(at + 1..at + 1 + size)?;
        at += 1 + size;
        let mut raw = [0u8; 4];
        raw[..size].copy_from_slice(bytes);
        let unsigned = u32::from_le_bytes(raw);
        let signed = match size {
            1 => i32::from(raw[0] as i8),
            2 => i32::from(i16::from_le_bytes([raw[0], raw[1]])),
            _ => unsigned as i32,
        };
        match (prefix >> 2 & 3, prefix >> 4) {
            // Main items.
            (0, 0xA) => {
                // Collection: an application collection at the top level starts a
                // device; it is a pointer if its usage says so.
                if collections == 0 {
                    let usage = locals.usage(0, globals.page).unwrap_or(0);
                    in_pointer = unsigned == 1
                        && (usage == PAGE_DESKTOP << 16 | USAGE_MOUSE
                            || usage == PAGE_DESKTOP << 16 | USAGE_POINTER);
                }
                collections += 1;
                locals = Locals::default();
            }
            (0, 0xC) => {
                collections = collections.saturating_sub(1);
                if collections == 0 {
                    in_pointer = false;
                }
                locals = Locals::default();
            }
            (0, 0x8) => {
                // Input: advance this report ID's offset past each field.
                let slot = match offsets[..ids].iter().position(|&(id, _)| id == globals.id) {
                    Some(slot) => slot,
                    None if ids < MAX_IDS => {
                        offsets[ids] = (globals.id, 0);
                        ids += 1;
                        ids - 1
                    }
                    None => return found.filter(|l| l.x.is_some() && l.y.is_some()),
                };
                let variable = unsigned & 0x2 != 0 && unsigned & 0x1 == 0;
                for index in 0..globals.count.min(256) {
                    let offset = offsets[slot].1;
                    offsets[slot].1 += globals.size;
                    let take = variable
                        && in_pointer
                        && found
                            .is_none_or(|f| f.report_id == (globals.id != 0).then_some(globals.id));
                    if !take {
                        continue;
                    }
                    let layout = found.get_or_insert(PointerLayout {
                        report_id: (globals.id != 0).then_some(globals.id),
                        ..PointerLayout::default()
                    });
                    let field = Field {
                        offset,
                        size: globals.size.min(32) as u8,
                        min: globals.min,
                        max: globals.max,
                        relative: unsigned & 0x4 != 0,
                    };
                    match locals.usage(index, globals.page) {
                        Some(u) if u >> 16 == PAGE_BUTTON && (1..=8).contains(&(u & 0xFFFF)) => {
                            layout.buttons[(u & 0xFFFF) as usize - 1] = Some(offset);
                        }
                        Some(u) if u == PAGE_DESKTOP << 16 | USAGE_X => layout.x = Some(field),
                        Some(u) if u == PAGE_DESKTOP << 16 | USAGE_Y => layout.y = Some(field),
                        Some(u) if u == PAGE_DESKTOP << 16 | USAGE_WHEEL => {
                            layout.wheel = Some(field)
                        }
                        _ => {}
                    }
                }
                locals = Locals::default();
            }
            (0, _) => locals = Locals::default(),
            // Global items.
            (1, 0x0) => globals.page = unsigned,
            (1, 0x1) => globals.min = signed,
            // A maximum is read as unsigned when the minimum is not negative.
            (1, 0x2) if globals.min >= 0 => globals.max = unsigned as i32,
            (1, 0x2) => globals.max = signed,
            (1, 0x7) => globals.size = unsigned,
            (1, 0x8) => globals.id = unsigned as u8,
            (1, 0x9) => globals.count = unsigned,
            (1, 0xA) if depth_pushed < stack.len() => {
                stack[depth_pushed] = globals;
                depth_pushed += 1;
            }
            (1, 0xB) if depth_pushed > 0 => {
                depth_pushed -= 1;
                globals = stack[depth_pushed];
            }
            // Local items.
            (2, 0x0) if locals.len < MAX_USAGES => {
                locals.usages[locals.len] = unsigned;
                locals.len += 1;
            }
            (2, 0x1) => locals.min = Some(unsigned),
            (2, 0x2) => locals.max = Some(unsigned),
            _ => {}
        }
    }
    found.filter(|layout| layout.x.is_some() && layout.y.is_some())
}

#[cfg(test)]
mod tests;
