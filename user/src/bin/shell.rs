//! The Rhai shell: answers each line the kernel terminal sends with `ok <text>`,
//! `err <text>` or `confirm <id> <text>`. The evaluation itself lives in the
//! host-tested `shellcore` crate; this program connects it to the system calls.

#![no_std]
#![no_main]

extern crate alloc;

// A program's crate root sits in src/bin, so its modules are named by path.
#[path = "shell/apart.rs"]
mod apart;
#[path = "shell/build.rs"]
mod build;
#[path = "shell/files.rs"]
mod files;
#[path = "shell/host.rs"]
mod host;
#[path = "shell/net.rs"]
mod net;
#[path = "shell/routes.rs"]
mod routes;
#[path = "shell/ui.rs"]
mod ui;
#[path = "shell/waiting.rs"]
mod waiting;
#[path = "shell/warmth.rs"]
mod warmth;

use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use kabi::sched::{SchedStats, TrialRequest};
use llmproto::act::{self, ActError, ActRequest, Call};
use llmproto::chat::Completion;
use llmproto::script::{self, Answer, AnswerError, Lang, ScriptRequest};
use llmproto::tune::{self, TuneRequest};
use shellcore::outbox::Stage;
use shellcore::targets::Service;
use shellcore::waiting::Took;
use shellcore::{DEFAULT_CONFIG, Host, PanelResult, Proof, SCRIPT_ROUND_MS, Shell};
use user::{arg, args, logln, println, sys};

user::entry!(main);

/// How long `ask` waits for the llm service.
const ASK_TIMEOUT_MS: u64 = 140_000;
/// How long one script round waits for the llm service: its own limit and a margin.
const SCRIPT_CALL_MS: u64 = SCRIPT_ROUND_MS + 5_000;
/// How long `remember` and `new` wait for llm, which only stores the turn.
const MEMORY_CALL_MS: u64 = 5_000;
/// llm's wait for an app job's `app-start`, `app-take` or `app-stop`, which it
/// answers between the steps of the job's answer.
const APP_JOB_CALL_MS: u64 = 10_000;
/// The app job (`app-start`) the shell waits for, 0 for none; `app-take` is
/// asked only while there is one.
static APP_TICKET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// When `app-take` was last asked, and the least time between two.
static APP_TAKEN_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
const APP_TAKE_GAP_NS: u64 = 2_000_000_000;

/// How long a script's child may run: the line budget its engine keeps, and a
/// margin for the child's own check before it.
const SCRIPT_LIMIT_MS: u64 = DEFAULT_CONFIG.line_budget_ms + 2_000;

struct System {
    pool: apart::Pool,
    files: files::Files,
    state: files::State,
    /// The installed apps' rows, loaded as the agent first calls them (P4).
    apps: core::cell::RefCell<shellcore::apps::Apps>,
    /// The saved appearance is still to reach `desktop`: at start, and after
    /// each change ([`give_appearance`]).
    appearance_due: core::cell::Cell<bool>,
    /// When `desktop` was last offered it, so one that is down costs a call
    /// only every [`APPEARANCE_RETRY_NS`].
    appearance_tried_ns: core::cell::Cell<u64>,
    /// The `desktop` last given it, by pid: a new one (a restart) is given it
    /// again.
    desktop_pid: core::cell::Cell<u64>,
    /// When the shell last looked for a new `desktop`.
    desktop_looked_ns: core::cell::Cell<u64>,
    /// What the apps asked while the model thought that was not a read, most
    /// of it given back to `desktop` to keep: run after the line, before
    /// anything newer ([`waiting::run_held`]).
    held: core::cell::RefCell<waiting::HeldRequests>,
}

/// How often an appearance `desktop` did not take is offered again.
const APPEARANCE_RETRY_NS: u64 = 10_000_000_000;
/// How often the shell looks for a `desktop` that started or restarted since
/// it last gave it the appearance.
const DESKTOP_LOOK_NS: u64 = 2_000_000_000;

/// The running `desktop`'s pid, from the kernel's METRICS.
fn desktop_pid() -> Option<u64> {
    let metrics = sys::metrics().ok()?;
    metrics
        .processes
        .iter()
        .take(metrics.process_count as usize)
        .find(|p| p.name() == "desktop")
        .map(|p| p.pid)
}

// The shell's first routes, by their index in its plan line's
// `uses=llm+supervisor+cc+updated` (`kabi::plan::SHELL_ROUTES`); those after
// them by name (`shell/routes.rs`).
use kabi::plan::shell_route;

/// The route to the llm service.
fn llm() -> kabi::handle::Handle {
    user::route(shell_route::LLM).unwrap_or(kabi::handle::Handle::NONE)
}

/// One app-job request to llm (#618): its reply as text.
fn app_job(request: &str) -> Result<String, kabi::Error> {
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let len = sys::call_route(llm(), request.as_bytes(), &mut reply, APP_JOB_CALL_MS)?;
    Ok(String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned())
}

/// The route to the supervisor.
fn supervisor() -> kabi::handle::Handle {
    user::route(shell_route::SUPERVISOR).unwrap_or(kabi::handle::Handle::NONE)
}

/// The route to `cc`, the `c>` prompt's compiler.
fn cc_route() -> kabi::handle::Handle {
    user::route(shell_route::CC).unwrap_or(kabi::handle::Handle::NONE)
}

