//! The agent's and the network's requests over the wire, and a client's
//! needs once its message is read.

use super::*;

pub(super) fn effects_request(state: &mut State, bytes: &[u8], pid: u64) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    if !text.starts_with("effects ") {
        return None;
    }
    let caller = name_of(pid);
    let shell = caller == "shell";
    let put = text
        .strip_prefix(effects::PUT_USER)
        .map(|lines| (lines, true))
        .or_else(|| text.strip_prefix(effects::PUT).map(|lines| (lines, false)));
    if let Some((lines, user)) = put {
        if shell || caller.starts_with("pid ") {
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
    let caller = name_of(pid);
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
    let caller = name_of(pid);
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

pub(super) fn apply_need(
    state: &mut State,
    at: usize,
    pid: u64,
    need: Need,
    handle: Option<Handle>,
) {
    match need {
        Need::MapPool { pool, size } => {
            let Some(handle) = handle else { return };
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
                        address,
                        size,
                        writable,
                    });
                    state.clients[at].pool_mapped(pool);
                }
                Err(error) => println!("desktop: cannot map pid {pid}'s pool: {error:?}"),
            }
        }
        Need::Show {
            window,
            buffer,
            scale,
        } => {
            state.shown.retain(|(w, ..)| *w != window);
            state.shown.push((window, pid, buffer, scale));
        }
        Need::Closed { window } => {
            state.shown.retain(|(w, ..)| *w != window);
            state.agent.closed(window);
        }
    }
}

/// The name a process runs under, which labels its windows and names its app
/// in standing instructions' events. From the metrics' process list: LIST is
/// the supervisor's alone.
pub(super) fn name_of(pid: u64) -> alloc::string::String {
    let named = sys::metrics().ok().and_then(|m| {
        let listed = &m.processes[..(m.process_count as usize).min(m.processes.len())];
        listed
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| alloc::string::String::from(p.name()))
    });
    named.unwrap_or_else(|| alloc::format!("pid {pid}"))
}
