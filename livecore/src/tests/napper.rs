//! The nap's driver against a real host: every way an app is resumed ends
//! the nap, and a suspend that is not the nap's own is never woken by focus.

use super::{V1, call, host, kernel};
use crate::nap::Nap;
use crate::napper::{Napped, Napper};

const NAP: u64 = 1_000;

fn napper() -> Napper {
    Napper::new(Nap::new(NAP, 0))
}

/// A request as comphost serves it: the napper, then the host, then `after`.
fn serve(
    n: &mut Napper,
    h: &mut crate::host::Host<crate::local::LocalEnv<super::fake::Fake>>,
    request: &[u8],
    now: u64,
) -> alloc::string::String {
    let (reply, _) = n.request(h, 0, request, now).unwrap();
    let reply = match reply {
        Some(reply) => reply,
        None => h.handle(0, request).unwrap(),
    };
    n.after(h, now);
    alloc::string::String::from_utf8(reply).unwrap()
}

/// comphost's loop: a tick at each wait the nap asks for, up to `to`; what
/// the tick at `to` (or the nap before it) made of it.
fn tick_to(
    n: &mut Napper,
    h: &mut crate::host::Host<crate::local::LocalEnv<super::fake::Fake>>,
    from: u64,
    to: u64,
) -> Result<Option<Napped>, crate::host::Exit> {
    let mut at = from;
    loop {
        let wait = n.nap.wait_ms(at).unwrap_or(to.saturating_sub(at)).max(1);
        at = (at + wait).min(to);
        let ticked = n.tick(h, at)?;
        if at == to || ticked.is_some() {
            return Ok(ticked);
        }
    }
}

fn text(reply: Option<alloc::vec::Vec<u8>>) -> alloc::string::String {
    alloc::string::String::from_utf8(reply.unwrap()).unwrap()
}

#[test]
fn an_unused_app_naps_and_a_request_wakes_it() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    assert_eq!(tick_to(&mut n, &mut h, 0, NAP - 1), Ok(None));
    assert_eq!(
        tick_to(&mut n, &mut h, NAP - 1, NAP),
        Ok(Some(Napped::Napped))
    );
    assert!(h.suspended() && n.nap.is_napping());
    let (reply, woke) = n.request(&mut h, 5, b"get", 2 * NAP).unwrap();
    assert!(reply.is_none() && matches!(woke, Some(Napped::Woke(_))));
    assert!(!h.suspended() && !n.nap.is_napping());
    assert_eq!(call(&mut h, "get"), "ok 0 schema 1");
}

#[test]
fn a_wake_from_the_window_side_ends_the_nap_so_it_naps_again() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    tick_to(&mut n, &mut h, 0, NAP).unwrap();
    // A close or the agent's action on its kept window: `Host::wake`.
    h.wake().unwrap();
    // The next entry syncs; the nap is over and comes again in time.
    assert_eq!(tick_to(&mut n, &mut h, NAP, NAP + 1), Ok(None));
    assert!(!n.nap.is_napping());
    assert_eq!(
        tick_to(&mut n, &mut h, NAP + 1, 2 * NAP + 1),
        Ok(Some(Napped::Napped))
    );
}

#[test]
fn focus_never_wakes_a_suspend_that_is_not_the_nap_s() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    // Napped, woken from the window side, then a rule suspends it: served
    // as comphost serves every request, through the napper first.
    tick_to(&mut n, &mut h, 0, NAP).unwrap();
    h.wake().unwrap();
    assert_eq!(
        serve(&mut n, &mut h, b"ctl suspend", 2 * NAP),
        "ok suspended counter 1"
    );
    assert_eq!(n.focus(&mut h, true, 3 * NAP), Ok(None));
    assert!(h.suspended(), "the rule's suspend holds");
    // A rule's suspend of a napping app takes it over the same way.
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    tick_to(&mut n, &mut h, 0, NAP).unwrap();
    let (reply, woke) = n.request(&mut h, 0, b"ctl suspend", NAP + 1).unwrap();
    assert_eq!(text(reply), "ok suspended counter (it was napping)");
    assert!(woke.is_none() && h.suspended() && !n.nap.is_napping());
    assert_eq!(n.focus(&mut h, true, 2 * NAP), Ok(None));
    assert!(h.suspended());
}

#[test]
fn status_and_resume_pass_as_they_are_and_resume_ends_the_nap() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    tick_to(&mut n, &mut h, 0, NAP).unwrap();
    for request in [b"ctl status".as_slice(), b"ctl resume"] {
        let (reply, woke) = n.request(&mut h, 0, request, NAP + 1).unwrap();
        assert!(
            reply.is_none() && woke.is_none(),
            "{request:?} goes to the host"
        );
    }
    assert_eq!(
        call(&mut h, "ctl status"),
        "ok counter 1 schema 1 suspended"
    );
    assert_eq!(call(&mut h, "ctl resume"), "ok resumed counter 1");
    n.after(&h, NAP + 2);
    assert!(!n.nap.is_napping());
}