/// cc's wait for one entry: a compile and a run of 2 s each, and on its first
/// request, translating cc.wasm and compiling kairt.
const CC_CALL_MS: u64 = 30_000;

/// The route to `updated`, the model channel (P4 task 10).
fn updated() -> kabi::handle::Handle {
    user::route(shell_route::UPDATED).unwrap_or(kabi::handle::Handle::NONE)
}

/// `updated`'s wait for one model request: a list reads the store and checks
/// the signed role, and a switch commits the trial record and the hint; an
/// import only starts, and goes on between requests.
const UPDATED_CALL_MS: u64 = 10_000;
/// How long a restart waits between the kernel's Busy answers, and how many
/// times it asks: five seconds in all.
const REBOOT_RETRY_MS: u64 = 100;
const REBOOT_TRIES: u32 = 50;

/// The route to `desktop`, on a desktop only (P3 task 7): `routes=desktop`.
fn desktop() -> Option<kabi::handle::Handle> {
    routes::named("desktop")
}

/// The route to `pkg`, where `cc build` installs (P4 task 13): `routes=pkg`.
fn pkg_route() -> Result<kabi::handle::Handle, String> {
    routes::named("pkg").ok_or_else(|| String::from("this shell has no route to pkg"))
}

const DESKTOP_CALL_MS: u64 = 5_000;

/// One panel request to `desktop`: its answer.
fn ask_desktop(route: kabi::handle::Handle, request: &str) -> Result<String, String> {
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let len = sys::call_route(route, request.as_bytes(), &mut reply, DESKTOP_CALL_MS)
        .map_err(|error| format!("desktop did not answer ({error:?})"))?;
    Ok(String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned())
}

/// Gives `desktop` the saved appearance, when it is due: at start (a
/// `desktop` that starts after the shell gets it on a later pass), after each
/// change, and to each new `desktop` (a restart). No desktop, or nothing
/// saved, leaves nothing to do.
fn give_appearance(system: &System) {
    let now = sys::time_ns();
    // A `desktop` that started or restarted (the supervisor's restart, a new
    // boot) starts from its plan line: give it the saved appearance again.
    if now.saturating_sub(system.desktop_looked_ns.get()) >= DESKTOP_LOOK_NS {
        system.desktop_looked_ns.set(now);
        if let Some(pid) = desktop_pid().filter(|&pid| pid != system.desktop_pid.get()) {
            system.desktop_pid.set(pid);
            system.appearance_due.set(true);
            system.appearance_tried_ns.set(0);
        }
    }
    if !system.appearance_due.get()
        || now.saturating_sub(system.appearance_tried_ns.get()) < APPEARANCE_RETRY_NS
            && system.appearance_tried_ns.get() != 0
    {
        return;
    }
    let (Some(route), Some(text)) = (desktop(), system.state.read(files::APPEARANCE)) else {
        system.appearance_due.set(false);
        return;
    };
    system.appearance_tried_ns.set(now);
    let request = deskcore::panel_wire::encode_appearance(&text);
    // Not up yet, or busy: offered again on a later pass.
    if let Ok(reply) = ask_desktop(route, &request) {
        if reply != "ok" {
            println!("desktop did not take the saved appearance: {reply}");
        }
        system.appearance_due.set(false);
    }
}

/// The start of a unit of work the shell takes from `desktop` rather than
/// receives as a request (a panel line, effects, files, changes, the
/// network): a Stop for an earlier unit no longer stands, since only the
/// shell's next `svc_recv` would clear it (#887).
fn start_of_unit() {
    if sys::interrupt_clear() {
        println!("a Stop for the last line was cleared before the next");
    }
}

