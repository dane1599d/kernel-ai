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

    /// Whether the kernel lent the keys to the terminal, as `desktop` knows.
    pub fn lent(&self) -> bool {
        self.leased_at.is_some()
    }

    /// The lease is over: `desktop` ended it ([`Step::End`]), or a pointer
    /// button went down and the kernel ended it on the press, so the next step
    /// asks again at once instead of after [`ASK_NS`].
    pub fn forget(&mut self) {
        *self = Self::new();
    }
}

/// The desk's keys for one kernel key event, `usage` and `value`
/// (`kabi::input::Kind::Key`). A Super tap the kernel saw
/// ([`kabi::input::TAP`]) is Super down and up, which opens Start. Any
/// other Super event is dropped, lease or not: only the kernel sees every
/// key (PrintScreen and Pause reach no one), so a bare release must never
/// pass for a tap, and under a lease Super belongs to the terminal.
pub fn desk_keys(usage: u16, value: i32) -> alloc::vec::Vec<(u16, bool)> {
    let gui = matches!(
        usage,
        crate::desk::usage::LEFT_GUI | crate::desk::usage::RIGHT_GUI
    );
    match (gui, value) {
        (true, kabi::input::TAP) => alloc::vec![(usage, true), (usage, false)],
        (true, _) => alloc::vec::Vec::new(),
        (_, 1 | 2) => alloc::vec![(usage, true)],
        _ => alloc::vec![(usage, false)],
    }
}

/// What `desktop` does with one kernel key event ([`on_key`]).
#[derive(Debug, PartialEq, Eq)]
pub struct KeyStep {
    /// The event ended a lease, as `desktop` knew it: it forgot the lease, and
    /// ends it on the kernel's side too (`input_to_console(false)`), since a
    /// tap the kernel saw unleased may arrive after the kernel lent the keys
    /// for a new focus.
    pub end_lease: bool,
    /// A Super press that is not a tap: it opens nothing, but on an ask card
    /// it is any key but `y`, which disarms it ([`crate::desk::Desk::super_pressed`]).
    pub super_press: bool,
    /// The keys for the desk ([`desk_keys`]).
    pub keys: alloc::vec::Vec<(u16, bool)>,
}

/// One kernel key event, `usage` and `value`, with `lease` as `desktop`
/// knows it: forgets the lease when the event ended it, and says what else
/// to do.
pub fn on_key(lease: &mut Lease, usage: u16, value: i32) -> KeyStep {
    let end_lease = ends_lease(usage, value, lease.lent());
    if end_lease {
        lease.forget();
    }
    let gui = matches!(
        usage,
        crate::desk::usage::LEFT_GUI | crate::desk::usage::RIGHT_GUI
    );
    KeyStep {
        end_lease,
        super_press: gui && value == 1,
        keys: desk_keys(usage, value),
    }
}

