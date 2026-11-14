//! The model's `act` calls, and the apps' requests looked at while one runs
//! (the founder's test 2, item 7). A call takes 3 to 70 s, and the shell
//! answered nothing else meanwhile: Files sat on "Loading…", and the Editor
//! and Settings waited.
//!
//! [`call_serving`] hands the call to the kernel (`sys::call_submit`) and,
//! each [`POLL_MS`] `sys::call_poll` waits without its answer, runs [`poll`]:
//! `desktop`'s waiting effects, file transfers and Settings requests go to the
//! agent core ([`Shell::serve_waiting`]), which answers those that only read.
//! The shell stays one thread. The rest are held (`shellcore::waiting::Held`,
//! which says what waits behind what) and run when the line ends, in the order
//! they came ([`run_held`]). Past `MAX_HELD` for an app, one is refused at
//! once.
//!
//! The person's Stop (#887) withdraws a call `desktop` has not taken yet. So
//! a request taken just as the Stop came is held, not served, and an answer
//! whose giving was withdrawn is held too, and given after the line: an app
//! never waits for ever on a request the shell took.

use crate::{ask_desktop, desktop};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use deskcore::files::{Job, Op};
use deskcore::settings_wire::{self as wire, Answer};
use kabi::Error;
use kabi::handle::Handle;
use shellcore::waiting::{Held, Kind, MAX_HELD, Served, Waiting};
use shellcore::{EffectDone, Shell};
use user::sys;

/// The longest the shell waits on the model before it looks at what the apps
/// ask again.
const POLL_MS: u64 = 50;
/// Why a request past `MAX_HELD` is refused.
const TOO_MANY: &str = "too many requests wait while Kai thinks; ask again";

// The shell holds no more for an app than `desktop` queues for it.
const _: () = assert!(MAX_HELD == deskcore::effects::MAX_QUEUED);

/// The call `route` answers `request` with, as `sys::call_route` gives it,
/// running `poll` each [`POLL_MS`] it has not come. A call made while one is
/// in flight (none should be: a read calls no model) waits as before.
///
/// The person's Stop lasts the line. Once it is set, no look begins, and the
/// call ends: llm answers a call it took with `stopped` within about 50 ms,
/// and one it had not taken is withdrawn (Interrupted).
pub fn call_serving(
    route: Handle,
    request: &[u8],
    reply: &mut [u8],
    timeout_ms: u64,
    poll: &mut dyn FnMut(),
) -> Result<usize, Error> {
    match sys::call_submit(route, request, timeout_ms) {
        Ok(()) => {}
        Err(Error::Busy) => return sys::call_route(route, request, reply, timeout_ms),
        Err(error) => return Err(error),
    }
    loop {
        match sys::call_poll(reply, POLL_MS) {
            Err(Error::Busy) if sys::interrupted() => {}
            Err(Error::Busy) => poll(),
            done => return done,
        }
    }
}

/// One request held while the model thought.
pub enum Request {
    /// An effect batch as `effects take` gave it: app, whether a person
    /// queued it, the lines.
    Effects(String, bool, String),
    /// The editor's transfer.
    File(Job),
    /// The Settings app's requests.
    Settings(String, Vec<wire::Request>),
    /// What came of an app's request, as the messages still to give
    /// `desktop`: the person's Stop withdrew the first before it took it.
    Give(String, Vec<Vec<u8>>),
}

/// What the apps asked while the model thought that was not a read.
pub type HeldRequests = Held<Request>;

