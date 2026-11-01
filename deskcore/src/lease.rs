//! `desktop`'s side of the keyboard lease (INPUT_TO_CONSOLE, P3 task 8): when
//! to ask the kernel to lend the keys to its terminal, and when to end that.
//! The kernel decides which window is the terminal's, and ends a lease itself
//! on a click, Alt+Tab or the screen changing hands; no timer ends it.

use crate::desk::WindowId;

/// How often the lease is asked for again while the window has focus: this
/// only restores a lease the kernel ended where `desktop` did not see it.
pub const ASK_NS: u64 = 250_000_000;

/// What `desktop` does about the lease now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing.
    Stay,
    /// Asks the kernel to lend the keys for the focused window.
    Ask,
    /// Ends the lease: no window has focus.
    End,
}

/// The lease as `desktop` knows it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lease {
    /// When the keys were last lent, while they are.
    leased_at: Option<u64>,
    /// The focused window, and whether the kernel lent the keys for it.
    asked: Option<(WindowId, bool)>,
}

/// What came of an [`Step::Ask`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lent {
    /// The keys were not lent before: the desk forgets the keys it holds down,
    /// as their releases go to the console now.
    pub new: bool,
    /// Whether the keys changed hands, and to the terminal (`true`) or back.
    pub moved: Option<bool>,
}

impl Lease {
    pub const fn new() -> Self {
        Self {
            leased_at: None,
            asked: None,
        }
    }

    /// The step for `focused`, the window with focus (none minimized), at `now`.
    pub fn step(&self, focused: Option<WindowId>, now: u64) -> Step {
        let Some(id) = focused else {
            return match self.leased_at {
                Some(_) => Step::End,
                None => Step::Stay,
            };
        };
        let fresh = self
            .leased_at
            .is_some_and(|at| now.saturating_sub(at) < ASK_NS);
        match self.asked {
            // Not the terminal: asked once for this focus.
            Some((asked, false)) if asked == id => Step::Stay,
            Some((asked, true)) if asked == id && fresh => Step::Stay,
            _ => Step::Ask,
        }
    }

    /// The kernel answered an ask for `id` at `now`.
    pub fn asked(&mut self, id: WindowId, lent: bool, now: u64) -> Lent {
        let was = self.leased_at.is_some();
        self.asked = Some((id, lent));
        self.leased_at = lent.then_some(now);
        Lent {
            new: lent && !was,
            moved: (lent != was).then_some(lent),
        }
    }

    /// The lease is over: `desktop` ended it ([`Step::End`]), or a pointer
    /// button went down and the kernel ended it on the press, so the next step
    /// asks again at once instead of after [`ASK_NS`].
    pub fn forget(&mut self) {
        *self = Self::new();
    }
}