/// Whether the kernel ended the lease for kernel key event `usage`,
/// `value`, while `lent`: a Super tap or any Tab or Esc press under the
/// lease (Alt+Tab's and Ctrl+Esc's, which reach `desktop` only as chords;
/// a lone one never reaches it). `desktop` forgets the lease then, so it
/// asks again only for a terminal that has the focus. It reads no Ctrl or
/// Alt state of its own, which goes stale with both sides held.
pub fn ends_lease(usage: u16, value: i32, lent: bool) -> bool {
    const TAB: u16 = 0x2B;
    const ESCAPE: u16 = 0x29;
    match (usage, value) {
        (_, kabi::input::TAP) | (TAB | ESCAPE, 1) => lent,
        _ => false,
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;
    use crate::desk::usage::LEFT_GUI;
    use crate::desk::{Desk, Effect, Focus, Input};
    use crate::geom::Size;

    /// Feeds `events` (usage, value) to a desk with a window focused: the
    /// effects.
    fn feed(events: &[(u16, i32)]) -> (Desk, alloc::vec::Vec<Effect>) {
        let mut desk = Desk::new(1280, 800);
        desk.open(1, "term", "Terminal", Size { w: 300, h: 200 }, true);
        let mut effects = alloc::vec::Vec::new();
        for &(usage, value) in events {
            for (usage, down) in desk_keys(usage, value) {
                effects.extend(desk.input(Input::Key { usage, down }));
            }
        }
        (desk, effects)
    }

    /// Exactly what desktop sees of Super+L under a lease: nothing of Super
    /// (the kernel keeps it the terminal's); even a stray bare Super event
    /// is dropped, so no Start opens and the terminal keeps the keys.
    #[test]
    fn super_with_a_letter_under_a_lease_opens_nothing() {
        let (desk, effects) = feed(&[]);
        assert!(effects.is_empty());
        assert!(desk.start().is_none());
        let (desk, effects) = feed(&[(LEFT_GUI, 1), (LEFT_GUI, 0)]);
        assert!(desk.start().is_none(), "{effects:?}");
        assert!(matches!(desk.focus(), Focus::Window(_)));
    }

    /// The lease is over after a tap and a chord's press, never a release
    /// or a key alone.
    #[test]
    fn the_lease_ends_on_a_tap_or_a_chords_press() {
        assert!(ends_lease(LEFT_GUI, kabi::input::TAP, true));
        assert!(ends_lease(0x2B, 1, true), "Alt+Tab's press");
        assert!(ends_lease(0x29, 1, true), "Ctrl+Esc's press");
        assert!(!ends_lease(0x2B, 0, true), "a release");
        assert!(!ends_lease(0x2B, 1, false), "no lease to end");
        assert!(!ends_lease(0x04, 1, true), "a letter");
        assert!(!ends_lease(LEFT_GUI, 0, true), "a bare release");
        assert!(
            !ends_lease(LEFT_GUI, kabi::input::TAP, false),
            "a tap, no lease"
        );
    }

    /// A tap that reaches `desktop` while it holds a lease, even one the
    /// kernel saw unleased just before it lent the keys for a new focus,
    /// ends the lease on both sides; Super's press alone is reported, and
    /// its keys are none.
    #[test]
    fn a_tap_while_lent_ends_the_lease_on_both_sides() {
        let mut lease = Lease::new();
        lease.asked(crate::desk::WindowId(1), true, 0);
        assert!(lease.lent());
        let press = on_key(&mut lease, LEFT_GUI, 1);
        assert_eq!(
            press,
            KeyStep {
                end_lease: false,
                super_press: true,
                keys: alloc::vec::Vec::new()
            }
        );
        assert!(lease.lent());
        let tap = on_key(&mut lease, LEFT_GUI, kabi::input::TAP);
        assert!(tap.end_lease, "desktop must end the kernel's lease too");
        assert!(!tap.super_press);
        assert!(!lease.lent());
        // Not lent: nothing to end.
        assert!(!on_key(&mut lease, LEFT_GUI, kabi::input::TAP).end_lease);
        assert!(!on_key(&mut lease, LEFT_GUI, 2).super_press, "a repeat");
    }

    /// Only the kernel's tap opens Start. Without a lease, Super pressed and
    /// released around PrintScreen or Pause reaches `desktop` as a bare press
    /// and release (the kernel keeps those keys), which open nothing.
    #[test]
    fn only_the_kernels_tap_opens_start() {
        let (desk, _) = feed(&[(LEFT_GUI, kabi::input::TAP)]);
        assert!(desk.start().is_some());
        let (desk, effects) = feed(&[(LEFT_GUI, 1), (LEFT_GUI, 0)]);
        assert!(desk.start().is_none(), "Super+PrintScreen: {effects:?}");
        // A repeat is a press; anything else is a release.
        assert_eq!(desk_keys(0x04, 2), [(0x04, true)]);
        assert_eq!(desk_keys(0x04, 0), [(0x04, false)]);
    }
}
