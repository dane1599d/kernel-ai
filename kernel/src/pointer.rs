//! The mouse pointer. Mouse bytes from the i8042 ring go through
//! `termcore::mouse`; the pointer starts at the top-left corner, stays on the
//! framebuffer, and sits over the console cell its pixel falls in. With
//! `pointerlog` on the command line each move and button press is logged.
//!
//! Every pointing device, PS/2 or USB, also queues its reports as [`Event`]s,
//! which `:mouse` and the USB self-test read.

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
    let (cell_width, cell_height) = console::cell();
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
    drop(pointer);
    let held = |button, bit| if packet.buttons.held(button) { bit } else { 0 };
    push(Event {
        source: Source::Ps2,
        motion: Motion::Relative {
            dx: i32::from(packet.dx),
            dy: -i32::from(packet.dy),
        },
        buttons: held(Button::Left, 1) | held(Button::Right, 2) | held(Button::Middle, 4),
    });
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

/// Where a pointer event came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Ps2,
    UsbMouse,
    UsbTablet,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Ps2 => "PS/2 mouse",
            Source::UsbMouse => "USB mouse",
            Source::UsbTablet => "USB tablet",
        }
    }
}

/// How the pointer moved: by a distance (screen-down positive), or to a
/// position out of a device range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Relative {
        dx: i32,
        dy: i32,
    },
    Absolute {
        x: u32,
        y: u32,
        x_max: u32,
        y_max: u32,
    },
}

/// One pointer report, from any pointing device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub source: Source,
    pub motion: Motion,
    /// Bit 0 left, bit 1 right, bit 2 middle.
    pub buttons: u8,
}

/// Events kept for a reader; older ones are dropped first.
const QUEUE: usize = 64;

/// The event queue, and where the last event left the pointer in pixels.
struct Queue {
    events: [Option<Event>; QUEUE],
    head: usize,
    len: usize,
    last: Option<Event>,
    pixel: (u32, u32),
}

static EVENTS: Mutex<Queue> = Mutex::new(Queue {
    events: [None; QUEUE],
    head: 0,
    len: 0,
    last: None,
    pixel: (0, 0),
});

/// Queues `event`. Safe from interrupt handlers: the lock is only ever held with
/// interrupts off.
pub fn push(event: Event) {
    let (width, height) = console::pixels().unwrap_or((1, 1));
    let (width, height) = (width.max(1) as u64, height.max(1) as u64);
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut queue = EVENTS.lock();
        queue.pixel = match event.motion {
            Motion::Absolute { x, y, x_max, y_max } => (
                (u64::from(x) * width / (u64::from(x_max) + 1)) as u32,
                (u64::from(y) * height / (u64::from(y_max) + 1)) as u32,
            ),
            Motion::Relative { dx, dy } => {
                let step = |at: u32, by: i32, size: u64| {
                    (i64::from(at) + i64::from(by)).clamp(0, size as i64 - 1) as u32
                };
                (
                    step(queue.pixel.0, dx, width),
                    step(queue.pixel.1, dy, height),
                )
            }
        };
        let at = (queue.head + queue.len) % QUEUE;
        queue.events[at] = Some(event);
        if queue.len == QUEUE {
            queue.head = (queue.head + 1) % QUEUE;
        } else {
            queue.len += 1;
        }
        queue.last = Some(event);
    });
}

/// The oldest queued event, if any.
pub fn next_event() -> Option<Event> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut queue = EVENTS.lock();
        if queue.len == 0 {
            return None;
        }
        let head = queue.head;
        queue.head = (head + 1) % QUEUE;
        queue.len -= 1;
        queue.events[head].take()
    })
}

/// The `:mouse` command: the last pointer event and where it left the pointer.
pub fn show() {
    let (last, pixel) = x86_64::instructions::interrupts::without_interrupts(|| {
        let queue = EVENTS.lock();
        (queue.last, queue.pixel)
    });
    let Some(event) = last else {
        log!("mouse: no pointer input yet");
        return;
    };
    let motion = match event.motion {
        Motion::Relative { dx, dy } => alloc::format!("moved {dx} {dy}"),
        Motion::Absolute { x, y, x_max, y_max } => {
            alloc::format!("at {x} {y} of {x_max} {y_max}")
        }
    };
    log!(
        "mouse: {} {motion}, buttons {:03b}; pointer at pixel {} {}",
        event.source.name(),
        event.buttons,
        pixel.0,
        pixel.1
    );
}
