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
//! - An ask is answered only on the console (ADR 0005 §7): while the terminal's
//!   Allow / Deny menu, a script's confirmation or `:apply`'s card waits, the
//!   console takes the screen as Ctrl+Alt+F12 does ([`take_for_answer`]) and
//!   gives it back after ([`give_back`]), so no key typed in a window answers.
//! - Every change of hands ends INPUT_TO_CONSOLE's lease (`input::end_lease`).

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
    /// The console took it for an answer ([`take_for_answer`]): only
    /// [`give_back`] returns it.
    answering: bool,
    /// A claim was refused while `answering`: [`give_back`] sends the screen
    /// event it is waiting for, which the holder's old claim cannot, if the
    /// holder ended meanwhile (the desktop restarted during an ask).
    refused: bool,
    /// The hardware cursor's hot spot.
    cursor: (u32, u32),
}

static STATE: Mutex<State> = Mutex::new(State {
    holder: None,
    showing: None,
    taken_back: false,
    answering: false,
    refused: false,
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

/// One Ctrl+Alt+F12. A press that finds no holder showing and none taken from
/// is ignored, and logged, whether an answer waits or not (the console shows
/// then, and no holder was taken from). One that finds the screen taken from a
/// holder and kept for an answer is ignored silently.
pub fn switch() {
    input::end_lease();
    let mut state = STATE.lock();
    if let Some(grant) = state.showing.take() {
        state.taken_back = true;
        drop(state);
        to_console(grant);
        input::screen(false);
        log!("display: Ctrl+Alt+F12 gave the screen to the console");
    } else if state.taken_back && !state.answering {
        state.taken_back = false;
        drop(state);
        input::screen(true);
        log!("display: Ctrl+Alt+F12 gave the screen back to its holder");
    } else if !state.taken_back {
        // No holder shows and none was taken from: the press does nothing.
        // Said once per press (a keypress, not a hot path), since a press that
        // vanished hid a key race in the release boot.
        drop(state);
        log!("display: Ctrl+Alt+F12 with no holder showing the screen; ignored");
    }
}

/// An answer waits on the console: if the holder shows, the console takes the
/// screen until [`give_back`]. Whether it took it or the console showed already
/// (Ctrl+Alt+F12), the screen stays on the console until then: neither
/// Ctrl+Alt+F12 nor a claim returns it, so no window gets keys meanwhile.
/// Returns whether it took it.
///
/// `publish` makes the question visible (`terminal`'s flag). It runs under
/// this lock once `answering` is set, so a Ctrl+Alt+F12 that comes after the
/// question can be seen also sees the answer pending, and leaves the screen
/// on the console. One just before it may have handed the screen back to a
/// holder that has not claimed it yet: the console keeps it then too, and the
/// answer hands it back (#1505's desk-ask flake).
pub fn take_for_answer(publish: impl FnOnce()) -> bool {
    let mut state = STATE.lock();
    state.answering = true;
    publish();
    if let Some(grant) = state.showing.take() {
        state.taken_back = true;
        drop(state);
        to_console(grant);
        input::screen(false);
        log!("display: the console took the screen for an answer");
        return true;
    }
    // Not showing, not taken back, and a holder: it was given the screen
    // (a second Ctrl+Alt+F12, or an earlier answer) and its claim is still
    // to come. The claim would be refused while the answer waits, so the
    // answer must give the screen back after.
    if state.taken_back || state.holder.is_none() {
        return false;
    }
    state.taken_back = true;
    drop(state);
    input::end_lease();
    input::screen(false);
    log!("display: the console kept a screen handed back, for an answer");
    true
}

/// The answer [`take_for_answer`] waited for came. The holder gets the screen
/// back, as on a second Ctrl+Alt+F12, if `took` says the answer took it; a
/// screen the console had before stays the console's. A claim refused while
/// the answer waited is told to come again (a `Screen` 1 event), unless
/// Ctrl+Alt+F12 took the screen from a live holder: a `desktop` restarted
/// during the ask then gets the screen.
pub fn give_back(took: bool) {
    let mut state = STATE.lock();
    state.answering = false;
    // A screen Ctrl+Alt+F12 took from a live holder stays the console's.
    let retry = core::mem::take(&mut state.refused) && state.showing.is_none() && !state.taken_back;
    let back = took && state.taken_back && state.holder.is_some();
    if back {
        state.taken_back = false;
    }
    drop(state);
    if back {
        input::screen(true);
        log!("display: the answer came; the screen went back to its holder");
    } else if retry {
        // A claim the answer refused is not lost: its holder ended or never
        // had the screen, so no event would tell the new one to try again.
        input::screen(true);
        log!("display: the answer came; a refused claim may try again");
    }
}

/// Whether a claim was refused while the answer waits, for `test=desk-ask`.
pub fn refused_claim() -> bool {
    STATE.lock().refused
}

/// Revokes the holder's `Shm` and repaints the console.
fn to_console(grant: GrantRef) {
    input::end_lease();
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
    input::end_lease();
    crate::ask_box::on_desktop_exit();
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
    // An answer waiting on the console keeps it there, for a new holder too;
    // `give_back` tells the claim to come again.
    if state.answering {
        state.refused = true;
        return Err(Error::Busy);
    }
    if state.showing.is_some()
        || (state.taken_back && state.holder == Some(pid))
        || (holder_alive && state.holder != Some(pid))
    {
        return Err(Error::Busy);
    }
    let (phys, pages) = frames(&surface)?;
    let object = shm::create_kernel_object(phys, pages)?;
    let grant = match grants::create(grants::root(SYSTEM), 0, Life::Session, b"display") {
        Ok(grant) => grant,
        Err(error) => {
            // The back buffer's object goes too: pinned, it would stay for good.
            shm::settle(object);
            shm::collect();
            return Err(error);
        }
    };
    let entry = Entry {
        grant: Some(grant),
        ..Entry::new(
            Kind::Shm,
            Rights::MAP | Rights::READ | Rights::WRITE,
            object,
        )
    };
    let inserted = grants::insert(slot, entry);
    shm::settle(object);
    let handle = match inserted {
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
    // already says the holder has the screen. Nothing here may run after the
    // claim is seen: a clear then would drop a key typed for the holder.
    console::suspend();
    input::end_lease();
    input::clear();
    input::set_taking(true);
    state.holder = Some(pid);
    state.showing = Some(grant);
    state.taken_back = false;
    state.cursor = (surface.width as u32 / 2, surface.height as u32 / 2);
    drop(state);
    let (width_mm, height_mm) = super::size_mm();
    *info = DisplayInfo {
        width: surface.width as u32,
        height: surface.height as u32,
        stride: (surface.stride * 4) as u32,
        format: FORMAT_XRGB8888,
        width_mm,
        height_mm,
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
/// Whether a holder shows on the screen now.
pub fn showing() -> bool {
    STATE.lock().showing.is_some()
}

pub fn shows(slot: usize) -> bool {
    let pid = process::identity(slot).pid;
    let state = STATE.lock();
    state.holder == Some(pid) && state.showing.is_some_and(|g| Kernel.alive(g))
}

/// DISPLAY_FLUSH's `rect` for the process in `slot`, onto a shadowed screen
/// (`display::shadowed`): packed band by band, each band under this module's
/// lock and only while `slot` still shows. Ctrl+Alt+F12 and
/// [`take_for_answer`] take `showing` under the same lock before the console
/// repaints, so a band either finishes before the console has the screen or is
/// never packed: no window pixel lands over the console. A busy display is no
/// error: the next present sends the whole screen.
pub fn present(slot: usize, rect: gfxcore::Rect) -> Result<(), Error> {
    let pid = process::identity(slot).pid;
    let mut shown = true;
    gfxcore::rect::each_band(super::pending(rect), super::BAND_ROWS, |band| {
        let state = STATE.lock();
        shown = state.holder == Some(pid) && state.showing.is_some_and(|g| Kernel.alive(g));
        // Packed with the lock held: the switch waits for this band.
        shown && super::pack_band(band).is_ok()
    });
    match shown {
        true => Ok(()),
        false => Err(Error::Permission),
    }
}

/// The hardware cursor's hot spot, for DISPLAY_CURSOR's image.
pub fn cursor() -> (u32, u32) {
    STATE.lock().cursor
}

pub fn set_cursor_at(at: (u32, u32)) {
    STATE.lock().cursor = at;
}
