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

use crate::{ask_desktop, desktop};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use deskcore::files::{Job, Op};
use deskcore::settings_wire::{self as wire, Answer};
use kabi::Error;
use kabi::handle::Handle;
use shellcore::Shell;
use shellcore::waiting::{Held, Kind, MAX_HELD, Served, Waiting};
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
/// The person's Stop (#887) lasts the line. Once it is set, no look begins
/// (an app's request taken from `desktop` would be lost if its call were
/// cut), and the call ends: llm answers a call it took with `stopped` within
/// about 50 ms, and one it had not taken is withdrawn (Interrupted).
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
    let blocked = held.borrow().blocks(app);
    let served = match blocked {
        true => None,
        false => serve(Waiting::Effects { app, lines, user }),
    };
    if let Some(Served::Effects(results)) = served {
        answered_now(app);
        return crate::give_effect_results(route, app, user, results);
    }
    let size = lines.lines().count();
    let request = Request::Effects(app.into(), user, lines.into());
    if hold(held, Kind::Effects, app, size, request).is_err() {
        let refused = tickets(lines)
            .into_iter()
            .map(|ticket| shellcore::EffectDone {
                ticket,
                outcome: Err(String::from(TOO_MANY)),
            })
            .collect();
        crate::give_effect_results(route, app, user, refused);
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
        let blocked = held.borrow().blocks(&job.app);
        let served = match (blocked, &job.op) {
            (false, Op::Read) => serve(Waiting::FileRead {
                app: &job.app,
                path: &job.path,
            }),
            _ => None,
        };
        if let Some(Served::FileRead(bytes)) = served {
            answered_now(&job.app);
            let done = bytes.and_then(|bytes| crate::fill_job(route, job.id, &bytes));
            crate::report_file_job(route, job.id, done);
            continue;
        }
        let (id, app) = (job.id, job.app.clone());
        if hold(held, Kind::File, &app, 1, Request::File(job)).is_err() {
            crate::report_file_job(route, id, Err(String::from(TOO_MANY)));
        }
    }
}

fn poll_settings(
    route: Handle,
    held: &RefCell<HeldRequests>,
    serve: &mut dyn FnMut(Waiting<'_>) -> Option<Served>,
) {
    let words = &wire::SETTINGS;
    let Ok(answer) = ask_desktop(route, words.take) else {
        return;
    };
    let Some(Some((app, requests))) = wire::parse_take(&answer) else {
        return;
    };
    let asked: Vec<(&str, bool)> = requests.iter().map(|r| (r.text.as_str(), r.user)).collect();
    let blocked = held.borrow().blocks(app);
    let served = match blocked {
        true => None,
        false => serve(Waiting::Settings {
            app,
            requests: &asked,
        }),
    };
    if let Some(Served::Settings(outcomes)) = served {
        answered_now(app);
        let answers: Vec<Answer> = requests
            .iter()
            .zip(outcomes)
            .map(|(r, outcome)| Answer::new(r.ticket, outcome))
            .collect();
        return crate::give_answers(route, words, app, &answers);
    }
    let refused: Vec<Answer> = requests
        .iter()
        .map(|r| Answer::new(r.ticket, Err(String::from(TOO_MANY))))
        .collect();
    let size = requests.len();
    let request = Request::Settings(app.into(), requests);
    if hold(held, Kind::Settings, app, size, request).is_err() {
        crate::give_answers(route, words, app, &refused);
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
    }
}
