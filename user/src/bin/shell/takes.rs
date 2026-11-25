//! The desktop's queues the shell takes from between lines: an app's
//! effects and their results, the Terminal's call results, the editor's and
//! the apps' file jobs, Settings' and Models' changes, and the apps' network
//! asks.

use super::*;

// A Terminal line's answer, with its `…`, fits what the transport carries of a
// person's result.
const _: () =
    assert!(shellcore::appline::MAX_ANSWER + '…'.len_utf8() <= deskcore::effects::MAX_LINE_TEXT);

// shellcore's copy of the Settings wire's answer bound is desktop's.
const _: () = assert!(shellcore::SETTINGS_MAX_ANSWER == deskcore::settings_wire::MAX_ANSWER);

/// Takes an app's oldest effects from `desktop`, runs them through the agent
/// core, and reports what came of each; `desktop` hands the results to the
/// app. `desktop` names the app, from its record of who queued them.
pub(crate) fn take_effects(shell: &mut Shell, held: &core::cell::RefCell<waiting::HeldRequests>) {
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
pub(crate) fn run_effects_taken(
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
pub(crate) fn give_effect_results(
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
pub(crate) fn effects_done(app: &str, user: bool, results: Vec<shellcore::EffectDone>) -> String {
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
pub(crate) fn give_call_results(shell: &mut Shell) {
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
pub(crate) fn ask_desktop_bytes(
    route: kabi::handle::Handle,
    request: &[u8],
) -> Result<Vec<u8>, String> {
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let len = sys::call_route(route, request, &mut reply, DESKTOP_CALL_MS)
        .map_err(|error| format!("desktop did not answer ({error:?})"))?;
    reply.truncate(len.min(reply.len()));
    Ok(reply)
}

/// Most files taken in one pass of the loop.
pub(crate) const FILES_PER_PASS: usize = 4;

/// The editor's files, a few at a time: each complete transfer `desktop` holds is
/// done through the agent core's door (`Shell::door_read`, `Shell::door_write`),
/// and what came of it goes back, a write's reason or a read's bytes.
pub(crate) fn take_files(shell: &mut Shell) {
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
pub(crate) fn run_file_taken(shell: &mut Shell, route: kabi::handle::Handle) -> Took {
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
pub(crate) fn report_file_job(route: kabi::handle::Handle, id: u32, done: Result<(), String>) {
    if let Err(why) = report_to_desktop(route, &file_done(id, done)) {
        println!("the editor's file {id} was not reported: {why}");
    }
}

/// The message that tells `desktop` what transfer `id` came to.
pub(crate) fn file_done(id: u32, done: Result<(), String>) -> String {
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
pub(crate) fn fill_job(route: kabi::handle::Handle, id: u32, bytes: &[u8]) -> Result<(), String> {
    for message in file_fills(id, bytes) {
        if report_to_desktop_bytes(route, &message)? != b"ok" {
            return Err(String::from("desktop did not take the file"));
        }
    }
    Ok(())
}

/// The messages that fill `bytes` into transfer `id`, a piece each.
pub(crate) fn file_fills(id: u32, bytes: &[u8]) -> Vec<Vec<u8>> {
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
pub(crate) fn do_file_job(
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

pub(crate) const fn same(a: &str, b: &str) -> bool {
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
pub(crate) type Answering = fn(&mut Shell, &str, &str, bool) -> Result<String, String>;

/// Answers one app's waiting requests, which `desktop` holds under `words`
/// (`deskcore::settings_wire`): the Settings app's `kai:settings` requests, or
/// the Models window's switches (`deskcore::models_wire`). The app is
/// `desktop`'s record of the caller, and `answering` checks it again. What
/// came of the take.
pub(crate) fn take_changes(
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
pub(crate) fn give_answers(
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
pub(crate) fn take_net(shell: &mut Shell) {
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