/// A line's result to `desktop`, which a Stop must never hide: the work ran.
/// The flag is cleared first; a Stop pressed during the call itself ends it
/// Interrupted, and then it is cleared and sent once more.
///
/// Only for what comes after a line: during one (`waiting::give`) a Stop is
/// the line's, and clearing it would let the line go on.
fn report_to_desktop(route: kabi::handle::Handle, request: &str) -> Result<String, String> {
    let reply = report_to_desktop_bytes(route, request.as_bytes())?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// [`report_to_desktop`] for a message that is not all text (a file's piece),
/// answered in bytes.
fn report_to_desktop_bytes(route: kabi::handle::Handle, request: &[u8]) -> Result<Vec<u8>, String> {
    report_call(route, request).map_err(|error| format!("desktop did not answer ({error:?})"))
}

/// [`report_to_desktop_bytes`], failing with the call's own error.
fn report_call(route: kabi::handle::Handle, request: &[u8]) -> Result<Vec<u8>, kabi::Error> {
    let _ = sys::interrupt_clear();
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let mut sent = sys::call_route(route, request, &mut reply, DESKTOP_CALL_MS);
    if sent == Err(kabi::Error::Interrupted) {
        let _ = sys::interrupt_clear();
        sent = sys::call_route(route, request, &mut reply, DESKTOP_CALL_MS);
    }
    let len = sent?;
    reply.truncate(len.min(reply.len()));
    Ok(reply)
}

/// Takes the panel's oldest request, runs a line apart from the terminal's
/// entry and questions (`Shell::eval_panel`), and reports what came of it:
/// first what the AI said, for the desk's history (`panel said`), then the
/// bar's state, code's result line or why not, never the model's words. The
/// panel shows thinking from the Enter until then. A line that asks first
/// waits for the trusted card (`deskask`); the panel says so. Whether it
/// took a request, a line or a Stop: no held send starts on that pass
/// (`give_sends`), so an undo queued behind it is taken first. A take that
/// failed, or an answer that does not read, counts as taken too: an undo may
/// wait behind it, so held sends wait while the desktop does not answer.
fn take_panel(shell: &mut Shell, warmth: &mut warmth::Warmth) -> bool {
    use deskcore::panel::{Request, State};
    use deskcore::panel_wire;
    // A unit of work that is no request: a Stop for the last one ends here.
    start_of_unit();
    let Some(route) = desktop() else {
        return false;
    };
    let Ok(answer) = ask_desktop(route, panel_wire::TAKE) else {
        return true;
    };
    let shown = match panel_wire::parse_take(&answer) {
        Some(Some(Request::Line(line))) => {
            ui::begin();
            // "Kai is getting ready" for a line that waits on llm's warm-up.
            warmth.before_request();
            let result = shell.eval_panel(&line);
            let said = shell.take_panel_said().map(|s| panel_wire::encode_said(&s));
            let note = shell.take_panel_note().map(|n| panel_wire::encode_note(&n));
            // After the line: an Esc pressed during it must not drop them.
            for request in said.iter().chain(note.iter()) {
                match report_to_desktop(route, request) {
                    Ok(answer) if answer == "ok" => {}
                    Ok(answer) => println!("the panel's history was not given it: {answer}"),
                    Err(why) => println!("the panel's history was not given it: {why}"),
                }
            }
            let shown = match result {
                PanelResult::Done(line) => State::Done { line },
                PanelResult::Refused(reason) => State::Refused { reason },
            };
            warmth.after_request();
            shown
        }
        // Esc while Kai builds an app in the background stops the build.
        // A Stop that stopped nothing (the line it was for has ended) is
        // still a request taken: the next pass takes the one behind it.
        Some(Some(Request::Stop)) => match shell.stop_app_build() {
            Some(reply) => State::Done { line: reply.text },
            None => return true,
        },
        // Nothing waits. Esc after the shell took the line: it has run
        // already, one line at a time, and its result stays shown. Esc before
        // is desktop's own: the line never came here.
        Some(None) => return false,
        // An answer that does not read: whatever it was, it is not passed
        // over for a send.
        None => return true,
    };
    // A done line names the log entry its undo link takes back.
    let entry = shell.take_panel_entry();
    let Some(request) = panel_wire::encode_done(&shown, entry) else {
        return true;
    };
    // The line ended: a Stop pressed as it did must not hide its result.
    match report_to_desktop(route, &request) {
        Ok(answer) if answer == "ok" => {}
        Ok(answer) => println!("the panel's state was not set: {answer}"),
        Err(why) => println!("the panel's state was not set: {why}"),
    }
    true
}

/// The held sends (`shellcore::outbox`), a unit of their own after the
/// panel's line, so an `undo` it ran wins: each one due starts (its
/// `sending` on disk first), the panel is told it is going, and only then is
/// it sent. What each came to, and the sends still held, go to the panel.
/// One send a pass at most, and none on a pass whose [`take_panel`] took a
/// request (`took`): the next pass takes the panel's next request first, so
/// an undo queued behind a line or a Stop, or clicked during a send's call,
/// wins over the next send (the wait is 1 ms while one is due). `shown` is
/// the list last reported, without its countdowns, and the desktop it went to.
fn give_sends(shell: &mut Shell, system: &System, shown: &mut SendsShown, took: bool) {
    start_of_unit();
    // A desktop that restarted has no list: it is told again.
    if shown
        .as_ref()
        .is_some_and(|(pid, _)| *pid != system.desktop_pid.get())
    {
        *shown = None;
    }
    if let Some(entry) = shell.start_send_after(took, now_ms()) {
        report_sends(shell, system, shown);
        shell.run_send(entry);
    }
    let ended = shell.take_sends_ended();
    let Some(route) = desktop() else {
        return;
    };
    for send in ended {
        let failed = !matches!(send.stage, Stage::Sent | Stage::Cancelled);
        let request = deskcore::sends::encode_sent(send.entry, failed, &send.line);
        if let Err(why) = report_to_desktop(route, &request) {
            println!("the panel was not told what a send came to: {why}");
        }
    }
    report_sends(shell, system, shown);
}

/// The held sends last reported to the panel, as (entry, going), and the
/// desktop's pid then.
type SendsShown = Option<(u64, Vec<(u64, bool)>)>;

/// The held sends to the panel, when they changed since `shown`.
fn report_sends(shell: &Shell, system: &System, shown: &mut SendsShown) {
    let now = now_ms();
    let held = shell.held_sends(now);
    let key: Vec<(u64, bool)> = held.iter().map(|s| (s.entry, s.sending)).collect();
    let pid = system.desktop_pid.get();
    if shown.as_ref() == Some(&(pid, key.clone())) {
        return;
    }
    let Some(route) = desktop() else {
        return;
    };
    let wire: Vec<deskcore::sends::WireSend> = held
        .into_iter()
        .map(|s| deskcore::sends::WireSend {
            entry: s.entry,
            sending: s.sending,
            left_ms: s.left_ms,
            host: s.host,
        })
        .collect();
    match report_to_desktop(route, &deskcore::sends::encode_sends(&wire)) {
        Ok(answer) if answer == "ok" => *shown = Some((pid, key)),
        Ok(answer) => println!("the panel did not take the held sends: {answer}"),
        Err(why) => println!("the panel did not take the held sends: {why}"),
    }
}

fn now_ms() -> u64 {
    sys::time_ns() / 1_000_000
}

/// Shows on the panel what the answer to a desktop line's card came to,
/// with the send it held, if it did, so the pane shows that send's countdown
/// and undo as the line's own.
fn give_card_result(ran: bool, line: String, held: Option<u64>) {
    use deskcore::panel::State;
    use deskcore::panel_wire;
    let shown = match ran {
        true => State::Done { line },
        false => State::Refused { reason: line },
    };
    let (Some(route), Some(request)) = (desktop(), panel_wire::encode_card_held(&shown, held))
    else {
        return;
    };
    if let Err(why) = ask_desktop(route, &request) {
        println!("the panel's state was not set: {why}");
    }
}

/// Goes on with an `app_build` in the background, and shows on the panel what
/// one came to when it ends, so it is seen without another line.
fn give_app_build(shell: &mut Shell) {
    use deskcore::panel::State;
    use deskcore::panel_wire;
    let shown = match shell.app_poll() {
        Some(PanelResult::Done(line)) => State::Done { line },
        Some(PanelResult::Refused(reason)) => State::Refused { reason },
        None => return,
    };
    let (Some(route), Some(request)) = (desktop(), panel_wire::encode_state(&shown)) else {
        return;
    };
    if let Err(why) = ask_desktop(route, &request) {
        println!("the panel's state was not set: {why}");
    }
}

// A Terminal line's answer, with its `…`, fits what the transport carries of a
// person's result.
const _: () =
    assert!(shellcore::appline::MAX_ANSWER + '…'.len_utf8() <= deskcore::effects::MAX_LINE_TEXT);

// shellcore's copy of the Settings wire's answer bound is desktop's.
const _: () = assert!(shellcore::SETTINGS_MAX_ANSWER == deskcore::settings_wire::MAX_ANSWER);

/// Takes an app's oldest effects from `desktop`, runs them through the agent
/// core, and reports what came of each; `desktop` hands the results to the
/// app. `desktop` names the app, from its record of who queued them.
fn take_effects(shell: &mut Shell, held: &core::cell::RefCell<waiting::HeldRequests>) {
    // A unit of work that is no request: a Stop for the last one ends here.
    start_of_unit();
    if let Some(route) = desktop() {
        run_effects_taken(shell, held, route, deskcore::effects::TAKE);
    }
}

/// Takes effects from `desktop` with `take` (`effects take`, or after a line
/// `effects take app <name>` for what it gave back), runs them and reports
/// what came of each: what came of the take.
///
/// While the batch runs, its app counts as running in `held`: a Terminal's
/// `shell_line` runs a line, and what the app asks during that line's model
/// call waits behind the rest of the batch.
fn run_effects_taken(
    shell: &mut Shell,
    held: &core::cell::RefCell<waiting::HeldRequests>,
    route: kabi::handle::Handle,
    take: &str,
) -> Took {
    use deskcore::effects;
    let Ok(answer) = ask_desktop(route, take) else {
        return Took::CallFailed;
    };
    let (app, user, lines) = match effects::parse_take(&answer) {
        Some(Some(taken)) => taken,
        Some(None) => return Took::Empty,
        None => return Took::CallFailed,
    };
    held.borrow_mut()
        .running(shellcore::waiting::Kind::Effects, app);
    let results = shell.run_effects_from(app, lines, user);
    held.borrow_mut().done();
    give_effect_results(route, app, user, results);
    Took::Ran
}

/// Gives `desktop` what app `app`'s effects came to, for its `comphost`.
/// Nothing when none has a result yet: a `shell_card` whose question waits
/// for the card is held, and its result goes with the card's answer
/// (`give_call_results`).
fn give_effect_results(
    route: kabi::handle::Handle,
    app: &str,
    user: bool,
    results: Vec<shellcore::EffectDone>,
) {
    if results.is_empty() {
        return;
    }
    // The line ended: a Stop pressed as it did must not hide what ran.
    if let Err(why) = report_to_desktop(route, &effects_done(app, user, results)) {
        println!("{app}'s effect results were not sent: {why}");
    }
}

/// The message that gives `desktop` what app `app`'s effects came to.
fn effects_done(app: &str, user: bool, results: Vec<shellcore::EffectDone>) -> String {
    use deskcore::effects::{self, Done};
    let results: Vec<Done> = results
        .into_iter()
        .map(|done| Done {
            ticket: done.ticket,
            outcome: done.outcome,
        })
        .collect();
    effects::encode_done_as(app, user, &results)
}

/// Hands each app what came of the effects its calls queued: the call's reply
/// carried them, so `desktop` never held them, and without this the app is never
/// told (a Files window the agent opened a folder in would show nothing). They go
/// as `take_effects`' results do, and `desktop` gives them to the app's `comphost`.
/// What came of a Terminal line's question on the card goes this way too, as
/// its held `shell_card`'s result (#922), cut as a person's result is, as the
/// line's own answer is.
fn give_call_results(shell: &mut Shell) {
    let calls = shell.take_effect_results().into_iter();
    let cards = shell.take_card_answers().into_iter();
    let batches: Vec<(String, bool, Vec<shellcore::EffectDone>)> = calls
        .map(|(app, done)| (app, false, done))
        .chain(cards.map(|(app, done)| (app, true, done)))
        .collect();
    if batches.is_empty() {
        return;
    }
    let Some(route) = desktop() else {
        for (app, _, _) in &batches {
            println!("{app}'s effect results were dropped: there is no desktop");
        }
        return;
    };
    for (app, user, done) in batches {
        // After the line that called: its Stop must not hide them.
        if let Err(why) = report_to_desktop(route, &effects_done(&app, user, done)) {
            println!("{app}'s effect results were not sent: {why}");
        }
    }
}

// The shell and `desktop` name the one process that may ask alike.
const _: () = assert!(same(
    shellcore::SETTINGS_APP,
    deskcore::settings_wire::SETTINGS_APP
));
// The same for the Models window's switches (#556).
const _: () = assert!(same(
    shellcore::MODELS_APP,
    deskcore::models_wire::MODELS_APP
));
// The same for the editor's file door.
const _: () = assert!(same(shellcore::door::DOOR_APP, deskcore::files::DOOR_APP));

/// One request to `desktop`, answered in bytes: a file's piece is not always text.
fn ask_desktop_bytes(route: kabi::handle::Handle, request: &[u8]) -> Result<Vec<u8>, String> {
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let len = sys::call_route(route, request, &mut reply, DESKTOP_CALL_MS)
        .map_err(|error| format!("desktop did not answer ({error:?})"))?;
    reply.truncate(len.min(reply.len()));
    Ok(reply)
}

/// Most files taken in one pass of the loop.
const FILES_PER_PASS: usize = 4;

/// The editor's files, a few at a time: each complete transfer `desktop` holds is
/// done through the agent core's door (`Shell::door_read`, `Shell::door_write`),
/// and what came of it goes back, a write's reason or a read's bytes.
fn take_files(shell: &mut Shell) {
    // A unit of work that is no request: a Stop for the last one ends here.
    start_of_unit();
    let Some(route) = desktop() else {
        return;
    };
    for _ in 0..FILES_PER_PASS {
        if run_file_taken(shell, route) != Took::Ran {
            return;
        }
    }
}

/// Takes the oldest complete transfer `desktop` holds, does it, and reports
/// it: what came of the take.
fn run_file_taken(shell: &mut Shell, route: kabi::handle::Handle) -> Took {
    let Ok(answer) = ask_desktop(route, "files take") else {
        return Took::CallFailed;
    };
    if answer == "none" {
        return Took::Empty;
    }
    let Some(job) = deskcore::files::parse_job(&answer) else {
        return Took::CallFailed;
    };
    let done = do_file_job(shell, route, &job);
    report_file_job(route, job.id, done);
    Took::Ran
}

/// Tells `desktop` what transfer `id` came to: a Stop for a line before
/// it, a held transfer's, must not hide it.
fn report_file_job(route: kabi::handle::Handle, id: u32, done: Result<(), String>) {
    if let Err(why) = report_to_desktop(route, &file_done(id, done)) {
        println!("the editor's file {id} was not reported: {why}");
    }
}

/// The message that tells `desktop` what transfer `id` came to.
fn file_done(id: u32, done: Result<(), String>) -> String {
    match done {
        Ok(()) => format!("files done {id} ok"),
        Err(why) => {
            // One line, as `desktop` keeps it.
            let why: String = why
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            format!("files done {id} err {}", why.trim())
        }
    }
}

/// Fills a read's bytes, or a build's report, into the transfer in pieces,
/// as [`report_file_job`] reports it.
fn fill_job(route: kabi::handle::Handle, id: u32, bytes: &[u8]) -> Result<(), String> {
    for message in file_fills(id, bytes) {
        if report_to_desktop_bytes(route, &message)? != b"ok" {
            return Err(String::from("desktop did not take the file"));
        }
    }
    Ok(())
}

/// The messages that fill `bytes` into transfer `id`, a piece each.
fn file_fills(id: u32, bytes: &[u8]) -> Vec<Vec<u8>> {
    use deskcore::files::CHUNK;
    let total = bytes.len();
    bytes
        .chunks(CHUNK)
        .enumerate()
        .map(|(i, piece)| {
            let mut message = format!("files fill {id} {} {total}\n", i * CHUNK).into_bytes();
            message.extend_from_slice(piece);
            message
        })
        .collect()
}

/// One transfer: a write fetches its pieces and saves; a read reads and fills; a
/// build builds the component's C sources as a typed `cc build` does and fills its
/// report, which takes as long as the build.
fn do_file_job(
    shell: &mut Shell,
    route: kabi::handle::Handle,
    job: &deskcore::files::Job,
) -> Result<(), String> {
    use deskcore::files::Op;
    match job.op {
        Op::Read => {
            let bytes = shell.door_read(&job.app, &job.path)?;
            fill_job(route, job.id, &bytes)
        }
        Op::Build => {
            let report = shell.door_build(&job.app, &job.path, job.user)?;
            fill_job(route, job.id, report.as_bytes())
        }
        Op::Write { create } => {
            let mut bytes: Vec<u8> = Vec::with_capacity(job.total);
            while bytes.len() < job.total {
                let request = format!("files fetch {} {}", job.id, bytes.len());
                let reply = ask_desktop_bytes(route, request.as_bytes())?;
                let piece = reply
                    .strip_prefix(b"ok\n")
                    .filter(|piece| !piece.is_empty())
                    .ok_or_else(|| String::from("desktop did not give the file"))?;
                bytes.extend_from_slice(piece);
            }
            shell.door_write(&job.app, &job.path, &bytes, create, job.user)
        }
    }
}

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut at = 0;
    while at < a.len() {
        if a[at] != b[at] {
            return false;
        }
        at += 1;
    }
    true
}

