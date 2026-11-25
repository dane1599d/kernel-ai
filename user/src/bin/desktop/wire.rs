//! The agent's and the network's requests over the wire, and a client's
//! needs once its message is read.

use super::*;

/// The answer to a request whose caller has no name: it ended before it was
/// served, or it is the kernel.
const NO_NAME: &str = "err the caller is not running";

pub(super) fn effects_request(state: &mut State, bytes: &[u8], pid: u64) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    if !text.starts_with("effects ") {
        return None;
    }
    let Some(caller) = name_of(pid) else {
        return Some(String::from(NO_NAME));
    };
    let shell = caller == "shell";
    let put = text
        .strip_prefix(effects::PUT_USER)
        .map(|lines| (lines, true))
        .or_else(|| text.strip_prefix(effects::PUT).map(|lines| (lines, false)));
    if let Some((lines, user)) = put {
        if shell {
            return Some(String::from("err only an app queues effects"));
        }
        // The Terminal's Stop, on a person's key or click: the shell's line
        // is interrupted now, never queued behind it.
        let (stops, rest) = effects::split_stops(lines);
        let lines = match (caller == effects::STOPS_FROM && user, stops.is_empty()) {
            (true, false) => {
                // A line still queued is taken back; one running is
                // interrupted.
                let queued = state.effects.withdraw_users(&caller);
                let line = deskcore::running::Line::Terminal;
                let stopped = match queued.is_empty() {
                    true => interrupt_shell(state, line, "the Terminal's Stop"),
                    false => {
                        println!("desktop: the Terminal's Stop took back its queued line");
                        Ok(String::from("stopped"))
                    }
                };
                // The line's own answer, as the shell's wire says it: the
                // Terminal shows "stopped" and takes a new line.
                for ticket in queued {
                    state.effects.finish(
                        &caller,
                        effects::Done {
                            ticket,
                            outcome: Ok(String::from("ok stopped")),
                        },
                    );
                }
                for ticket in stops {
                    state.effects.finish(
                        &caller,
                        effects::Done {
                            ticket,
                            outcome: stopped.clone(),
                        },
                    );
                }
                rest.as_str()
            }
            _ => lines,
        };
        let refused = state.effects.put_as(&caller, lines, user);
        let by = if user { ", by a person's input" } else { "" };
        println!("desktop: {caller} queued effects ({refused} refused{by})");
        wake_shell();
        return Some(String::from("ok"));
    }
    if text == effects::RESULTS {
        let results = state.effects.results(&caller, effects::MESSAGE_BUDGET);
        return Some(effects::encode_results(&results));
    }
    if !shell {
        return Some(String::from("err only the shell runs effects"));
    }
    // A take starts new work, except one the shell makes while a line runs
    // (#877): that line, and its tickets, stay the one running.
    let budget = kabi::MAX_MESSAGE - 64;
    if let Some(took) = state.running.take(&mut state.effects, text, budget) {
        if let Some(taken) = &took.taken {
            let during = match text.starts_with(effects::TAKE_DURING) {
                true => " during a line",
                false => "",
            };
            println!("desktop: the shell took {}'s effects{during}", taken.app);
        }
        finish_lost(state, took.stale);
        return Some(effects::encode_take(took.taken.as_ref()));
    }
    // #890: lines the shell took and holds back wait here again, so a shell
    // that dies mid-line loses none.
    if text.starts_with(effects::BACK) {
        let Some(taken) = effects::parse_back(text) else {
            return Some(String::from("err not an effects request"));
        };
        let app = taken.app.clone();
        let kept = state.effects.put_back(taken);
        println!("desktop: the shell put back {kept} of {app}'s effects");
        return Some(String::from("ok"));
    }
    let Some((app, results)) = effects::parse_done(text) else {
        return Some(String::from("err not an effects request"));
    };
    // Only a result for the line's own ticket ends it: what came of a card,
    // or a result the shell held, comes outside the line.
    if app == effects::STOPS_FROM {
        let tickets: Vec<u32> = results.iter().map(|done| done.ticket).collect();
        state.running.answered(&tickets);
    }
    for done in results {
        state.effects.finish(app, done);
    }
    Some(String::from("ok"))
}

/// A request on one app's wire (`deskcore::settings_wire`): `settings ...`
/// for the Settings app, `models ...` for the Models window (`words`). Only
/// that app's process may put or read answers, and only the shell may take and
/// answer; anything else is not one, for the next handler.
pub(super) fn wire_request(
    words: &settings::Words,
    queue: &mut settings::Queue,
    bytes: &[u8],
    pid: u64,
) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    if !text.starts_with(words.word) {
        return None;
    }
    let what = words.word.trim_end();
    let Some(caller) = name_of(pid) else {
        return Some(String::from(NO_NAME));
    };
    if let Some(lines) = text.strip_prefix(words.put) {
        return Some(match queue.put_as(words, &caller, lines) {
            Ok(()) => {
                wake_shell();
                String::from("ok")
            }
            Err(why) => format!("err {why}"),
        });
    }
    if text == words.results {
        return Some(match caller == words.app {
            true => queue.results(kabi::MAX_MESSAGE - 64),
            false => format!("err {}", words.refused),
        });
    }
    if caller != "shell" {
        return Some(format!("err only the shell answers {what}"));
    }
    if text == words.take {
        let taken = queue.take(kabi::MAX_MESSAGE - 64);
        return Some(settings::encode_take_for(words, taken.as_deref()));
    }
    // #890: requests the shell took and holds back wait here again.
    if text.starts_with(words.back) {
        return Some(match settings::parse_back_for(words, text) {
            Some(requests) => {
                let kept = queue.put_back(requests);
                println!("desktop: the shell put back {kept} {what} requests");
                String::from("ok")
            }
            None => format!("err not a {what} request"),
        });
    }
    let Some(body) = text
        .strip_prefix(words.done)
        .and_then(|rest| rest.strip_prefix(words.app))
        .and_then(|rest| rest.strip_prefix('\n'))
    else {
        return Some(format!("err not a {what} request"));
    };
    match settings::parse_answers(body) {
        Some(answers) => {
            for answer in answers {
                queue.finish(answer);
            }
            Some(String::from("ok"))
        }
        None => Some(String::from("err the answers are out of shape")),
    }
}