#[test]
fn the_keep_running_switch_is_answered_and_holds_it_awake() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    let (reply, _) = n.request(&mut h, 0, b"ctl keep-running on", 0).unwrap();
    assert_eq!(text(reply), "ok keep-running on");
    assert_eq!(tick_to(&mut n, &mut h, 0, 10 * NAP), Ok(None));
    // Only the kernel may set it.
    let (reply, _) = n.request(&mut h, 7, b"ctl keep-running off", 0).unwrap();
    assert!(reply.is_none());
    n.request(&mut h, 0, b"ctl keep-running off", 10 * NAP)
        .unwrap();
    assert_eq!(
        tick_to(&mut n, &mut h, 10 * NAP, 11 * NAP),
        Ok(Some(Napped::Napped))
    );
}

#[test]
fn keep_running_on_wakes_a_napping_app_but_not_a_rule_s_suspend() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    tick_to(&mut n, &mut h, 0, NAP).unwrap();
    let (reply, woke) = n
        .request(&mut h, 0, b"ctl keep-running on", NAP + 1)
        .unwrap();
    assert_eq!(text(reply), "ok keep-running on");
    assert!(matches!(woke, Some(Napped::Woke(_))));
    assert!(!h.suspended() && !n.nap.is_napping());
    assert_eq!(
        tick_to(&mut n, &mut h, NAP + 1, 10 * NAP),
        Ok(None),
        "and it stays awake"
    );
    // A rule's suspend is the rule's to end: the switch only stops the nap.
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    assert_eq!(call(&mut h, "ctl suspend"), "ok suspended counter 1");
    let (_, woke) = n.request(&mut h, 0, b"ctl keep-running on", 1).unwrap();
    assert!(woke.is_none() && h.suspended());
}

#[test]
fn the_agents_look_at_an_apps_rows_neither_wakes_it_nor_counts_as_use() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    tick_to(&mut n, &mut h, 0, NAP).unwrap();
    // From the supervisor, for the agent core: answered from the artifact.
    for request in [b"kai-actions 1".as_slice(), b"kai-actions 1\n"] {
        let (reply, woke) = n.request(&mut h, 9, request, NAP + 1).unwrap();
        assert!(
            reply.is_none() && woke.is_none(),
            "{request:?} goes to the host"
        );
        h.handle(9, request).unwrap();
        n.after(&h, NAP + 1);
    }
    assert!(h.suspended() && n.nap.is_napping(), "still napping");
    // Nor does it put off the next nap of a running app.
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    assert_eq!(tick_to(&mut n, &mut h, 0, NAP - 1), Ok(None));
    n.request(&mut h, 9, b"kai-actions 1", NAP - 1).unwrap();
    assert_eq!(
        tick_to(&mut n, &mut h, NAP - 1, NAP),
        Ok(Some(Napped::Napped))
    );
}

/// The founder's second test (backlog item 1): a window that holds the focus
/// never naps, however long; once it loses the focus it naps when unused.
#[test]
fn a_focused_window_never_naps_and_one_that_lost_the_focus_does() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    assert_eq!(n.focus(&mut h, true, 0), Ok(None));
    assert_eq!(
        tick_to(&mut n, &mut h, 0, 10 * NAP),
        Ok(None),
        "focused, it napped"
    );
    assert_eq!(n.focus(&mut h, false, 10 * NAP), Ok(None));
    assert_eq!(tick_to(&mut n, &mut h, 10 * NAP, 11 * NAP - 1), Ok(None));
    assert_eq!(
        tick_to(&mut n, &mut h, 11 * NAP - 1, 11 * NAP),
        Ok(Some(Napped::Napped))
    );
}

/// Audit item 7: after a VM pause or host sleep, the clock leaps, and the
/// nap counts it as half a nap at most: the app naps only after the rest
/// passes, tick by tick.
#[test]
fn a_clock_jump_does_not_nap_every_app_at_once() {
    let k = kernel(&[V1]);
    let mut h = host(&k);
    let mut n = napper();
    assert_eq!(tick_to(&mut n, &mut h, 0, 10), Ok(None));
    assert_eq!(n.tick(&mut h, 3_600_000), Ok(None), "an hour's stall");
    assert!(!h.suspended());
    assert_eq!(
        tick_to(&mut n, &mut h, 3_600_000, 3_600_000 + NAP / 2 - 11),
        Ok(None)
    );
    assert_eq!(
        tick_to(&mut n, &mut h, 3_600_000 + NAP / 2 - 11, 3_600_000 + NAP),
        Ok(Some(Napped::Napped))
    );
}
