//! The terminal: a prompt with line editing. Each line goes to the ring-3 Rhai
//! shell, whose answer is printed with a `= ` (result), `! ` (error) or `? ` (a
//! question waiting for y/n) lead that no kernel message uses, so shell output can
//! never pass for kernel output. Lines starting with `:` are kernel commands, which
//! work even when the shell is down. Most scripts run at once; after a `? ` reply
//! the prompt changes and the next line, whatever it is, answers the question:
//! only `y` or `yes` runs the script, and `show` prints it and asks again.
//! In edit mode (`:edit`), plain lines go to the editor instead, and `:apply` asks
//! the same way before an upload runs. A call that asks shows the Allow / Deny
//! menu (`termcore::menu`), drawn here and nowhere else: the shell replies
//! `menu ...`, keys pick a choice, and the terminal answers `answer <id> <word>`.
//! Every question is answered on the console: while one waits and `desktop`
//! holds the screen, the console takes it (`display::claim::take_for_answer`)
//! and gives it back after, so no key typed in a window answers (ADR 0005 §7).

use crate::console::{self, FORM_FEED, Screen};
use crate::display::claim;
use crate::keyboard::Decoder;
use crate::{ask_box, edit_mode, hpet, ipc, llm, log, memory, net, serial, smp, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use llmproto::text::{printable, wrap};
use termcore::line::{Line, Outcome};
use termcore::menu::{self, Answer, Gate, Menu, Step};
use termcore::route::{Route, route};

const PROMPT: &str = "kai> ";
const MAX_LINE: usize = 200;
/// Longest a shell line may take: the shell's longest LLM wait (`tune`: a 3 s
/// window, then `llmproto::tune::CALL_TIMEOUT_MS` for the answer; 140 s for `ask`;
/// two script rounds of 70 s) plus its compute budget, with room to spare, so the
/// shell always answers first.
const SHELL_TIMEOUT_MS: u64 = 310_000;
const _: () = assert!(llmproto::tune::CALL_TIMEOUT_MS + 10_000 <= SHELL_TIMEOUT_MS);
/// How long to wait for a shell that is starting or restarting.
const SHELL_START_WAIT_NS: u64 = 3_000_000_000;
/// Longest piece of a reply printed in one go (each print keeps interrupts off).
pub(crate) const PIECE_CHARS: usize = 160;
/// Characters of the `ai: ` an `:ask` answer's rows start with.
const AI_LEAD: usize = 4;
const MAX_REPLY_LINES: usize = 64;

/// Shell replies printed in full, for the self-test.
static REPLIES_SHOWN: AtomicU64 = AtomicU64::new(0);
/// Set while the terminal waits for the answer to a confirmation, for the self-test.
static AWAITING_ANSWER: AtomicBool = AtomicBool::new(false);
/// Typed lines Ctrl-C dropped, for the self-test.
static LINES_CLEARED: AtomicU64 = AtomicU64::new(0);
/// When the last line was entered and when the last shell reply was on screen, in
/// HPET nanoseconds, for the latency case.
static ENTERED_NS: AtomicU64 = AtomicU64::new(0);
static REPLIED_NS: AtomicU64 = AtomicU64::new(0);

/// When the last line was entered, and when the last shell reply was shown in
/// full, in HPET nanoseconds.
pub fn last_times() -> (u64, u64) {
    (
        ENTERED_NS.load(Ordering::Relaxed),
        REPLIED_NS.load(Ordering::Relaxed),
    )
}

/// Shell replies the terminal has printed in full, for the self-tests.
pub fn replies_shown() -> u64 {
    REPLIES_SHOWN.load(Ordering::Relaxed)
}

/// Whether the terminal shows a confirmation and waits for its answer.
pub fn awaiting_answer() -> bool {
    AWAITING_ANSWER.load(Ordering::Acquire)
}

const HELP: &[(&str, &str)] = &[
    (":help", "this list"),
    (":ask <text>", "send <text> straight to the llm service"),
    (
        ":policy [interactive|rr|classes firm]",
        "show or switch the scheduling policy on every CPU",
    ),
    (":threads", "scheduler statistics"),
    (":cpus", "CPUs online"),
    (":mem", "free memory and heap use"),
    (
        ":net",
        "address, gateway, DNS, lease and the model's health",
    ),
    (":resolve <host>", "look up a host name's IPv4 addresses"),
    (":ping <host|ip> [n]", "ICMP echo, n times (4 by default)"),
    (":ps", "running processes"),
    (":uptime", "time since the clock started"),
    (
        ":mouse",
        "the last pointer report and where it left the pointer",
    ),
    (
        ":log [n]",
        "the latest n lines kept off the screen (30 by default)",
    ),
    (":clear", "clear the screen"),
    (
        ":c [entry]",
        "C mode (c>): each entry compiles and runs on Enter; or one entry",
    ),
    (":rhai", "back from C mode to kai>"),
    (
        ":dis f / :defs / :reset",
        "at c>: f's code, the definitions, a new session",
    ),
];

/// At `c>` rather than `kai>` (ADR 0012 D1): set and cleared by the shell's
/// answers to `:c` and `:rhai`, and cleared when the shell restarts.
static C_MODE: AtomicBool = AtomicBool::new(false);
/// The shell's pid at its last line, to see a restart.
static SHELL_PID: AtomicU64 = AtomicU64::new(0);
/// The C mode prompt.
const C_PROMPT: &str = "c> ";

/// Lines `:log` shows when it is not given a number.
const LOG_LINES: usize = 30;

/// What the next line is for, when it is not a new line at the prompt.
enum Waiting {
    /// A script or a tune trial the shell proposed.
    Confirm(Confirm),
    /// An upload `:apply` showed.
    Apply(edit_mode::Proposal),
    /// The shell replied `more`: the entry has a bracket open, and the next line
    /// continues it.
    More,
    /// A call that asks: the Allow / Deny menu.
    Menu(Menu),
}

/// The prompt for a line that continues an open entry (ADR 0012).
const CONTINUE_PROMPT: &str = "..> ";
/// Sent to the shell when Ctrl-C clears a continuation line; it drops the entry
/// (`shellcore::CANCEL_ENTRY`).
const CANCEL_ENTRY: &str = "\u{3}";

/// A confirmation the shell is waiting for: its id, and whether it proposes a
/// tune trial (its card starts `Tune "`) rather than a script (`script N:`).
#[derive(Clone, Copy)]
struct Confirm {
    id: u64,
    tune: bool,
}

impl Confirm {
    fn prompt(self) -> String {
        match self.tune {
            true => format!("start trial {}? yes/no> ", self.id),
            false => format!("run script {}? y/n> ", self.id),
        }
    }
}

/// Runs the terminal on the calling thread, forever.
pub fn run(_: usize) {
    console::status::start();
    let mut decoder = Decoder::new();
    log!(
        "Kernel AI terminal. Talk to the AI or type Rhai (try help); kernel commands start with ':'."
    );
    let mut waiting: Option<Waiting> = None;
    loop {
        if let Some(Waiting::Menu(menu)) = waiting {
            waiting = choose(&mut decoder, menu);
            continue;
        }
        let prompt = match &waiting {
            Some(Waiting::Confirm(confirm)) => confirm.prompt(),
            Some(Waiting::Apply(p)) => format!("apply {} to {}? yes/no> ", p.label, p.component),
            Some(Waiting::More) => CONTINUE_PROMPT.into(),
            Some(Waiting::Menu(_)) | None => {
                edit_mode::prompt().unwrap_or_else(|| match C_MODE.load(Ordering::Relaxed) {
                    true => C_PROMPT.into(),
                    false => PROMPT.into(),
                })
            }
        };
        let asks = matches!(waiting, Some(Waiting::Confirm(_) | Waiting::Apply(_)));
        // The question is answered on the console, and only by keys typed once
        // it is on screen: the flag first, then the screen, then the drop.
        if asks {
            AWAITING_ANSWER.store(true, Ordering::Release);
        }
        let took = asks && claim::take_for_answer();
        console::show_input(&prompt, "", 0);
        if asks {
            decoder.discard_pending();
        }
        let line = read_line(&mut decoder, &prompt);
        AWAITING_ANSWER.store(false, Ordering::Release);
        claim::give_back(took);
        // Ctrl-C: prompt again, with a pending question still asked; an open entry
        // is dropped.
        let Some(line) = line else {
            if let Some(Waiting::More) = waiting {
                waiting = shell_line(CANCEL_ENTRY);
            }
            continue;
        };
        let line = line.trim();
        match waiting.take() {
            Some(Waiting::Confirm(confirm)) => {
                waiting = shell_line(&format!("answer {} {line}", confirm.id));
                continue;
            }
            Some(Waiting::Apply(proposal)) => {
                edit_mode::show(&edit_mode::answer(&proposal, line));
                continue;
            }
            Some(Waiting::More) => {
                waiting = shell_line(line);
                continue;
            }
            Some(Waiting::Menu(_)) | None => {}
        }
        let editing = edit_mode::editing().is_some();
        match route(line, C_MODE.load(Ordering::Relaxed)) {
            Route::Mode => waiting = shell_mode_line(line),
            Route::ShellCommand => waiting = shell_line(line),
            Route::Kernel => waiting = kernel_command(line[1..].trim()),
            Route::Nothing => {}
            Route::Refused | Route::Shell if editing => {
                edit_mode::show(&edit_mode::editor_line(line));
            }
            Route::Refused => log!("! scripts are answered only at their own prompt"),
            Route::Shell => waiting = shell_line(line),
        }
    }
}

/// Sends `line` to the shell as if typed, and answers each menu it asks on its
/// one surface, as [`run`] does. Whether one was asked. For boot cases that
/// need the shell's own ask (`desk-standing`).
pub(crate) fn shell_line_answered(line: &str) -> bool {
    let mut decoder = Decoder::new();
    let mut waiting = shell_line(line);
    let mut asked = false;
    while let Some(Waiting::Menu(menu)) = waiting {
        asked = true;
        waiting = choose(&mut decoder, menu);
    }
    asked
}

/// Shows `menu` on the console, reads its answer, then sends it to the shell
/// with what an Allow minted.
fn choose(decoder: &mut Decoder, mut menu: Menu) -> Option<Waiting> {
    let answer = answer(decoder, &mut menu);
    let (prompt, _) = menu.input();
    finish_line(prompt, answer.word(), "");
    // Allow mints the call's ONCE (and an "Always allow" its rule node); the
    // request ends once the shell has answered, so the ONCE cannot be used later.
    match crate::allow::allow(menu.id, menu.call(), answer) {
        Ok(Some(allowed)) => {
            let line = menu::granted_line(menu.id, answer, allowed.granted);
            let waiting = shell_line(&line);
            crate::allow::end(&allowed);
            waiting
        }
        Ok(None) => shell_line(&menu::answer_line(menu.id, answer)),
        Err(error) => {
            log!("! the Allow could not be granted ({error:?}), so it was denied");
            shell_line(&menu::answer_line(menu.id, Answer::Deny))
        }
    }
}

/// Asks `menu` on its one answering surface, which the kernel picks: `desktop`'s
/// ask card while `desktop` shows and its trusted layer is alive, else the
/// console (ADR 0005 §7). A card withdrawn before an answer, because `desktop`
/// stopped showing or reading, goes to the console after.
pub(crate) fn answer(decoder: &mut Decoder, menu: &mut Menu) -> Answer {
    match answer_on_card(menu) {
        Some(answer) => answer,
        None => answer_on_console(decoder, menu),
    }
}

/// Posts `menu` to `desktop`'s card and waits for its answer: None if the card
/// was never shown or was withdrawn unanswered. Unanswered after the card's
/// countdown, it is Deny.
fn answer_on_card(menu: &Menu) -> Option<Answer> {
    if !claim::showing() || !ask_box::alive() {
        return None;
    }
    if !ask_box::post(menu.id, menu.call(), menu.question()) {
        return None;
    }
    AWAITING_ANSWER.store(true, Ordering::Release);
    log!("ask: {} went to desktop's card", menu.id);
    let start = hpet::nanos();
    let answer = loop {
        if let Some(answered) = ask_box::finish(false) {
            break answered;
        }
        let gone = !claim::showing() || !ask_box::alive();
        let late = hpet::nanos().saturating_sub(start) > ask_box::WAIT_NS;
        if gone || late {
            match ask_box::finish(true) {
                Some(Some(answer)) => break Some(answer),
                _ if late => break Some(Answer::Deny),
                _ => {
                    log!(
                        "ask: {} came back from the card; the console asks it",
                        menu.id
                    );
                    break None;
                }
            }
        }
        threads::sleep_ms(20);
    };
    AWAITING_ANSWER.store(false, Ordering::Release);
    if let Some(answer) = answer {
        log!(
            "ask: {} answered {} on desktop's card",
            menu.id,
            answer.word()
        );
    }
    answer
}

/// Shows `menu` on the console and reads keys until one picks a choice. The
/// console takes the screen from `desktop` first, and gives it back after. Keys
/// typed before the menu showed are dropped, and the menu's gate takes none for
/// its settle, none while a key held then is down, and no auto-repeat; Ctrl-C
/// and Esc are Deny.
pub(crate) fn answer_on_console(decoder: &mut Decoder, menu: &mut Menu) -> Answer {
    AWAITING_ANSWER.store(true, Ordering::Release);
    let took = claim::take_for_answer();
    for row in menu.rows() {
        log!("{row}");
    }
    let (prompt, text) = menu.input();
    console::show_input(prompt, &text, text.chars().count());
    decoder.discard_pending();
    menu.arm(Gate::new(now_ms(), decoder.held()));
    let answer = loop {
        match menu.press(&decoder.read_stroke(), now_ms()) {
            Step::Ignored => {}
            Step::Moved => {
                let (prompt, text) = menu.input();
                console::show_input(prompt, &text, text.chars().count());
            }
            Step::Chosen(answer) => break answer,
        }
    };
    AWAITING_ANSWER.store(false, Ordering::Release);
    claim::give_back(took);
    answer
}

/// HPET time in milliseconds, for the menu's gate.
fn now_ms() -> u64 {
    hpet::nanos() / 1_000_000
}

/// Reads a line into the input region after `prompt`, edited at the cursor. Typed
/// keys go only to the screen; serial gets the whole line, with its prompt, on Enter.
/// None when Ctrl-C clears it.
fn read_line(decoder: &mut Decoder, prompt: &str) -> Option<String> {
    let mut line = Line::new(MAX_LINE);
    loop {
        match line.apply(decoder.read_key()) {
            Outcome::Unchanged => {}
            Outcome::Changed => console::show_input(prompt, line.text(), line.cursor()),
            Outcome::Submitted(text) => {
                ENTERED_NS.store(hpet::nanos(), Ordering::Relaxed);
                finish_line(prompt, &text, "");
                return Some(text);
            }
            Outcome::Cleared(text) => {
                LINES_CLEARED.fetch_add(1, Ordering::Relaxed);
                finish_line(prompt, &text, "^C");
                return None;
            }
        }
    }
}

/// Ends a typed line: the input region goes, and the line is logged in its place
/// with its prompt and `mark` after it.
fn finish_line(prompt: &str, text: &str, mark: &str) {
    console::hide_input();
    log!("{prompt}{text}{mark}");
}

/// Sends a line to the shell and prints its answer. Returns what the next line is
/// for: a script that waits for the user's yes, or the rest of an open entry.
fn shell_line(line: &str) -> Option<Waiting> {
    let reply = shell_reply(line)?;
    show_shell_reply(&reply)
}

/// `:c`, `:rhai` and the C session's commands: the shell's reply, not the
/// line, decides the prompt, so a shell that refused keeps `kai> `.
fn shell_mode_line(line: &str) -> Option<Waiting> {
    let reply = shell_reply(line)?;
    if reply.starts_with(b"ok C mode") || reply == b"ok already in C mode" {
        C_MODE.store(true, Ordering::Relaxed);
    } else if reply == b"ok back at kai>" || reply == b"ok already at kai>" {
        C_MODE.store(false, Ordering::Relaxed);
    }
    show_shell_reply(&reply)
}

/// Sends `line` to the shell: its reply, or `None` once the console says why
/// there is none. A shell that restarted comes back at `kai>`, so a new pid
/// clears C mode, and a line typed at `c>` for the old shell is not sent.
fn shell_reply(line: &str) -> Option<Vec<u8>> {
    let start_deadline = hpet::nanos() + SHELL_START_WAIT_NS;
    if ipc::wait_bound("shell", start_deadline, || false).is_err() {
        log!("shell not running; kernel commands start with ':' (try :help)");
        return None;
    }
    if let Some(pid) = ipc::server_of("shell")
        && SHELL_PID.swap(pid, Ordering::Relaxed) != pid
        && C_MODE.swap(false, Ordering::Relaxed)
    {
        // The line was typed as C: a new shell would read it as Rhai.
        log!("! the shell restarted at kai>; the line was not sent (:c goes back to C)");
        return None;
    }
    console::status::set_busy(true);
    let reply = ipc::call("shell", line.as_bytes(), SHELL_TIMEOUT_MS);
    console::status::set_busy(false);
    match reply {
        Ok(reply) => Some(reply),
        Err(error) => {
            log!("shell: no answer ({error:?})");
            None
        }
    }
}

/// Prints `ok <text>`, `err <text>` or `confirm <id> <text>` with its lead, one
/// complete line per piece, and returns a confirmation shown in full. `more` prints
/// nothing and asks for the entry's next line.
fn show_shell_reply(reply: &[u8]) -> Option<Waiting> {
    if reply == b"more" {
        return Some(Waiting::More);
    }
    if reply.starts_with(b"menu ") {
        return menu_reply(reply);
    }
    let text = String::from_utf8_lossy(reply);
    let clean = printable(&text, kabi::MAX_MESSAGE, MAX_REPLY_LINES);
    let (kind, rest) = clean.split_once(' ').unwrap_or((clean.as_str(), ""));
    let (lead, body, confirm) = match kind {
        "ok" => ("=", rest, None),
        "err" => ("!", rest, None),
        "confirm" => match rest.split_once(' ') {
            Some((id, body)) => ("?", body, id.parse::<u64>().ok()),
            None => ("!", "the shell sent a malformed reply", None),
        },
        _ => ("!", "the shell sent a malformed reply", None),
    };
    show_lines(lead, body);
    REPLIED_NS.store(hpet::nanos(), Ordering::Relaxed);
    REPLIES_SHOWN.fetch_add(1, Ordering::Relaxed);
    // A script runs only on a yes to all of it, so a confirmation asks nothing if
    // it lost characters or lines on the way, or cannot be read on one screen.
    if let Some(id) = confirm
        && (clean != text || !fits_on_screen(body))
    {
        log!("! the confirmation could not be shown in full, so it will not run");
        // Decline it, so no later line can answer it.
        return shell_line(&format!("answer {id} no"));
    }
    confirm.map(|id| {
        Waiting::Confirm(Confirm {
            id,
            tune: body.starts_with("Tune \""),
        })
    })
}

/// A `menu` reply: shown only if it decodes and every row is shown exactly as
/// sent and fits the screen; otherwise the call is denied, as a confirmation
/// that cannot be shown in full is declined.
fn menu_reply(reply: &[u8]) -> Option<Waiting> {
    REPLIED_NS.store(hpet::nanos(), Ordering::Relaxed);
    REPLIES_SHOWN.fetch_add(1, Ordering::Relaxed);
    let text = core::str::from_utf8(reply).unwrap_or("");
    match Menu::decode(text) {
        Ok(menu) if fits_on_screen(&menu.rows().join("\n")) => Some(Waiting::Menu(menu)),
        Ok(menu) => {
            log!("! the question could not be shown in full, so it was denied");
            shell_line(&menu::answer_line(menu.id, Answer::Deny))
        }
        Err(why) => {
            log!("! the shell sent a menu that cannot be shown ({why:?})");
            // An id that still reads gets its Deny, so no later key can answer it.
            let id = text
                .strip_prefix("menu ")
                .and_then(|rest| rest.split(' ').next())
                .and_then(|id| id.parse::<u64>().ok());
            id.and_then(|id| shell_line(&menu::answer_line(id, Answer::Deny)))
        }
    }
}

/// Whether `body`, printed with a two-character lead and followed by the prompt,
/// fits the screen with every line on one row, so no line is split or wrapped
/// (a wrapped piece could pass for a script line) and none scrolls away.
fn fits_on_screen(body: &str) -> bool {
    let (cols, rows) = crate::console::size().unwrap_or((usize::MAX, usize::MAX));
    let width = PIECE_CHARS.min(cols.saturating_sub(2));
    body.lines().all(|line| line.chars().count() <= width) && body.lines().count() + 1 < rows
}

/// Prints each line of `body` after `lead`, wrapped at spaces to the screen's
/// width so no word is split across rows.
fn show_lines(lead: &str, body: &str) {
    let (cols, _) = crate::console::size().unwrap_or((usize::MAX, usize::MAX));
    let width = PIECE_CHARS.min(cols.saturating_sub(2));
    for line in body.lines() {
        for row in wrap(line, width) {
            log!("{lead} {row}");
        }
    }
}

/// The shell's action log (`log`) after the log book: what each request did, so
/// `:log` is the one place to look.
fn show_actions() {
    if ipc::server_of("shell").is_none() {
        return;
    }
    match ipc::call("shell", b"log", SHELL_TIMEOUT_MS) {
        Ok(reply) => {
            let text = String::from_utf8_lossy(&reply);
            log!("actions:");
            show_lines(" ", text.strip_prefix("ok ").unwrap_or(&text));
        }
        Err(error) => log!("actions: the shell did not answer ({error:?})"),
    }
}

/// Runs a `:` command; a question it leaves for the next line.
fn kernel_command(line: &str) -> Option<Waiting> {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match command {
        "" | "help" => {
            let all = HELP.iter().chain(crate::llm_commands::HELP);
            let all = all.chain(crate::live_commands::HELP);
            for (usage, what) in all.chain(edit_mode::HELP) {
                log!("  {usage:<26} {what}");
            }
            log!("  any other line goes to the Rhai shell");
        }
        "ask" if rest.is_empty() => log!("usage: :ask <text>"),
        "ask" => ask(rest),
        "policy" => policy(rest),
        "threads" => {
            let (policy, ejections, switches) = threads::stats();
            log!(
                "policy {policy}; {switches} context switches; {ejections} ejections; {} migrations",
                threads::migrations()
            );
        }
        "cpus" => log!("{} CPU(s) online", smp::online()),
        "mem" => {
            let (free, total) = memory::with(|m| (m.free_frames(), m.total_frames()));
            let (allocations, bytes) = memory::heap::live();
            log!(
                "{} of {} MiB free; heap: {allocations} allocations, {} KiB",
                free * 4 / 1024,
                total * 4 / 1024,
                bytes / 1024
            );
        }
        "net" => net_status(),
        "log" => {
            crate::logbook::show(rest.parse().unwrap_or(LOG_LINES));
            show_actions();
        }
        "resolve" if rest.is_empty() => log!("usage: :resolve <host>"),
        "resolve" => crate::net_commands::resolve(rest),
        "ping" if rest.is_empty() => log!("usage: :ping <host|ip> [count]"),
        "ping" => crate::net_commands::ping(rest),
        "ps" => processes(),
        "uptime" => log!("{} s", hpet::nanos() / 1_000_000_000),
        "mouse" => crate::pointer::show(),
        "clear" => serial::_print(format_args!("{FORM_FEED}")),
        other if crate::llm_commands::run(other, rest) => {}
        other if crate::live_commands::run(other, rest) => {}
        other => match edit_mode::command(other, rest) {
            Some(proposal) => return proposal.map(Waiting::Apply),
            None => log!("unknown kernel command :{other} (try :help)"),
        },
    }
    None
}

fn ask(prompt: &str) {
    if !llm::available() {
        log!("{}", llmproto::chat::NO_MODEL);
        if let Some(why) = crate::model::rejected() {
            log!("(the kai-model disk was attached but not kept: {why})");
        }
        return;
    }
    log!("(asking the LLM...)");
    console::status::set_busy(true);
    let reply = llm::ask(prompt);
    console::status::set_busy(false);
    match reply {
        Ok(reply) => {
            // Rows of `ai: ` plus at most `PIECE_CHARS - 4`, so model text never
            // wraps onto a row of its own, which it could make look like a prompt.
            for line in reply.lines() {
                let line = printable(line, kabi::MAX_MESSAGE, 1);
                for row in wrap(&line, PIECE_CHARS - AI_LEAD) {
                    log!("ai: {row}");
                }
            }
        }
        Err(error) => log!("error: {error}"),
    }
}

fn policy(name: &str) {
    use sched::spec::PolicySpec;
    let (kind, preset) = name.split_once(' ').unwrap_or((name, "gentle"));
    let kind = if kind == "rr" { "round-robin" } else { kind };
    match PolicySpec::parse(kind, preset) {
        _ if name.is_empty() => {}
        Ok(spec) => {
            if let Err(error) = crate::sched_control::set_policy(spec) {
                log!("cannot switch the policy: {error:?}");
            }
        }
        Err(_) => {
            log!("unknown policy {name}: use interactive, rr, responsive or classes gentle|firm");
            return;
        }
    }
    log!("scheduling policy: {}", threads::stats().0);
}

fn net_status() {
    // A model needs no network; with no server, only the llm service can say what
    // it mapped.
    if let Some(model) = crate::model::get() {
        log!("model: {} bytes held from the kai-model disk", model.len());
        if llm::target().is_none() {
            match llm::health() {
                Ok(status) => log!("health: {status}"),
                Err(error) => log!("health: {error}"),
            }
        }
    }
    if !net::is_up() {
        log!("network: no card");
        return;
    }
    crate::net_commands::status();
    // An LLM server is a test double (`llm=`); without one there is nothing to say.
    if let Some((ip, port, model)) = llm::target() {
        log!("LLM server: {ip}:{port}, model {model}");
        match ipc::server_of("llm") {
            Some(pid) => log!("llm service: pid {pid}"),
            None => log!("llm service: not running"),
        }
        match llm::health() {
            Ok(status) => log!("health: {status}"),
            Err(error) => log!("health: {error}"),
        }
    }
}

fn processes() {
    let mut any = false;
    crate::process::services(|pid, name| {
        log!("  {pid:>5}  {}", name.as_str());
        any = true;
    });
    match crate::process::supervision::supervisor_pid() {
        0 => log!("  supervisor not running"),
        pid => log!("  {pid:>5}  supervisor"),
    }
    if !any {
        log!("  no services running");
    }
}

/// M6 exit test, now through the shell, with M9d's console cells and keys. The
/// harness types "xyz" and Ctrl-C, which must clear the line, then "pol"; once it
/// shows at the prompt the kernel logs a line, which must land above the typed line
/// and leave it whole. The harness then makes "policy rr" of it with arrows, Home,
/// End, Backspace and Delete (only that exact line switches the policy), and types
/// "ask hello" and a read-only request in plain English, whose script runs at once.
/// Then it types "hello", which the AI answers in words, and a request whose script
/// switches the policy to interactive, which runs at once too; then "undo", which
/// puts round-robin back. The policy must be round-robin on every CPU, the llm
/// service must have answered the shell four times (the question and three script
/// requests), and the terminal must have shown all six replies. The counts come
/// from the kernel, not from anything a program prints; xtask checks the printed
/// text.
pub fn self_test() -> Result<(), &'static str> {
    const TIMEOUT_MS: u64 = 60_000;
    const NOT_CLEARED: &str = "terminal: Ctrl-C did not clear the typed line";
    let cleared = || LINES_CLEARED.load(Ordering::Relaxed) >= 1;
    threads::spawn("terminal", run, 0)?;
    for _ in 0..TIMEOUT_MS / 10 {
        if ipc::server_of("llm").is_some() && ipc::server_of("shell").is_some() {
            break;
        }
        threads::sleep_ms(10);
    }
    threads::sleep_ms(200);
    log!("terminal: ready for keys");
    match mid_typing_check() {
        Ok(()) => {}
        // Ctrl-C comes before "pol", so a Ctrl-C that failed shows up here first.
        Err(NEVER_TYPED) if !cleared() => {
            return Err("terminal: Ctrl-C cleared no line, and \"kai> pol\" never showed");
        }
        Err(error) => return Err(error),
    }
    log!("terminal: ready for keys");
    // The read-only script ran with no question: its reply is the third shown.
    for _ in 0..TIMEOUT_MS / 10 {
        if REPLIES_SHOWN.load(Ordering::Relaxed) >= 3 && !AWAITING_ANSWER.load(Ordering::Acquire) {
            log!("terminal: ready for keys");
            break;
        }
        threads::sleep_ms(10);
    }
    // `undo` is typed once the switch it undoes was shown, with no question asked.
    for _ in 0..TIMEOUT_MS / 10 {
        if REPLIES_SHOWN.load(Ordering::Relaxed) >= 5 && !AWAITING_ANSWER.load(Ordering::Acquire) {
            log!("terminal: ready for keys");
            break;
        }
        threads::sleep_ms(10);
    }
    // The installed policy on every CPU, not the active one: a watchdog fallback
    // also runs round-robin and must not count as the command having worked.
    let switched = || threads::installed_policy_everywhere("round-robin");
    let llm_answered_shell = || ipc::replies("llm").1 >= 4;
    let all_shown = || REPLIES_SHOWN.load(Ordering::Relaxed) >= 6;
    for _ in 0..TIMEOUT_MS / 10 {
        if cleared() && switched() && llm_answered_shell() && all_shown() {
            log!(
                "terminal: line cleared, policy switched, LLM replies shown, scripts ran at once and undo put the policy back"
            );
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    if !cleared() {
        return Err(NOT_CLEARED);
    }
    if !switched() {
        return Err("terminal: the policy command did not switch to round-robin");
    }
    if !llm_answered_shell() {
        return Err("terminal: the llm service did not answer the shell four times");
    }
    Err("terminal: not all six replies were shown")
}

/// What the harness types first, at the prompt.
const TYPED: &str = "kai> pol";
/// The line logged while it shows.
const LOGGED: &str = "terminal: a log line while typing";
const NEVER_TYPED: &str = "terminal: the typed keys never reached the prompt";

/// Waits for `TYPED` at the prompt, logs a line, and checks that the line went above
/// the typed one, which stayed whole with the cursor after it. CI boots get a 1280x800
/// framebuffer even with `-display none`, so there is always a console to check.
fn mid_typing_check() -> Result<(), &'static str> {
    const TIMEOUT_MS: u64 = 30_000;
    /// The cursor's row, if it reads exactly `TYPED` with the cursor after it.
    fn typed_row(grid: &Screen) -> Option<usize> {
        let (row, col) = grid.cursor()?;
        (col == TYPED.len() && grid.row_reads(row, TYPED)).then_some(row)
    }
    let mut typed = false;
    for _ in 0..TIMEOUT_MS / 10 {
        match console::with_grid(|grid| typed_row(grid).is_some()) {
            None => return Err("terminal: no console to check"),
            Some(true) => {
                typed = true;
                break;
            }
            Some(false) => threads::sleep_ms(10),
        }
    }
    if !typed {
        return Err(NEVER_TYPED);
    }
    log!("{LOGGED}");
    let whole = console::with_grid(|grid| {
        typed_row(grid).is_some_and(|row| (0..row).any(|above| grid.row_reads(above, LOGGED)))
    });
    if whole != Some(true) {
        return Err("terminal: the log line split the typed line");
    }
    log!("terminal: a log line arrived mid-typing; the typed line stayed whole below it");
    Ok(())
}
