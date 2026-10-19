//! The terminal: a prompt with line editing. Each line goes to the ring-3 Rhai
//! shell, whose answer is printed with a `= ` (result), `! ` (error) or `? ` (a
//! script waiting for a yes) lead that no kernel message uses, so shell output can
//! never pass for kernel output. Lines starting with `:` are kernel commands, which
//! work even when the shell is down. After a `? ` reply the prompt changes and the
//! next line, whatever it is, answers the question; only `y` or `yes` runs the script.
//! In edit mode (`:edit`), plain lines go to the editor instead, and `:apply` asks
//! the same way before an upload runs.

use crate::console::{self, BACKSPACE, FORM_FEED, Screen};
use crate::keyboard::Decoder;
use crate::{edit_mode, hpet, ipc, llm, log, memory, net, serial, smp, threads};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use llmproto::text::printable;
use sched::{Interactive, RoundRobin};

const PROMPT: &str = "kai> ";
const MAX_LINE: usize = 200;
const DELETE: char = 127u8 as char;
/// Longest a shell line may take: the shell's longest LLM wait (140 s for `ask`, two
/// script rounds of 75 s) plus its compute budget, with room to spare, so the shell
/// always answers first.
const SHELL_TIMEOUT_MS: u64 = 160_000;
/// How long to wait for a shell that is starting or restarting.
const SHELL_START_WAIT_NS: u64 = 3_000_000_000;
/// Longest piece of a reply printed in one go (each print keeps interrupts off).
pub(crate) const PIECE_CHARS: usize = 160;
const MAX_REPLY_LINES: usize = 64;

/// Shell replies printed in full, for the self-test.
static REPLIES_SHOWN: AtomicU64 = AtomicU64::new(0);
/// Set while the terminal waits for the answer to a confirmation, for the self-test.
static AWAITING_ANSWER: AtomicBool = AtomicBool::new(false);

const HELP: &[(&str, &str)] = &[
    (":help", "this list"),
    (":ask <text>", "send <text> straight to the llm service"),
    (
        ":policy [interactive|rr]",
        "show or switch the scheduling policy on every CPU",
    ),
    (":threads", "scheduler statistics"),
    (":cpus", "CPUs online"),
    (":mem", "free memory and heap use"),
    (":net", "network, LLM server and llm service status"),
    (":ps", "running processes"),
    (":uptime", "time since the clock started"),
    (":clear", "clear the screen"),
];

/// A question the next line answers.
enum Waiting {
    /// A script the shell proposed.
    Script(u64),
    /// An upload `:apply` showed.
    Apply(edit_mode::Proposal),
}

/// Runs the terminal on the calling thread, forever.
pub fn run(_: usize) {
    let mut decoder = Decoder::new();
    log!("Kernel AI terminal. Lines are Rhai (try help); kernel commands start with ':'.");
    let mut waiting: Option<Waiting> = None;
    loop {
        let prompt = match &waiting {
            Some(Waiting::Script(id)) => format!("run script {id}? yes/no> "),
            Some(Waiting::Apply(p)) => format!("apply {} to {}? yes/no> ", p.label, p.component),
            None => edit_mode::prompt().unwrap_or_else(|| PROMPT.into()),
        };
        console::show_input(&prompt, "", 0);
        if waiting.is_some() {
            // Only keys typed once the question is on screen may answer it.
            decoder.discard_pending();
            AWAITING_ANSWER.store(true, Ordering::Release);
        }
        let line = read_line(&mut decoder, &prompt);
        AWAITING_ANSWER.store(false, Ordering::Release);
        let line = line.trim();
        match waiting.take() {
            Some(Waiting::Script(id)) => {
                waiting = shell_line(&format!("answer {id} {line}")).map(Waiting::Script);
                continue;
            }
            Some(Waiting::Apply(proposal)) => {
                edit_mode::show(&edit_mode::answer(&proposal, line));
                continue;
            }
            None => {}
        }
        match line.strip_prefix(':') {
            Some(command) => waiting = kernel_command(command.trim()),
            None if line.is_empty() => {}
            None if edit_mode::editing().is_some() => {
                if edit_mode::builds(line) {
                    log!("(building on the host...)");
                }
                edit_mode::show(&edit_mode::editor_line(line));
            }
            None if is_answer(line) => log!("! scripts are answered only at their own prompt"),
            None => waiting = shell_line(line).map(Waiting::Script),
        }
    }
}

