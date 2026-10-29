//! The kernel's one input queue (ADR 0005 §2, ADR 0007): 16-byte
//! [`InputEvent`]s with HID usage codes, which `desktop` drains with INPUT_READ.
//!
//! - Every keyboard byte (PS/2, or a USB keyboard's keys as set 1 scancodes)
//!   passes [`on_byte`] as the i8042 ring takes it, in interrupt context on the
//!   CPU that takes IRQ1. It keeps the Ctrl and Alt state, so Ctrl+Alt+F12 is
//!   seen whoever holds the screen: that press never reaches anyone, and asks the
//!   display thread to switch (`display::claim`).
//! - While `desktop` holds the screen, keys and PS/2 mouse packets become events
//!   here instead of going to the console, and USB pointer reports come through
//!   [`on_pointer`]. While the console holds it, they go to the console as
//!   before, and the queue gets only [`Kind::Screen`] when the screen changes
//!   hands.
//! - INPUT_TO_CONSOLE leases the keys back to the console's terminal while
//!   `desktop` shows, for the terminal window (P3 task 8): about a second at a
//!   time, renewed by `desktop` while that window has focus. The lease ends on
//!   any pointer button, on Alt+Tab, on Ctrl+Alt+F12, and whenever the screen
//!   changes hands ([`end_lease`]). Alt and Alt+Tab always reach the queue, so
//!   `desktop` can always switch away. Keys change hands only between scancode
//!   sequences, and the console's ring is emptied each time they do.

use crate::{hpet, i8042, threads};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use kabi::input::{InputEvent, Kind, Set1, usage};
use spin::Mutex;
use termcore::mouse::{Button, Buttons, Decoder};

/// Button bits, as `pointer::Event` has them: 0 left, 1 right, 2 middle.
const BITS: [(Button, u8, u16); 3] = [
    (Button::Left, 1, 1),
    (Button::Right, 2, 2),
    (Button::Middle, 4, 3),
];

/// Events kept for `desktop`; the oldest goes when it falls this far behind.
const QUEUE: usize = 256;
const NO_WAITER: usize = usize::MAX;

struct Queue {
    events: [Option<InputEvent>; QUEUE],
    head: usize,
    len: usize,
    dropped: u64,
}

static QUEUE_LOCK: Mutex<Queue> = Mutex::new(Queue {
    events: [None; QUEUE],
    head: 0,
    len: 0,
    dropped: 0,
});
static WAITER: AtomicUsize = AtomicUsize::new(NO_WAITER);
/// Threads to wake once the interrupt that queued their work has ended
/// ([`wake_pending`]): the reader, and the display switch.
static PENDING: [AtomicUsize; 2] = [const { AtomicUsize::new(NO_WAITER) }; 2];
/// Set while `desktop` holds the screen: input is its, not the console's.
static TAKING: AtomicBool = AtomicBool::new(false);
/// While `desktop` holds the screen, keys go to the console until this HPET
/// time (INPUT_TO_CONSOLE); 0 for no lease.
static LEASE_UNTIL_NS: AtomicU64 = AtomicU64::new(0);
/// How long one INPUT_TO_CONSOLE lends the keys.
const LEASE_NS: u64 = 1_000_000_000;

/// The interrupt side's decoders: one producer, the CPU that takes IRQ1.
struct Decoders {
    keys: Set1,
    mouse: Decoder,
    /// Keys go to the console under a lease: changed only between sequences.
    leased: bool,
}

/// Buttons held, as [`BITS`], from whichever pointer reported last.
static BUTTONS: AtomicU8 = AtomicU8::new(0);
/// Key events queued for `desktop` since boot.
static KEYS: AtomicU64 = AtomicU64::new(0);

static DECODERS: Mutex<Decoders> = Mutex::new(Decoders {
    keys: Set1::new(),
    mouse: Decoder::new(),
    leased: false,
});

/// Whether input goes to `desktop` now.
pub fn taking() -> bool {
    TAKING.load(Ordering::Acquire)
}

/// `desktop` holds the screen (`true`) or the console does.
pub fn set_taking(on: bool) {
    TAKING.store(on, Ordering::Release);
}

/// INPUT_TO_CONSOLE: lends the keys to the console for [`LEASE_NS`] from now,
/// or (`on` false) ends the lease.
pub fn lease(on: bool) {
    let until = match on {
        true => hpet::nanos().saturating_add(LEASE_NS).max(1),
        false => 0,
    };
    LEASE_UNTIL_NS.store(until, Ordering::Release);
}