/// What the shell does with one app's request: `Shell::settings` for the
/// Settings app's, `Shell::models_change` for the Models window's switches.
type Answering = fn(&mut Shell, &str, &str, bool) -> Result<String, String>;

/// Answers one app's waiting requests, which `desktop` holds under `words`
/// (`deskcore::settings_wire`): the Settings app's `kai:settings` requests, or
/// the Models window's switches (`deskcore::models_wire`). The app is
/// `desktop`'s record of the caller, and `answering` checks it again. What
/// came of the take.
fn take_changes(
    shell: &mut Shell,
    words: &deskcore::settings_wire::Words,
    answering: Answering,
) -> Took {
    use deskcore::settings_wire::{self as wire, Answer};
    // A unit of work that is no request: a Stop for the last one ends here.
    start_of_unit();
    let Some(route) = desktop() else {
        return Took::CallFailed;
    };
    let Ok(answer) = ask_desktop(route, words.take) else {
        return Took::CallFailed;
    };
    let (app, requests) = match wire::parse_take(&answer) {
        Some(Some(taken)) => taken,
        Some(None) => return Took::Empty,
        None => return Took::CallFailed,
    };
    let answers: Vec<Answer> = requests
        .iter()
        .map(|r| Answer::new(r.ticket, answering(shell, app, &r.text, r.user)))
        .collect();
    give_answers(route, words, app, &answers);
    Took::Ran
}