/// Reads a line into the input region after `prompt`. Typed keys go only to the
/// screen; serial gets the whole line, with its prompt, on Enter.
fn read_line(decoder: &mut Decoder, prompt: &str) -> String {
    let mut line = String::new();
    loop {
        match decoder.read_char() {
            '\n' | '\r' => {
                finish_line(prompt, &line, "");
                return line;
            }
            BACKSPACE | DELETE => {
                if line.pop().is_none() {
                    continue;
                }
            }
            ch if !ch.is_control() && line.len() < MAX_LINE => line.push(ch),
            _ => continue,
        }
        console::show_input(prompt, &line, line.chars().count());
    }
}

/// Ends a typed line: the input region goes, and the line is logged in its place
/// with its prompt and `mark` after it.
fn finish_line(prompt: &str, text: &str, mark: &str) {
    console::hide_input();
    log!("{prompt}{text}{mark}");
}

/// Sends a line to the shell and prints its answer. Returns the id of a script that
/// waits for the user's yes.
fn shell_line(line: &str) -> Option<u64> {
    let start_deadline = hpet::nanos() + SHELL_START_WAIT_NS;
    if ipc::wait_bound("shell", start_deadline, || false).is_err() {
        log!("shell not running; kernel commands start with ':' (try :help)");
        return None;
    }
    match ipc::call("shell", line.as_bytes(), SHELL_TIMEOUT_MS) {
        Ok(reply) => show_shell_reply(&reply),
        Err(error) => {
            log!("shell: no answer ({error:?})");
            None
        }
    }
}

/// Prints `ok <text>`, `err <text>` or `confirm <id> <text>` with its lead, one
/// complete line per piece, and returns the id of a confirmation shown in full.
fn show_shell_reply(reply: &[u8]) -> Option<u64> {
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
    REPLIES_SHOWN.fetch_add(1, Ordering::Relaxed);
    // A script runs only on a yes to all of it, so a confirmation asks nothing if
    // it lost characters or lines on the way, or cannot be read on one screen.
    if let Some(id) = confirm
        && (clean != text || !fits_on_screen(body))
    {
        log!("! the confirmation could not be shown in full, so the script will not run");
        // Decline it, so no later line can answer it.
        return shell_line(&format!("answer {id} no"));
    }
    confirm
}

/// Whether a line typed at the `kai> ` prompt looks like an answer to a script,
/// `answer <id> ...`, which only the confirm prompt may send.
fn is_answer(line: &str) -> bool {
    line.strip_prefix("answer ")
        .and_then(|rest| rest.split(' ').next())
        .is_some_and(|id| id.parse::<u64>().is_ok())
}

/// Whether `body`, printed with a two-character lead and followed by the prompt,
/// fits the screen with every line on one row, so no line is split or wrapped
/// (a wrapped piece could pass for a script line) and none scrolls away.
fn fits_on_screen(body: &str) -> bool {
    let (cols, rows) = crate::console::size().unwrap_or((usize::MAX, usize::MAX));
    let width = PIECE_CHARS.min(cols.saturating_sub(2));
    body.lines().all(|line| line.chars().count() <= width) && body.lines().count() + 1 < rows
}

fn show_lines(lead: &str, body: &str) {
    for line in body.lines() {
        let mut rest = line;
        loop {
            let cut = rest
                .char_indices()
                .nth(PIECE_CHARS)
                .map_or(rest.len(), |(at, _)| at);
            log!("{lead} {}", &rest[..cut]);
            rest = &rest[cut..];
            if rest.is_empty() {
                break;
            }
        }
    }
}