/// Ends the lease: the keys go back to `desktop`'s queue at the next sequence
/// boundary. Safe in interrupt context.
pub fn end_lease() {
    LEASE_UNTIL_NS.store(0, Ordering::Release);
}

/// Whether a lease lends the keys to the console now.
fn leased_now() -> bool {
    let until = LEASE_UNTIL_NS.load(Ordering::Acquire);
    until != 0 && hpet::nanos() < until
}

/// What the i8042 ring does with a byte: keep it for the console, or not
/// (it became an event here, or was Ctrl+Alt+F12).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Console,
    Taken,
}

/// A byte from the keyboard port (`aux` false) or the mouse port, in interrupt
/// context. Returns where it goes. A Ctrl+Alt+F12 press asks the display thread
/// to switch; the interrupt handler wakes it, and the reader, after its EOI
/// ([`wake_pending`]).
pub fn on_byte(byte: u8, aux: bool) -> Route {
    let (route, switch) = route(byte, aux);
    if switch && let Some(tid) = crate::display::claim::request_switch() {
        PENDING[1].store(tid, Ordering::Release);
    }
    route
}

fn route(byte: u8, aux: bool) -> (Route, bool) {
    let mut decoders = match DECODERS.try_lock() {
        Some(d) => d,
        // Only one CPU feeds the ring; a held lock means a reader of the
        // decoders is mid-call on this CPU, which never happens in practice.
        None => return (Route::Console, false),
    };
    let taking = taking();
    if aux {
        if !taking {
            return (Route::Console, false);
        }
        if let Some(packet) = decoders.mouse.push(byte) {
            drop(decoders);
            let now = mask(packet.buttons);
            let before = BUTTONS.swap(now, Ordering::Relaxed);
            relative(i32::from(packet.dx), -i32::from(packet.dy));
            buttons(before, now);
        }
        return (Route::Taken, false);
    }
    // Keys change hands only between sequences, never inside an E0 or E1 one,
    // and the console's ring is emptied each time.
    if decoders.keys.at_boundary() {
        let leased = taking && leased_now();
        if leased != decoders.leased {
            decoders.leased = leased;
            i8042::discard_queued();
        }
    }
    let to_console = !taking || decoders.leased;
    let Some((code, down)) = decoders.keys.feed(byte) else {
        // A prefix byte: the console needs it only if it is getting keys.
        return (
            if to_console {
                Route::Console
            } else {
                Route::Taken
            },
            false,
        );
    };
    if code == usage::F12 && decoders.keys.ctrl_alt() {
        // The press switches; its release is dropped too, so neither side sees F12.
        end_lease();
        return (Route::Taken, down);
    }
    let alt = matches!(code, usage::LEFT_ALT | usage::RIGHT_ALT);
    let alt_tab = code == usage::TAB && decoders.keys.alt();
    drop(decoders);
    if !taking {
        return (Route::Console, false);
    }
    if alt_tab {
        // `desktop`'s own chord: never the console's, and it ends a lease.
        end_lease();
        push(Kind::Key, code, i32::from(down));
        return (Route::Taken, false);
    }
    if to_console {
        // Alt goes to both, so `desktop` sees an Alt+Tab whole.
        if alt {
            push(Kind::Key, code, i32::from(down));
        }
        return (Route::Console, false);
    }
    push(Kind::Key, code, i32::from(down));
    KEYS.fetch_add(1, Ordering::Relaxed);
    (Route::Taken, false)
}

/// A USB pointer report while `desktop` holds the screen: `motion` relative
/// (dx, dy) or absolute (x, y within 0..=max), buttons as bits 0 left, 1 right,
/// 2 middle.
pub fn on_pointer(motion: Motion, buttons_now: u8) {
    if !taking() {
        return;
    }
    match motion {
        Motion::Relative { dx, dy } => relative(dx, dy),
        Motion::Absolute { x, y, x_max, y_max } => {
            let scale = |v: u32, max: u32| (u64::from(v) * 32767 / u64::from(max.max(1))) as i32;
            push(Kind::Abs, 0, scale(x, x_max));
            push(Kind::Abs, 1, scale(y, y_max));
        }
    }
    let now = buttons_now & 7;
    buttons(BUTTONS.swap(now, Ordering::Relaxed), now);
}

