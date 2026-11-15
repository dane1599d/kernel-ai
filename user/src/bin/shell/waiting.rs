//! The model's `act` calls, and the apps' requests looked at while one runs
//! (the founder's test 2, item 7). A call takes 3 to 70 s, and the shell
//! answered nothing else meanwhile: Files sat on "Loading…", and the Editor
//! and Settings waited.
//!
//! [`call_serving`] hands the call to the kernel (`sys::call_submit`) and,
//! each [`POLL_MS`] `sys::call_poll` waits without its answer, runs [`poll`]:
//! `desktop`'s waiting effects, file transfers and Settings requests go to the
//! agent core ([`Shell::serve_waiting`]), which answers those that only read.
//! The shell stays one thread. The rest are given back to `desktop` (#890),
//! which keeps each at the front of its app's queue, so a shell that dies
//! mid-line loses none: the restarted shell takes them. Their apps are marked
//! held (`shellcore::waiting::Held`, which says what waits behind what), and
//! each look passes them by. When the line ends, [`run_held`] takes each
//! again from `desktop`, in the order they came, and runs it.
//!
//! The person's Stop (#887) withdraws a call `desktop` has not taken yet. So
//! a request taken just as the Stop came is held, not served; one whose
//! giving back was withdrawn is held in the shell, and an answer whose giving
//! was withdrawn too: each runs, or is given, after the line. An app never
//! waits for ever on a request the shell took. What comes of a held request
//! after the line goes through `report_to_desktop` (#892), which a Stop for
//! that line does not cut.

use crate::{ask_desktop, desktop};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use deskcore::files::{Job, Op};
use deskcore::settings_wire::{self as wire, Answer};
use kabi::Error;
use kabi::handle::Handle;
use shellcore::waiting::{Held, Kind, Served, Waiting};
use shellcore::{EffectDone, Shell};
use user::sys;

/// The longest the shell waits on the model before it looks at what the apps
/// ask again.
const POLL_MS: u64 = 50;

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
    /// Given back to `desktop`, which keeps it at the front of the app's
    /// queue of that kind: taken again from there after the line.
    Back(Kind, String),
    /// An effect batch as `effects take` gave it, kept in the shell as its
    /// giving back failed: app, whether a person queued it, the lines.
    Effects(String, bool, String),
    /// The editor's transfer, kept in the shell.
    File(Job),
    /// The Settings app's requests, kept in the shell.
    Settings(String, Vec<wire::Request>),
    /// What came of an app's request, as the messages still to give
    /// `desktop`: the person's Stop withdrew the first before it took it.
    Give(String, Vec<Vec<u8>>),
}

/// What the apps asked while the model thought that was not a read.
pub type HeldRequests = Held<Request>;

/// One look at what the apps ask: a request the agent core answers through
/// `serve` goes back to its app now; the rest is held for the line's end.
/// Nothing of an app held back is taken, and nothing at all while a Settings
/// change is held.
pub fn poll(held: &RefCell<HeldRequests>, serve: &mut dyn FnMut(Waiting<'_>) -> Option<Served>) {
    let Some(route) = desktop() else {
        return;
    };
    if held.borrow().blocks_all() {
        return;
    }
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

/// Leaves `app`'s `request`, of `kind`, for the line's end: given back to
/// `desktop` with `back`, to wait there; kept in the shell if that failed (a
/// Stop withdrew it, or `desktop` would not take it). Either way its app is
/// held back until it has run.
fn hold(
    route: Handle,
    held: &RefCell<HeldRequests>,
    kind: Kind,
    app: &str,
    back: &str,
    request: Request,
) {
    let kept = match ask_desktop(route, back) {
        Ok(reply) if reply == "ok" => {
            user::println!("shell: gave {app}'s request back to desktop until the line ends");
            Request::Back(kind, String::from(app))
        }
        Ok(reply) => {
            user::println!("shell: holds {app}'s request until the line ends: {reply}");
            request
        }
        Err(why) => {
            user::println!("shell: holds {app}'s request until the line ends: {why}");
            request
        }
    };
    held.borrow_mut().hold(kind, app, kept);
}

/// The apps held back, for a take to pass by.
fn held_apps(held: &RefCell<HeldRequests>) -> Vec<String> {
    held.borrow().apps()
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
                held.borrow_mut().hold(Kind::Answer, app, rest);
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
    use deskcore::effects::{self, Taken};
    let apps = held_apps(held);
    let skip: Vec<&str> = apps.iter().map(String::as_str).collect();
    // Not a take of new work: `desktop` keeps the line running as it was.
    let Ok(answer) = ask_desktop(route, &effects::encode_take_during_but(&skip)) else {
        return;
    };
    let Some(Some((app, user, lines))) = effects::parse_take(&answer) else {
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
    let taken = Taken {
        app: app.into(),
        user,
        lines: lines.lines().map(String::from).collect(),
    };
    let request = Request::Effects(app.into(), user, lines.into());
    hold(
        route,
        held,
        Kind::Effects,
        app,
        &effects::encode_back(&taken),
        request,
    );
}

fn poll_files(
    route: Handle,
    held: &RefCell<HeldRequests>,
    serve: &mut dyn FnMut(Waiting<'_>) -> Option<Served>,
) {
    for _ in 0..crate::FILES_PER_PASS {
        let apps = held_apps(held);
        let take = match apps.is_empty() {
            true => String::from("files take"),
            false => format!("files take but {}", apps.join(" ")),
        };
        let Ok(answer) = ask_desktop(route, &take) else {
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
        let (back, app) = (format!("files back {}", job.id), job.app.clone());
        hold(route, held, Kind::File, &app, &back, Request::File(job));
    }
}

fn poll_settings(
    route: Handle,
    held: &RefCell<HeldRequests>,
    serve: &mut dyn FnMut(Waiting<'_>) -> Option<Served>,
) {
    if held.borrow().blocks(wire::SETTINGS.app) {
        return;
    }
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
    let back = wire::encode_back_for(&wire::SETTINGS, &requests);
    let request = Request::Settings(app.into(), requests);
    hold(route, held, Kind::Settings, app, &back, request);
}

/// Runs what was held while the model thought, oldest first, before the main
/// loop takes anything newer: one given back to `desktop` is taken again from
/// its app's queue, where it waits first. Each runs from the front of `held`,
/// so what is still held, and the one running, hold back what a line it runs
/// (a held `shell_line`) is asked meanwhile; what that holds runs here after.
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
        crate::start_of_unit();
        run_one(shell, held, route, request);
        held.borrow_mut().done();
    }
}

fn run_one(shell: &mut Shell, held: &RefCell<HeldRequests>, route: Handle, request: Request) {
    match request {
        Request::Back(kind, app) => {
            user::println!("shell: runs {app}'s held request after the line");
            match kind {
                Kind::Effects => {
                    let take = deskcore::effects::encode_take_app(&app);
                    crate::run_effects_taken(shell, held, route, &take);
                }
                Kind::File => {
                    crate::run_file_taken(shell, route);
                }
                Kind::Settings => crate::take_changes(shell, &wire::SETTINGS, Shell::settings),
                // An answer is never given back: `give` holds it in the shell.
                Kind::Answer => {}
            }
        }
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
                match crate::report_to_desktop_bytes(route, &message) {
                    Ok(reply) if !reply.starts_with(b"err") => {}
                    Ok(reply) => {
                        let why = String::from_utf8_lossy(&reply);
                        return user::println!("shell: desktop refused {app}'s answer: {why}");
                    }
                    Err(why) => {
                        return user::println!("shell: {app}'s answer was not given: {why}");
                    }
                }
            }
        }
    }
}