/// Gives `desktop` app `app`'s answers under `words`: a Stop for a line
/// before them, held ones', must not hide them.
fn give_answers(
    route: kabi::handle::Handle,
    words: &deskcore::settings_wire::Words,
    app: &str,
    answers: &[deskcore::settings_wire::Answer],
) {
    use deskcore::settings_wire as wire;
    for done in wire::encode_done_chunks_for(words, app, answers, kabi::MAX_MESSAGE) {
        if let Err(why) = report_to_desktop(route, &done) {
            println!(
                "{app}'s {} answers were not sent: {why}",
                words.word.trim_end()
            );
        }
    }
}

/// Answers the apps' waiting network asks, which `desktop` holds
/// (`deskcore::net_wire`): the app is `desktop`'s record of the caller. A new
/// host waits as a card, which the kernel's terminal takes; the app's host
/// asks again until the answer is not `asked`.
fn take_net(shell: &mut Shell) {
    use deskcore::net_wire as wire;
    // A unit of work that is no request: a Stop for the last one ends here.
    start_of_unit();
    let Some(route) = desktop() else {
        return;
    };
    for _ in 0..wire::MAX_WAITING {
        let Ok(answer) = ask_desktop(route, wire::TAKE) else {
            return;
        };
        let Some(Some(ask)) = wire::parse_take(&answer) else {
            return;
        };
        let connect = shell.net_connect(&ask.app, &ask.host, &ask.ceiling).wire();
        let done = wire::encode_done(&ask.app, &ask.host, &connect);
        if let Err(why) = ask_desktop(route, &done) {
            println!("{}'s network answer was not sent: {why}", ask.app);
        }
    }
}