/// A pointer's motion, as `pointer::Motion` has it.
#[derive(Clone, Copy)]
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

fn relative(dx: i32, dy: i32) {
    if dx != 0 {
        push(Kind::Rel, 0, dx);
    }
    if dy != 0 {
        push(Kind::Rel, 1, dy);
    }
}

fn mask(buttons: Buttons) -> u8 {
    BITS.iter()
        .filter(|(button, ..)| buttons.held(*button))
        .fold(0, |mask, (_, bit, _)| mask | bit)
}

/// Button events for what changed, with HID button usages (1 left, 2 right,
/// 3 middle). A press ends a lease: a click may move the focus.
fn buttons(before: u8, now: u8) {
    if now & !before != 0 {
        end_lease();
    }
    for (_, bit, code) in BITS {
        if before & bit != now & bit {
            push(Kind::Button, code, i32::from(now & bit != 0));
        }
    }
}

/// Tells `desktop` the screen changed hands: 0 to the console, 1 back. Called
/// from a thread, not an interrupt.
pub fn screen(returned: bool) {
    push(Kind::Screen, 0, i32::from(returned));
    x86_64::instructions::interrupts::without_interrupts(wake_pending);
}

/// Wakes the threads queued input asked for. Interrupts must be off: the
/// interrupt handlers call it after their EOI.
pub fn wake_pending() {
    for pending in &PENDING {
        match pending.swap(NO_WAITER, Ordering::AcqRel) {
            NO_WAITER => {}
            tid => threads::wake(tid),
        }
    }
}

/// Queues an event, dropping the oldest when `desktop` is far behind, and marks
/// its reader for [`wake_pending`]. Safe in interrupt context.
fn push(kind: Kind, code: u16, value: i32) {
    let event = InputEvent {
        time_ns: hpet::nanos(),
        kind,
        code,
        value,
    };
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut queue = QUEUE_LOCK.lock();
        if queue.len == QUEUE {
            queue.head = (queue.head + 1) % QUEUE;
            queue.len -= 1;
            queue.dropped += 1;
        }
        let at = (queue.head + queue.len) % QUEUE;
        queue.events[at] = Some(event);
        queue.len += 1;
    });
    match WAITER.swap(NO_WAITER, Ordering::AcqRel) {
        NO_WAITER => {}
        tid => PENDING[0].store(tid, Ordering::Release),
    }
}

fn has_events() -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| QUEUE_LOCK.lock().len > 0)
}

/// Whether events wait in the queue, unread by `desktop`.
pub fn pending() -> bool {
    has_events()
}

/// Takes up to `out.len()` events, waiting up to `timeout_ms` for the first.
pub fn read(out: &mut [InputEvent], timeout_ms: u64) -> usize {
    let deadline = hpet::nanos().saturating_add(timeout_ms.saturating_mul(1_000_000));
    loop {
        let taken = x86_64::instructions::interrupts::without_interrupts(|| {
            let mut queue = QUEUE_LOCK.lock();
            let n = queue.len.min(out.len());
            for slot in out.iter_mut().take(n) {
                let at = queue.head;
                *slot = queue.events[at].take().unwrap_or(InputEvent {
                    time_ns: 0,
                    kind: Kind::Screen,
                    code: 0,
                    value: 0,
                });
                queue.head = (queue.head + 1) % QUEUE;
                queue.len -= 1;
            }
            n
        });
        if taken > 0 || out.is_empty() || hpet::nanos() >= deadline {
            return taken;
        }
        // Stored before the check in block_until, so a push either sees the
        // waiter or its event is seen here.
        WAITER.store(threads::current(), Ordering::SeqCst);
        threads::sleep_until_unless(deadline, has_events);
    }
}

/// Whether a lease lends the keys to the console now.
pub fn leased() -> bool {
    leased_now()
}

/// Key events queued for `desktop` since boot.
pub fn keys() -> u64 {
    KEYS.load(Ordering::Relaxed)
}

/// Events dropped because `desktop` fell behind.
pub fn dropped() -> u64 {
    x86_64::instructions::interrupts::without_interrupts(|| QUEUE_LOCK.lock().dropped)
}

/// Clears the queue, when the screen changes hands, so `desktop` never acts on
/// keys typed for the console.
pub fn clear() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut queue = QUEUE_LOCK.lock();
        queue.head = 0;
        queue.len = 0;
        queue.events = [None; QUEUE];
    });
}
