//! The kernel's input events: the keyboard lease, keys and the pointer to
//! the desk.

use super::*;

/// Lends the keys to the kernel's terminal while the terminal window has
/// focus, asking again in case the kernel ended the lease; ends it once the
/// focus moves. The kernel says which window that is: it refuses the lease for
/// any other client. When the keys are newly lent the desk forgets the keys it
/// holds down, as their releases now go to the console.
pub(super) fn lease(state: &mut State) {
    let focused = match state.desk.focus() {
        Focus::Window(id) => state
            .desk
            .window(id)
            .filter(|w| !w.minimized)
            .map(|w| (id, u64::from(w.client))),
        _ => None,
    };
    let now = sys::time_ns();
    match state.lease.step(focused.map(|(id, _)| id), now) {
        Step::Stay => {}
        Step::End => {
            let _ = sys::input_to_console(false, 0);
            println!("desktop: the keys are the desktop's again");
            state.lease.forget();
        }
        Step::Ask => {
            let Some((id, pid)) = focused else {
                return;
            };
            let lent = sys::input_to_console(true, pid).is_ok();
            let lent = state.lease.asked(id, lent, now);
            if lent.new {
                state.desk.keys_lent();
            }
            match lent.moved {
                Some(true) => println!("desktop: the terminal window has the keys"),
                Some(false) => println!("desktop: the keys are the desktop's again"),
                None => {}
            }
        }
    }
}

/// One kernel input event, as logical px (a motion count is one logical px,
/// and an absolute position spans the logical screen).
pub(super) fn input(state: &mut State, screen: &Screen, event: &InputEvent) {
    let p = state.desk.pointer();
    let input = match event.kind {
        Kind::Rel => Some(Input::Motion(match event.code {
            0 => Point {
                x: p.x + event.value,
                y: p.y,
            },
            _ => Point {
                x: p.x,
                y: p.y + event.value,
            },
        })),
        Kind::Abs => {
            let scale = |v: i32, max: i32| (i64::from(v) * i64::from(max) / 32768) as i32;
            Some(Input::Motion(match event.code {
                0 => Point {
                    x: scale(event.value, state.width),
                    y: p.y,
                },
                _ => Point {
                    x: p.x,
                    y: scale(event.value, state.height),
                },
            }))
        }
        Kind::Button => {
            let button = match event.code {
                1 => Button::Left,
                2 => Button::Right,
                _ => Button::Middle,
            };
            if event.value != 0 {
                // The kernel ended any lease on the press: lend again this pass.
                state.lease.forget();
            }
            Some(Input::Button {
                button,
                down: event.value != 0,
            })
        }
        // The vertical wheel; a horizontal one goes nowhere yet.
        Kind::Wheel if event.code == 0 => Some(Input::Wheel(event.value)),
        Kind::Key => {
            if state.log_first_key && !state.key_logged {
                state.key_logged = true;
                println!("desktop: key {:#x} {}", event.code, event.value);
            }
            if event.value != 0 {
                perf::key(state, event.time_ns);
            }
            // A Super tap or a chord the kernel saw ended its lease: the next
            // step asks again only for a terminal that has the focus. The
            // kernel's side ends too, as a tap it saw unleased may arrive
            // after it lent the keys for a new focus.
            let key = deskcore::lease::on_key(&mut state.lease, event.code, event.value);
            if key.end_lease {
                let _ = sys::input_to_console(false, 0);
            }
            // Super pressed alone opens nothing, but disarms a card.
            if key.super_press {
                state.desk.super_pressed(event.time_ns / 1_000_000);
            }
            for (usage, down) in key.keys {
                apply_input(state, screen, event, Input::Key { usage, down });
            }
            None
        }
        // The kernel dropped input since the last read: a release may be
        // among it, so what a button held ends now (#1352's follow-up).
        Kind::Screen if event.value == INPUT_LOST => {
            let effects = state.desk.input_lost();
            deliver(state, &effects);
            None
        }
        _ => None,
    };
    if let Some(input) = input {
        apply_input(state, screen, event, input);
    }
}

/// `kabi::input::INPUT_LOST`: a `Kind::Screen` event of this value starts
/// the first read after the kernel's queue dropped events (#1397). Until
/// that lands, no event has it, and nothing here changes.
const INPUT_LOST: i32 = 2;

/// One input for the desk, from `event`: the desk's effects carried out,
/// and the cursor moved with the pointer.
fn apply_input(state: &mut State, screen: &Screen, event: &InputEvent, input: Input) {
    state.watch.input(now_ms());
    let before = state.desk.pointer();
    // The card judges a key by when it was pressed, not when it was read.
    state.desk.set_now(event.time_ns / 1_000_000);
    let effects = state.desk.input(input);
    let at = state.desk.pointer();
    if at != before {
        if state.draws_cursor {
            state.extra.push(paint::cursor_box(before));
            state.extra.push(paint::cursor_box(at));
        } else if screen.info.cursor != 0 {
            let physical = |v: i32| deskcore::scale::to_physical(v, state.scale).max(0) as u32;
            let _ = sys::display_cursor_move(physical(at.x), physical(at.y));
        }
    }
    deliver(state, &effects);
}