/// One look at what the apps ask: a request the agent core answers through
/// `serve` goes back to its app now; the rest is held for the line's end.
pub fn poll(held: &RefCell<HeldRequests>, serve: &mut dyn FnMut(Waiting<'_>) -> Option<Served>) {
    let Some(route) = desktop() else {
        return;
    };
    poll_effects(route, held, serve);
    poll_files(route, held, serve);
    poll_settings(route, held, serve);
}

/// The log's word that `app`'s request was answered while the model thought.
fn answered_now(app: &str) {
    user::println!("shell: answered {app}'s read while the model thought");
}

/// Whether a request of `app` just taken is served now: not while anything
/// holds it back, nor once the person has stopped (the answer's giving would
/// be withdrawn).
fn may_serve(held: &RefCell<HeldRequests>, app: &str) -> bool {
    !held.borrow().blocks(app) && !sys::interrupted()
}

/// Holds `request` for the line's end, or gives it back past `MAX_HELD`.
fn hold(
    held: &RefCell<HeldRequests>,
    kind: Kind,
    app: &str,
    size: usize,
    request: Request,
) -> Result<(), Request> {
    let held = held.borrow_mut().hold(kind, app, size, request);
    match held {
        Ok(()) => user::println!("shell: holds {app}'s request until the line ends"),
        Err(_) => user::println!("shell: refused {app}'s request: it has {MAX_HELD} waiting"),
    }
    held
}

/// Gives `desktop` `messages` for `app`, in order. One the person's Stop
/// withdrew is held with those after it, to give after the line; one
/// `desktop` refused ends the giving.
fn give(route: Handle, held: &RefCell<HeldRequests>, app: &str, messages: Vec<Vec<u8>>) {
    for (at, message) in messages.iter().enumerate() {
        match crate::ask_desktop_bytes(route, message) {
            Ok(reply) if !reply.starts_with(b"err") => {}
            Ok(reply) => {
                let why = String::from_utf8_lossy(&reply);
                return user::println!("shell: desktop refused {app}'s answer: {why}");
            }
            Err(why) if !sys::interrupted() => {
                return user::println!("shell: {app}'s answer was not given: {why}");
            }
            Err(_) => {
                let rest = Request::Give(app.into(), messages[at..].to_vec());
                held.borrow_mut()
                    .hold(Kind::Answer, app, 1, rest)
                    .unwrap_or_else(|_| {
                        user::println!("shell: {app}'s answer was dropped: too many wait")
                    });
                return user::println!("shell: holds {app}'s answer until the line ends");
            }
        }
    }
}

/// The message giving `desktop` what app `app`'s effects came to.
fn effects_done(app: &str, user: bool, results: Vec<EffectDone>) -> Vec<Vec<u8>> {
    alloc::vec![crate::effects_done(app, user, results).into_bytes()]
}

/// The messages giving `desktop` the answers to the Settings app's `app`.
fn settings_done(app: &str, answers: &[Answer]) -> Vec<Vec<u8>> {
    wire::encode_done_chunks_for(&wire::SETTINGS, app, answers, kabi::MAX_MESSAGE)
        .into_iter()
        .map(String::into_bytes)
        .collect()
}

fn poll_effects(
    route: Handle,
    held: &RefCell<HeldRequests>,
    serve: &mut dyn FnMut(Waiting<'_>) -> Option<Served>,
) {
    let Ok(answer) = ask_desktop(route, deskcore::effects::TAKE) else {
        return;
    };
    let Some(Some((app, user, lines))) = deskcore::effects::parse_take(&answer) else {
        return;
    };
    let served = match may_serve(held, app) {
        true => serve(Waiting::Effects { app, lines, user }),
        false => None,
    };
    if let Some(Served::Effects(results)) = served {
        answered_now(app);
        return give(route, held, app, effects_done(app, user, results));
    }
    let size = lines.lines().count();
    let request = Request::Effects(app.into(), user, lines.into());
    if hold(held, Kind::Effects, app, size, request).is_err() {
        let refused = tickets(lines)
            .into_iter()
            .map(|ticket| EffectDone {
                ticket,
                outcome: Err(String::from(TOO_MANY)),
            })
            .collect();
        give(route, held, app, effects_done(app, user, refused));
    }
}

/// The tickets of effect lines (`effect <ticket> …`).
fn tickets(lines: &str) -> Vec<u32> {
    lines
        .lines()
        .filter_map(|line| {
            line.strip_prefix("effect ")?
                .split_once(' ')?
                .0
                .parse()
                .ok()
        })
        .collect()
}

fn poll_files(
    route: Handle,
    held: &RefCell<HeldRequests>,
    serve: &mut dyn FnMut(Waiting<'_>) -> Option<Served>,
) {
    for _ in 0..crate::FILES_PER_PASS {
        let Ok(answer) = ask_desktop(route, "files take") else {
            return;
        };
        let Some(job) = deskcore::files::parse_job(&answer) else {
            return;
        };
        let served = match (may_serve(held, &job.app), &job.op) {
            (true, Op::Read) => serve(Waiting::FileRead {
                app: &job.app,
                path: &job.path,
            }),
            _ => None,
        };
        if let Some(Served::FileRead(read)) = served {
            answered_now(&job.app);
            let mut messages = match &read {
                Ok(bytes) => crate::file_fills(job.id, bytes),
                Err(_) => Vec::new(),
            };
            let done = crate::file_done(job.id, read.map(|_| ()));
            messages.push(done.into_bytes());
            give(route, held, &job.app, messages);
            continue;
        }
        let (id, app) = (job.id, job.app.clone());
        if hold(held, Kind::File, &app, 1, Request::File(job)).is_err() {
            let refused = crate::file_done(id, Err(String::from(TOO_MANY)));
            give(route, held, &app, alloc::vec![refused.into_bytes()]);
        }
    }
}

fn poll_settings(
    route: Handle,
    held: &RefCell<HeldRequests>,
    serve: &mut dyn FnMut(Waiting<'_>) -> Option<Served>,
) {
    let Ok(answer) = ask_desktop(route, wire::SETTINGS.take) else {
        return;
    };
    let Some(Some((app, requests))) = wire::parse_take(&answer) else {
        return;
    };
    let asked: Vec<(&str, bool)> = requests.iter().map(|r| (r.text.as_str(), r.user)).collect();
    let served = match may_serve(held, app) {
        true => serve(Waiting::Settings {
            app,
            requests: &asked,
        }),
        false => None,
    };
    if let Some(Served::Settings(outcomes)) = served {
        answered_now(app);
        let answers: Vec<Answer> = requests
            .iter()
            .zip(outcomes)
            .map(|(r, outcome)| Answer::new(r.ticket, outcome))
            .collect();
        return give(route, held, app, settings_done(app, &answers));
    }
    let refused: Vec<Answer> = requests
        .iter()
        .map(|r| Answer::new(r.ticket, Err(String::from(TOO_MANY))))
        .collect();
    let size = requests.len();
    let request = Request::Settings(app.into(), requests);
    if hold(held, Kind::Settings, app, size, request).is_err() {
        give(route, held, app, settings_done(app, &refused));
    }
}

/// Runs what was held while the model thought, oldest first, before the main
/// loop takes anything newer. Each runs from the front of `held`, so what is
/// still held, and the one running, hold back what a line it runs (a held
/// `shell_line`) is asked meanwhile; what that holds runs here after.
pub fn run_held(shell: &mut Shell, held: &RefCell<HeldRequests>) {
    if held.borrow().is_empty() {
        return;
    }
    let Some(route) = desktop() else {
        user::println!("shell: what the apps asked during the line waits: no desktop");
        return;
    };
    loop {
        // The borrow ends here: what runs may look at `held` again.
        let next = held.borrow_mut().start();
        let Some(request) = next else {
            return;
        };
        // Each is a unit of its own: a Stop for the line before, or for a
        // held line before it, is not its.
        let _ = sys::interrupt_clear();
        run_one(shell, route, request);
        held.borrow_mut().done();
    }
}

fn run_one(shell: &mut Shell, route: Handle, request: Request) {
    match request {
        Request::Effects(app, user, lines) => {
            user::println!("shell: runs {app}'s held request after the line");
            let results = shell.run_effects_from(&app, &lines, user);
            crate::give_effect_results(route, &app, user, results);
        }
        Request::File(job) => {
            user::println!("shell: runs {}'s held request after the line", job.app);
            let done = crate::do_file_job(shell, route, &job);
            crate::report_file_job(route, job.id, done);
        }
        Request::Settings(app, requests) => {
            user::println!("shell: runs {app}'s held request after the line");
            let answers: Vec<Answer> = requests
                .iter()
                .map(|r| Answer::new(r.ticket, shell.settings(&app, &r.text, r.user)))
                .collect();
            crate::give_answers(route, &wire::SETTINGS, &app, &answers);
        }
        Request::Give(app, messages) => {
            user::println!("shell: gives {app}'s held answer after the line");
            for message in messages {
                if let Err(why) = crate::ask_desktop_bytes(route, &message) {
                    return user::println!("shell: {app}'s answer was not given: {why}");
                }
            }
        }
    }
}