/// The supervisor's wait for a stop or a start: a stop waits for the process to end.
const SUPERVISOR_CALL_MS: u64 = 10_000;

/// Sends `stop`, `start` or `restart` to the supervisor: `ok`, or `err <why>`.
fn supervise(request: &str) -> Result<(), String> {
    let text = ask_supervisor(request)?;
    match text.split_once(' ').unwrap_or((&text, "")) {
        ("ok", _) => Ok(()),
        ("err", why) => Err(why.to_string()),
        _ => Err(String::from("the supervisor's answer was malformed")),
    }
}

/// A stop or restart carrying what lets it run, in the supervisor's own words
/// (`supervise::Request::encode`): `once=<badge> id=<entry>`, `rule`, or for a
/// stop, `undo id=<entry>`. One the supervisor would refuse is not sent.
fn with_proof(request: supervise::Request, proof: Proof) -> Result<String, String> {
    let proof = match proof {
        Proof::Once { badge, id } => supervise::Proof::Once(supervise::Once { badge, id }),
        Proof::Rule => supervise::Proof::Rule,
        Proof::Undo { id } => supervise::Proof::Undo(id),
        Proof::None => supervise::Proof::None,
    };
    supervise::proof_fits(request, proof)
        .map_err(|_| String::from("a stop or restart needs the menu's Allow or a rule"))?;
    Ok(request.encode(proof))
}

/// How long an app's call may take through the supervisor: its start, then
/// the call (`supervise::appcall`).
const APP_CALL_MS: u64 = 8_000;

/// `request` to app `app` through the supervisor's `app-call` (APP_CALL): the
/// app's reply, or why not. The supervisor waits at most 5 s on one call; a
/// timeout is tried once more.
fn ask_app(app: &str, request: &str) -> Result<String, String> {
    match ask_app_once(app, request) {
        Err(why) if why.starts_with("timeout") => {
            println!("{app} did not answer in time; asking once more");
            ask_app_once(app, request)
        }
        other => other,
    }
}

