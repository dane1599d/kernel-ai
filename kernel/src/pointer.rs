//! The mouse pointer. Mouse bytes from the i8042 ring go through
//! `termcore::mouse`; the pointer starts at the top-left corner, stays on the
//! framebuffer, and sits over the console cell its pixel falls in. With
//! `pointerlog` on the command line each move and button press is logged.

use crate::{console, hpet, i8042, keyboard, log};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use termcore::mouse::{Button, Buttons, Cursor, Decoder};

static LOG: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct Click {
    button: Button,
    /// The console cell as (row, column).
    cell: (u32, u32),
    /// Ctrl was held when the press came in.
    ctrl: bool,
}

struct Pointer {
    decoder: Decoder,
    /// Made on the first packet, once the console knows the screen size.
    cursor: Option<Cursor>,
    buttons: Buttons,
    /// The first clicks since boot, for the self-test.
    clicks: [Option<Click>; 2],
}

static POINTER: Mutex<Pointer> = Mutex::new(Pointer {
    decoder: Decoder::new(),
    cursor: None,
    buttons: Buttons::NONE,
    clicks: [None; 2],
});

pub fn configure(cmdline: &str) {
    let on = cmdline.split_whitespace().any(|arg| arg == "pointerlog");
    LOG.store(on, Ordering::Relaxed);
}

fn screen_cursor() -> Cursor {
    let (width, height) = console::pixels().unwrap_or((usize::MAX, usize::MAX));
    let size = |pixels: usize| u32::try_from(pixels).unwrap_or(u32::MAX);
    Cursor::new(size(width), size(height))
}

/// Takes one mouse byte. `ctrl` says whether Ctrl was held at its place in the ring.
pub fn on_byte(byte: u8, ctrl: bool) {
    let log = LOG.load(Ordering::Relaxed);
    let (cell_width, cell_height) = console::CELL;
    let mut pointer = POINTER.lock();
    let Some(packet) = pointer.decoder.push(byte) else {
        return;
    };
    let cursor = pointer.cursor.get_or_insert_with(screen_cursor);
    let moved = cursor.apply(&packet);
    let (x, y) = (cursor.x(), cursor.y());
    let cell = cursor.cell(cell_width as u32, cell_height as u32);
    if moved && log {
        log!("pointer: at {x} {y} cell {} {}", cell.0, cell.1);
    }
    for button in packet.buttons.pressed_since(pointer.buttons) {
        if log {
            let held = if ctrl { " ctrl" } else { "" };
            log!("pointer: click {}{held}", button.number());
        }
        if let Some(free) = pointer.clicks.iter_mut().find(|c| c.is_none()) {
            *free = Some(Click { button, cell, ctrl });
        }
    }
    pointer.buttons = packet.buttons;
}

/// M9d pointer spike: the harness moves the mouse right 38 and down 40 and clicks,
/// then clicks again while it holds Ctrl. With 7 by 16 pixel cells the first click
/// is on row 2, column 5, without Ctrl; the second click carries Ctrl.
pub fn self_test() -> Result<(), &'static str> {
    const TIMEOUT_NS: u64 = 30_000_000_000;
    const FIRST_CELL: (u32, u32) = (2, 5);
    if !i8042::has_mouse() {
        return Err("pointer: no PS/2 mouse answered at boot");
    }
    log!("pointer: ready for keys (a move and a click, then a click with Ctrl held)");
    let mut decoder = keyboard::Decoder::new();
    let start = hpet::nanos();
    let (first, second) = loop {
        decoder.discard_pending();
        if let [Some(first), Some(second)] = POINTER.lock().clicks {
            break (first, second);
        }
        if hpet::nanos() - start > TIMEOUT_NS {
            return Err("pointer: two clicks did not arrive within 30 s");
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    };
    log!(
        "pointer: clicks on cell {} {} (ctrl {}) and cell {} {} (ctrl {})",
        first.cell.0,
        first.cell.1,
        first.ctrl,
        second.cell.0,
        second.cell.1,
        second.ctrl
    );
    if first.button != Button::Left || second.button != Button::Left {
        return Err("pointer: a click was not the left button");
    }
    if first.cell != FIRST_CELL {
        return Err("pointer: the first click was not on row 2, column 5");
    }
    if first.ctrl || !second.ctrl {
        return Err("pointer: Ctrl was not held for the second click alone");
    }
    Ok(())
}
