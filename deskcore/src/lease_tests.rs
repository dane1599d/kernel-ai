use crate::desk::WindowId;
use crate::lease::{ASK_NS, Lease, Lent, Step};

const TERM: WindowId = WindowId(1);
const NOTES: WindowId = WindowId(2);

#[test]
fn the_terminal_is_asked_for_once_then_again_only_every_ask_ns() {
    let mut lease = Lease::new();
    assert_eq!(lease.step(Some(TERM), 0), Step::Ask);
    assert_eq!(
        lease.asked(TERM, true, 0),
        Lent {
            new: true,
            moved: Some(true)
        }
    );
    assert_eq!(lease.step(Some(TERM), ASK_NS - 1), Step::Stay);
    assert_eq!(lease.step(Some(TERM), ASK_NS), Step::Ask);
    // Asking again while lent is not a new lend: the desk keeps its keys.
    assert_eq!(
        lease.asked(TERM, true, ASK_NS),
        Lent {
            new: false,
            moved: None
        }
    );
}

#[test]
fn a_click_asks_again_at_once_as_the_kernel_ended_the_lease_on_it() {
    let mut lease = Lease::new();
    lease.asked(TERM, true, 0);
    // The press ended the lease in the kernel; the same pass lends again,
    // not up to ASK_NS later, so keys typed right after the click reach the
    // terminal.
    lease.forget();
    assert_eq!(lease.step(Some(TERM), 1), Step::Ask);
    assert!(lease.asked(TERM, true, 1).new);
    // A click with nothing focused leaves nothing to end.
    lease.forget();
    assert_eq!(lease.step(None, 2), Step::Stay);
}

#[test]
fn another_window_is_asked_for_once_and_losing_focus_ends_the_lease() {
    let mut lease = Lease::new();
    lease.asked(TERM, true, 0);
    assert_eq!(lease.step(Some(NOTES), 1), Step::Ask);
    assert_eq!(
        lease.asked(NOTES, false, 1),
        Lent {
            new: false,
            moved: Some(false)
        }
    );
    assert_eq!(lease.step(Some(NOTES), 10 * ASK_NS), Step::Stay);
    lease.asked(TERM, true, 10 * ASK_NS);
    assert_eq!(lease.step(None, 10 * ASK_NS + 1), Step::End);
    lease.forget();
    assert_eq!(lease.step(None, 10 * ASK_NS + 2), Step::Stay);
}