fn ask_app_once(app: &str, request: &str) -> Result<String, String> {
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let call = supervise::appcall::encode(app, request);
    let len = sys::call_route(supervisor(), call.as_bytes(), &mut reply, APP_CALL_MS)
        .map_err(|error| format!("the supervisor did not answer ({error:?})"))?;
    let text = String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned();
    match text.strip_prefix("err ") {
        Some(why) => Err(why.to_string()),
        None => Ok(text),
    }
}

/// Sends `request` to the supervisor over the route; its answer.
fn ask_supervisor(request: &str) -> Result<String, String> {
    let mut reply = vec![0u8; 2048];
    let len = sys::call_route(
        supervisor(),
        request.as_bytes(),
        &mut reply,
        SUPERVISOR_CALL_MS,
    )
    .map_err(|error| format!("the supervisor did not answer ({error:?})"))?;
    let text = core::str::from_utf8(&reply[..len.min(reply.len())]).unwrap_or("err not text");
    Ok(text.to_string())
}

impl System {
    /// One `act` request to llm. With a `poll`, the kernel holds the call
    /// while `poll` looks at what the apps ask ([`waiting::call_serving`]);
    /// without, the shell waits in it.
    fn act_with(
        &self,
        request: &ActRequest,
        poll: Option<&mut dyn FnMut()>,
    ) -> Result<Call, ActError> {
        let unavailable = |why: &str| ActError::Unavailable(alloc::format!("llm service: {why}"));
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let request = request.encode();
        let len = match poll {
            Some(poll) => {
                waiting::call_serving(llm(), request.as_bytes(), &mut reply, SCRIPT_CALL_MS, poll)
            }
            None => sys::call_route(llm(), request.as_bytes(), &mut reply, SCRIPT_CALL_MS),
        }
        .map_err(|error| match error {
            // The person's Stop (#887): the line ends here, with no second
            // try as a script.
            kabi::Error::Interrupted => ActError::Stopped,
            error => unavailable(&alloc::format!("{error:?}")),
        })?;
        let text = core::str::from_utf8(&reply[..len.min(reply.len())])
            .map_err(|_| unavailable("the reply is not UTF-8"))?;
        match text.split_once(' ').unwrap_or((text, "")) {
            ("ok", answer) => act::decode_answer(answer),
            ("err", why) => Err(unavailable(why)),
            _ => Err(unavailable("malformed reply")),
        }
    }
}

/// Standing instructions' events and actions (P3 task 11). The kernel's event
/// box holds every window, idle and time event, so nothing is asked to watch; a
/// firing goes to its owner, the supervisor, with the rule the Allow gave the
/// shell, which the supervisor checks with RULE_CHECK.
struct Firings;

impl shellcore::standing::Triggers for Firings {
    fn watch(&self, _when: &shellcore::standing::When) -> Result<(), String> {
        Ok(())
    }

    fn unwatch(&self, _when: &shellcore::standing::When) {}

    fn waiting(&self) -> String {
        let mut buf = vec![0u8; kabi::event::READ_BYTES];
        match sys::event_read(&mut buf) {
            Ok(len) => String::from_utf8_lossy(&buf[..len.min(buf.len())]).into_owned(),
            Err(_) => String::new(),
        }
    }

    /// `suspend <app> rule` or `resume <app> rule`; `undo` sends the inverse.
    fn run(&self, _rule: u64, action: &str, app: &str, undo: bool) -> Result<(), String> {
        let request = match (action, undo) {
            ("app_suspend", false) | ("app_resume", true) => supervise::Request::Suspend(app),
            ("app_resume", false) | ("app_suspend", true) => supervise::Request::Resume(app),
            _ => return Err(format!("{action} has no owner to run it here")),
        };
        let text = ask_supervisor(&request.encode(supervise::Proof::Rule))?;
        match text.split_once(' ').unwrap_or((&text, "")) {
            ("ok", _) => Ok(()),
            ("err", why) => Err(why.to_string()),
            _ => Err(format!("the supervisor's answer was malformed: {text}")),
        }
    }
}

/// The longest the shell waits for a line before it looks at the event box
/// again: a post wakes it sooner (`Woken`), so this only paces the minute
/// clock's `time` events.
const WAKE_WAIT_MS: u64 = 2_000;

/// Takes the kernel's event box and hands each event to the standing
/// instructions; a paused rule's note goes to the log.
fn take_events(shell: &mut Shell, buf: &mut [u8]) {
    // A unit of work that is no request: a Stop for the last one ends here.
    start_of_unit();
    let Ok(len) = sys::event_read(buf) else {
        return;
    };
    let text = core::str::from_utf8(&buf[..len.min(buf.len())]).unwrap_or("");
    for note in shell.on_events(text) {
        println!("{note}");
    }
}

