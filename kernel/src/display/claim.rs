//! Who holds the screen (ADR 0005 §2): the kernel console, or `desktop`.
//!
//! - DISPLAY_CLAIM lends `desktop` the back buffer as a kernel-owned `Shm` under
//!   a fresh `display` grant node, and the console stops painting. Input then
//!   goes to the kernel's queue (`crate::input`), not to the console.
//! - Ctrl+Alt+F12, or the holder's exit, revokes that node: the holder's mapping
//!   turns into a zero page (ADR 0001 D4), so it keeps running but draws nowhere,
//!   and the console repaints. A second Ctrl+Alt+F12 gives the screen back:
//!   `desktop` gets a `Screen` event and claims it again.
//! - The switch runs on its own kernel thread, since a revoke takes locks and
//!   shoots down TLBs, which the keyboard interrupt that sees the keys cannot.

use super::Surface;
use crate::handle::grant::{self as grants, Kernel};
use crate::{console, input, log, memory, process, shm, threads};
use captable::Entry;
use captable::grant::{GrantRef, Life, Live, SYSTEM};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use kabi::Error;
use kabi::display::{DisplayInfo, FORMAT_XRGB8888};
use kabi::handle::{Kind, Rights};
use mm::PAGE_SIZE;
use spin::Mutex;

struct State {
    /// The process that holds, or held and may take back, the screen.
    holder: Option<u64>,
    /// While it shows: the grant its `Shm` came under.
    showing: Option<GrantRef>,
    /// Ctrl+Alt+F12 took the screen from the holder; the next press gives it back.
    taken_back: bool,
    /// The hardware cursor's hot spot.
    cursor: (u32, u32),
}

static STATE: Mutex<State> = Mutex::new(State {
    holder: None,
    showing: None,
    taken_back: false,
    cursor: (0, 0),
});
static SWITCH: AtomicBool = AtomicBool::new(false);
/// Flushes the holder has made since boot, for the cases that wait for its
/// first frame and for the screen to keep still.
static FLUSHES: AtomicU64 = AtomicU64::new(0);
const NO_THREAD: usize = usize::MAX;
static SWITCHER: AtomicUsize = AtomicUsize::new(NO_THREAD);

/// Starts the thread that switches the screen on Ctrl+Alt+F12.
/// Counts a flush by the holder.
pub fn flushed() {
    FLUSHES.fetch_add(1, Ordering::Relaxed);
}

/// The holder's flushes since boot.
pub fn flushes() -> u64 {
    FLUSHES.load(Ordering::Relaxed)
}

pub fn init() -> Result<(), &'static str> {
    let tid = threads::spawn("display-switch", switcher, 0)?;
    SWITCHER.store(tid, Ordering::Release);
    Ok(())
}

/// Ctrl+Alt+F12 was pressed: called from the keyboard path, in interrupt
/// context. Returns the thread to wake.
pub fn request_switch() -> Option<usize> {
    SWITCH.store(true, Ordering::Release);
    match SWITCHER.load(Ordering::Acquire) {
        NO_THREAD => None,
        tid => Some(tid),
    }
}

fn switcher(_: usize) {
    loop {
        threads::block_unless(|| SWITCH.load(Ordering::Acquire));
        if SWITCH.swap(false, Ordering::AcqRel) {
            switch();
        }
    }
}

/// One Ctrl+Alt+F12.
pub fn switch() {
    let mut state = STATE.lock();
    if let Some(grant) = state.showing.take() {
        state.taken_back = true;
        drop(state);
        to_console(grant);
        input::screen(false);
        log!("display: Ctrl+Alt+F12 gave the screen to the console");
    } else if state.taken_back {
        state.taken_back = false;
        drop(state);
        input::screen(true);
        log!("display: Ctrl+Alt+F12 gave the screen back to its holder");
    }
}

/// Revokes the holder's `Shm` and repaints the console.
fn to_console(grant: GrantRef) {
    input::set_taking(false);
    input::clear();
    let _ = grants::revoke(grant);
    console::resume();
}

/// The holder `pid` ended: the console takes the screen for good.
pub fn on_exit(pid: u64) {
    let mut state = STATE.lock();
    if state.holder != Some(pid) {
        return;
    }
    state.holder = None;
    state.taken_back = false;
    if let Some(grant) = state.showing.take() {
        drop(state);
        to_console(grant);
        log!("display: its holder ended; the console has the screen");
    }
}

/// DISPLAY_CLAIM for the process in `slot`: the `Shm` handle, and the back
/// buffer's shape in `info`.
pub fn claim(slot: usize, info: &mut DisplayInfo) -> Result<u64, Error> {
    let surface = super::surface().ok_or(Error::NotFound)?;
    let pid = process::identity(slot).pid;
    let mut state = STATE.lock();
    let holder_alive = state.holder.is_some_and(|h| process::name_of(h).is_some());
    if state.showing.is_some()
        || (state.taken_back && state.holder == Some(pid))
        || (holder_alive && state.holder != Some(pid))
    {
        return Err(Error::Busy);
    }
    let (phys, pages) = frames(&surface)?;
    let object = shm::create_kernel_object(phys, pages)?;
    let grant = grants::create(grants::root(SYSTEM), 0, Life::Session, b"display")?;
    let entry = Entry {
        grant: Some(grant),
        ..Entry::new(
            Kind::Shm,
            Rights::MAP | Rights::READ | Rights::WRITE,
            object,
        )
    };
    let handle = match grants::insert(slot, entry) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = grants::revoke(grant);
            shm::collect();
            return Err(error);
        }
    };
    // The console stops painting, and input becomes the holder's, before the
    // claim can be seen: otherwise, on another CPU, the console could still
    // draw into the buffer the holder now owns, or take a key, while `shows`
    // already says the holder has the screen.
    console::suspend();
    input::clear();
    input::set_taking(true);
    state.holder = Some(pid);
    state.showing = Some(grant);
    state.taken_back = false;
    state.cursor = (surface.width as u32 / 2, surface.height as u32 / 2);
    drop(state);
    *info = DisplayInfo {
        width: surface.width as u32,
        height: surface.height as u32,
        stride: (surface.stride * 4) as u32,
        format: FORMAT_XRGB8888,
        width_mm: 0,
        height_mm: 0,
        pages: pages as u32,
        cursor: u32::from(super::kind() == Some(super::Kind::VirtioGpu)),
    };
    log!(
        "display: pid {pid} claimed the screen, {}x{}",
        surface.width,
        surface.height
    );
    Ok(u64::from(handle.0))
}

/// The back buffer's first frame and its page count. Both backends' buffers
/// are physically contiguous and reached through the direct map.
fn frames(surface: &Surface) -> Result<(u64, u64), Error> {
    let virt = surface.base.as_ptr() as u64;
    let phys = virt.checked_sub(memory::hhdm()).ok_or(Error::Invalid)?;
    if phys % PAGE_SIZE != 0 {
        return Err(Error::Invalid);
    }
    let bytes = (surface.stride * 4 * surface.height) as u64;
    Ok((phys, bytes.div_ceil(PAGE_SIZE)))
}

/// Whether the process in `slot` shows on the screen now.
pub fn shows(slot: usize) -> bool {
    let pid = process::identity(slot).pid;
    let state = STATE.lock();
    state.holder == Some(pid) && state.showing.is_some_and(|g| Kernel.alive(g))
}

/// The hardware cursor's hot spot, for DISPLAY_CURSOR's image.
pub fn cursor() -> (u32, u32) {
    STATE.lock().cursor
}

pub fn set_cursor_at(at: (u32, u32)) {
    STATE.lock().cursor = at;
}