/// A `net ...` request (`deskcore::net_wire`): any app's process may ask
/// about a host, known by the kernel's name for it; only the shell takes the
/// asks and answers them. Anything else is not one, for the next handler.
pub(super) fn net_request(state: &mut State, bytes: &[u8], pid: u64) -> Option<String> {
    use deskcore::net_wire as wire;
    let text = core::str::from_utf8(bytes).ok()?;
    if !text.starts_with("net ") {
        return None;
    }
    let Some(caller) = name_of(pid) else {
        return Some(String::from(NO_NAME));
    };
    if let Some(ask) = text.strip_prefix(wire::ASK) {
        let reply = state.net.ask(&caller, ask, now_ms());
        if reply == wire::WAIT {
            wake_shell();
        }
        return Some(reply);
    }
    if caller != "shell" {
        return Some(String::from("err only the shell answers network asks"));
    }
    if text == wire::TAKE {
        return Some(wire::encode_take(state.net.take().as_ref()));
    }
    let Some(done) = text.strip_prefix(wire::DONE) else {
        return Some(String::from("err not a network request"));
    };
    Some(match state.net.done(done, now_ms()) {
        Ok(()) => String::from("ok"),
        Err(why) => format!("err {why}"),
    })
}

/// Carries out one thing a client's message needed; true when it kept the
/// handle the message moved (a pool's `Shm`), which `pools::release` closes
/// later, and false when the caller must close it.
pub(super) fn apply_need(
    state: &mut State,
    at: usize,
    pid: u64,
    need: Need,
    handle: Option<Handle>,
) -> bool {
    match need {
        Need::MapPool { pool, size } => {
            let Some(handle) = handle else { return false };
            // Writable when the client allowed it, for its input ring; read-only
            // otherwise, as before.
            let mapped = sys::shm_map(handle, true)
                .map(|address| (address, true))
                .or_else(|_| sys::shm_map(handle, false).map(|address| (address, false)));
            match mapped {
                Ok((address, writable)) => {
                    state.pools.push(Pool {
                        pid,
                        pool,
                        handle,
                        address,
                        size,
                        writable,
                    });
                    state.clients[at].pool_mapped(pool);
                    return true;
                }
                Err(error) => println!("desktop: cannot map pid {pid}'s pool: {error:?}"),
            }
            false
        }
        Need::Show {
            window,
            buffer,
            scale,
        } => {
            state.shown.retain(|(w, ..)| *w != window);
            state.shown.push((window, pid, buffer, scale));
            if let Some(at) = state.wheeled.iter().position(|(w, ..)| *w == window) {
                let (_, before, left) = state.wheeled.remove(at);
                if left == super::frame::WHEEL_PICTURES {
                    println!("desktop: window {} drew after the wheel", window.0);
                }
                // Not just a redraw: did the content move? A hover or a caret
                // redraws in place and says nothing here. A late redraw counts:
                // each of the next pictures is compared with the one before the
                // wheel, until one scrolled or `WHEEL_PICTURES` have not.
                let after = super::frame::wheel_rows(state, window).unwrap_or_default();
                match deskcore::scrolled::shift(&before, &after) {
                    Some((rows, explained)) => println!(
                        "desktop: window {} scrolled {rows} rows after the wheel ({explained} rows moved)",
                        window.0
                    ),
                    // Each miss is said, so a case that fails here can be read:
                    // one redraw that did not scroll leaves its line even when
                    // a later picture does. No picture shows as 0 rows; a band
                    // with no px in it (past the picture's width) as every
                    // row, all with the one hash.
                    None => {
                        let seen = super::frame::WHEEL_PICTURES - left + 1;
                        println!(
                            "desktop: window {} did not scroll after the wheel (picture {seen} of {}; {} rows before, {} after)",
                            window.0,
                            super::frame::WHEEL_PICTURES,
                            before.len(),
                            after.len()
                        );
                        if left > 1 {
                            state.wheeled.push((window, before, left - 1));
                        }
                    }
                }
            }
            false
        }
        Need::Closed { window } => {
            let pool = state
                .shown
                .iter()
                .find(|(w, ..)| *w == window)
                .map(|(_, _, buffer, _)| buffer.pool);
            state.shown.retain(|(w, ..)| *w != window);
            state.wheeled.retain(|(w, ..)| *w != window);
            state.agent.closed(window);
            pools::after_closed(state, at, pid, pool);
            false
        }
    }
}

/// The name a process runs under, which labels its windows, names its app
/// in standing instructions' events, and is all any request is checked by:
/// the kernel's table's (PROC_NAME, which DISPLAY allows), however many
/// processes run. METRICS lists only the first 16, and a caller past them
/// was `pid N`: refused as an app, and split in two on the network wire
/// (H3-8). None for a pid that is not running, or the kernel's.
pub(super) fn name_of(pid: u64) -> Option<alloc::string::String> {
    let mut name = [0u8; kabi::MAX_NAME];
    let len = sys::proc_name(pid, &mut name).ok()?;
    core::str::from_utf8(name.get(..len)?)
        .ok()
        .map(alloc::string::String::from)
}