fn main() -> i32 {
    let args = args();
    let system = {
        let (files, state) = files::Files::new();
        Rc::new(System {
            pool: apart::Pool::new(),
            files,
            state,
            apps: core::cell::RefCell::default(),
            appearance_due: core::cell::Cell::new(true),
            appearance_tried_ns: core::cell::Cell::new(0),
            desktop_pid: core::cell::Cell::new(0),
            desktop_looked_ns: core::cell::Cell::new(0),
            held: core::cell::RefCell::default(),
        })
    };
    let mut shell = Shell::new(system.clone(), DEFAULT_CONFIG);
    println!("up (restart {})", arg(&args, "restart").unwrap_or("0"));
    let mut line = vec![0u8; kabi::MAX_MESSAGE];
    let mut events = vec![0u8; kabi::event::READ_BYTES];
    // The first wait binds the shell's service, so the next pass's read of the
    // event box is admitted and makes the shell the box's reader; until then a
    // post could not wake it.
    let mut wait_ms = 1;
    let mut warmth = warmth::Warmth::default();
    let mut sends_shown = None;
    let mut woke_before_send = false;
    loop {
        // What the apps asked during a line that was not a read runs before
        // anything newer is taken: after each step that may run a line (the
        // last pass's, the panel's, a Terminal's `shell_line` effect) and
        // before each that takes an app's requests.
        // Each step, and each held request, starts a unit of its own
        // (`start_of_unit`): the person's Stop (#887) lasts the unit it came
        // in, so one for a line that has ended does not cut the next step's
        // calls to `desktop`. A Terminal line's request clears it as it comes.
        let held = &system.held;
        waiting::run_held(&mut shell, held);
        // Between lines: what happened on the desktop since the last look, then
        // the panel's line and the apps' effects. A post to the event box wakes
        // the wait at once, so a click's effect does not wait for a poll.
        take_events(&mut shell, &mut events);
        waiting::run_held(&mut shell, held);
        let mut took = take_panel(&mut shell, &mut warmth);
        waiting::run_held(&mut shell, held);
        // A send falls due: a Terminal line that came meanwhile (an `undo`
        // typed as it did) is answered first, and a wake (a click on the
        // pane's undo) is taken on the next pass, before the send starts;
        // only once in a row, so the desktop's steady events never hold a
        // send back.
        if !took && shell.next_send_ms(now_ms()) == Some(0) {
            took = match serve_terminal(&mut shell, &mut line, POLL_MS) {
                Ok(()) => true,
                Err(kabi::Error::Woken) => !core::mem::replace(&mut woke_before_send, true),
                Err(_) => false,
            };
        }
        if !took {
            woke_before_send = false;
        }
        give_sends(&mut shell, &system, &mut sends_shown, took);
        take_effects(&mut shell, held);
        waiting::run_held(&mut shell, held);
        start_of_unit();
        // Files the AI, the Terminal, an app, an editor save or an undo changed:
        // the apps that list them are told, along with the results given next.
        if system.files.take_changed() {
            shell.fs_changed();
        }
        give_call_results(&mut shell);
        take_files(&mut shell);
        waiting::run_held(&mut shell, held);
        take_net(&mut shell);
        waiting::run_held(&mut shell, held);
        take_changes(
            &mut shell,
            &deskcore::settings_wire::SETTINGS,
            Shell::settings,
        );
        waiting::run_held(&mut shell, held);
        take_changes(
            &mut shell,
            &deskcore::models_wire::MODELS,
            Shell::models_change,
        );
        give_app_build(&mut shell);
        // "Kai is getting ready" on the panel until llm's prompt slots are warm.
        warmth.peek();
        // An install card waits: the kernel's terminal takes it when it may.
        // Said again each pass, so a desktop that shows later still gets it.
        if shell.card_waiting() {
            let _ = sys::card_ready();
        }
        give_appearance(&system);
        shell.retry_keep();
        // A held send is due within the wait: wake for it then.
        if let Some(left) = shell.next_send_ms(now_ms()) {
            wait_ms = wait_ms.min(left.max(1));
        }
        match serve_terminal(&mut shell, &mut line, wait_ms) {
            Ok(()) => {}
            Err(kabi::Error::TimedOut | kabi::Error::Woken) => wait_ms = WAKE_WAIT_MS,
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
            }
        }
    }
}

/// The shortest wait `svc_recv` takes (0 is its longest): a look for a
/// request already there.
const POLL_MS: u64 = 1;

/// Takes the next request to the shell's service within `wait_ms`, into
/// `line`, and answers it: only the kernel terminal may run shell lines.
fn serve_terminal(shell: &mut Shell, line: &mut [u8], wait_ms: u64) -> Result<(), kabi::Error> {
    let mut caller = u64::MAX;
    let len = sys::svc_recv(line, wait_ms, &mut caller)?.min(line.len());
    let mut reply = if caller == 0 {
        let text = core::str::from_utf8(&line[..len]).unwrap_or("");
        // An answer to a menu goes on with its run; any other line starts one.
        if !text.starts_with("answer ") {
            ui::begin();
        }
        let reply = shell.eval_console(text).to_wire();
        // A panel line's card was answered: the panel shows what came of
        // it. A Terminal line's answer goes back to the Terminal on the
        // next pass (`give_call_results`).
        if let Some((ran, line)) = shell.take_card_result() {
            let held = shell.take_card_held();
            give_card_result(ran, line, held);
        }
        reply
    } else {
        String::from("err the shell answers only the terminal")
    };
    reply.truncate(reply.floor_char_boundary(kabi::MAX_MESSAGE));
    if let Err(error) = sys::svc_reply(reply.as_bytes()) {
        println!("svc_reply failed: {error:?}");
    }
    Ok(())
}