/// Runs a `:` command; a question it leaves for the next line.
fn kernel_command(line: &str) -> Option<Waiting> {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match command {
        "" | "help" => {
            let all = HELP.iter().chain(crate::live_commands::HELP);
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
        "ps" => processes(),
        "uptime" => log!("{} s", hpet::nanos() / 1_000_000_000),
        "clear" => serial::_print(format_args!("{FORM_FEED}")),
        other if crate::live_commands::run(other, rest) => {}
        other => match edit_mode::command(other, rest) {
            Some(proposal) => return proposal.map(Waiting::Apply),
            None => log!("unknown kernel command :{other} (try :help)"),
        },
    }
    None
}

fn ask(prompt: &str) {
    log!("(asking the LLM...)");
    match llm::ask(prompt) {
        Ok(reply) => {
            for line in reply.lines() {
                log!("ai: {}", printable(line, PIECE_CHARS, 1));
            }
        }
        Err(error) => log!("error: {error}"),
    }
}

fn policy(name: &str) {
    match name {
        "" => {}
        "interactive" => threads::set_policy_all(|| Box::new(Interactive::new())),
        "rr" | "round-robin" => threads::set_policy_all(|| Box::new(RoundRobin::new())),
        other => {
            log!("unknown policy {other}: use interactive or rr");
            return;
        }
    }
    log!("scheduling policy: {}", threads::stats().0);
}

fn net_status() {
    if !net::is_up() {
        log!("network: no card");
        return;
    }
    log!("network: 10.0.2.15/24 via 10.0.2.2");
    match llm::target() {
        None => log!("LLM server: not configured (boot with llm=IP:PORT)"),
        Some((ip, port, model)) => {
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

/// M6 exit test, now through the shell, with M9d's console cells. The harness types
/// "pol"; once it shows at the prompt the kernel logs a line, which must land above
/// the typed line and leave it whole. The harness then finishes "policy rr" and types
/// "ask hello", a request in plain English and "yes" to the script proposed for it.
/// The policy must be installed on every CPU, the llm service must have answered the
/// shell twice (the question and the script request), and the terminal must have
/// shown all four replies. The counts come from the kernel, not from anything a
/// program prints; xtask checks the printed text.
pub fn self_test() -> Result<(), &'static str> {
    const TIMEOUT_MS: u64 = 60_000;
    threads::spawn("terminal", run, 0)?;
    for _ in 0..TIMEOUT_MS / 10 {
        if ipc::server_of("llm").is_some() && ipc::server_of("shell").is_some() {
            break;
        }
        threads::sleep_ms(10);
    }
    threads::sleep_ms(200);
    log!("terminal: ready for keys");
    mid_typing_check()?;
    log!("terminal: ready for keys");
    // The answer is typed only once the question is on screen: keys typed earlier
    // are dropped, so this shows the answer came from the prompt.
    for _ in 0..TIMEOUT_MS / 10 {
        if AWAITING_ANSWER.load(Ordering::Acquire) {
            log!("terminal: ready for keys");
            break;
        }
        threads::sleep_ms(10);
    }
    // The installed policy on every CPU, not the active one: a watchdog fallback
    // also runs round-robin and must not count as the command having worked.
    let switched = || threads::installed_policy_everywhere("round-robin");
    let llm_answered_shell = || ipc::replies("llm").1 >= 2;
    let all_shown = || REPLIES_SHOWN.load(Ordering::Relaxed) >= 4;
    for _ in 0..TIMEOUT_MS / 10 {
        if switched() && llm_answered_shell() && all_shown() {
            log!("terminal: policy switched, LLM reply and a confirmed script shown");
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    if !switched() {
        return Err("terminal: the policy command did not switch to round-robin");
    }
    if !llm_answered_shell() {
        return Err("terminal: the llm service did not answer the shell twice");
    }
    Err("terminal: not all four replies were shown")
}

/// What the harness types first, at the prompt.
const TYPED: &str = "kai> pol";
/// The line logged while it shows.
const LOGGED: &str = "terminal: a log line while typing";

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
        return Err("terminal: the typed keys never reached the prompt");
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
