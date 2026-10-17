//! The terminal: a prompt with line editing. Each line goes to the ring-3 Rhai
//! shell, whose answer is printed with a `= ` (result) or `! ` (error) lead that no
//! kernel message uses, so shell output can never pass for kernel output. Lines
//! starting with `:` are kernel commands, which work even when the shell is down.

use crate::console::{BACKSPACE, FORM_FEED};
use crate::keyboard::Decoder;
use crate::{hpet, ipc, llm, log, memory, net, serial, smp, threads};
use alloc::boxed::Box;
use alloc::string::String;
use llmproto::text::printable;
use sched::{Interactive, RoundRobin};

const PROMPT: &str = "kai> ";
const MAX_LINE: usize = 200;
const DELETE: char = 127u8 as char;
/// Longest a shell line may take: the shell's own LLM wait (140 s) plus its
/// two-second compute budget, with room to spare, so the shell always answers first.
const SHELL_TIMEOUT_MS: u64 = 160_000;
/// How long to wait for a shell that is starting or restarting.
const SHELL_START_WAIT_NS: u64 = 3_000_000_000;
/// Longest piece of a reply printed in one go (each print keeps interrupts off).
const PIECE_CHARS: usize = 160;
const MAX_REPLY_LINES: usize = 64;

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

fn print(args: core::fmt::Arguments) {
    serial::_print(args);
}

/// Runs the terminal on the calling thread, forever.
pub fn run(_: usize) {
    let mut decoder = Decoder::new();
    log!("Kernel AI terminal. Lines are Rhai (try help); kernel commands start with ':'.");
    loop {
        print(format_args!("{PROMPT}"));
        let line = read_line(&mut decoder);
        let line = line.trim();
        match line.strip_prefix(':') {
            Some(command) => kernel_command(command.trim()),
            None if line.is_empty() => {}
            None => shell_line(line),
        }
    }
}

fn read_line(decoder: &mut Decoder) -> String {
    let mut line = String::new();
    loop {
        match decoder.read_char() {
            '\n' | '\r' => {
                print(format_args!("\n"));
                return line;
            }
            BACKSPACE | DELETE => {
                if line.pop().is_some() {
                    print(format_args!("{BACKSPACE} {BACKSPACE}"));
                }
            }
            ch if !ch.is_control() && line.len() < MAX_LINE => {
                line.push(ch);
                print(format_args!("{ch}"));
            }
            _ => {}
        }
    }
}

/// Sends a line to the shell and prints its answer.
fn shell_line(line: &str) {
    let start_deadline = hpet::nanos() + SHELL_START_WAIT_NS;
    if ipc::wait_bound("shell", start_deadline, || false).is_err() {
        log!("shell not running; kernel commands start with ':' (try :help)");
        return;
    }
    match ipc::call("shell", line.as_bytes(), SHELL_TIMEOUT_MS) {
        Ok(reply) => show_shell_reply(&reply),
        Err(error) => log!("shell: no answer ({error:?})"),
    }
}

/// Prints `ok <text>` or `err <text>` with its lead, one complete line per piece.
fn show_shell_reply(reply: &[u8]) {
    let text = String::from_utf8_lossy(reply);
    let clean = printable(&text, kabi::MAX_MESSAGE, MAX_REPLY_LINES);
    let (lead, body) = match clean.split_once(' ').unwrap_or((clean.as_str(), "")) {
        ("ok", body) => ("=", body),
        ("err", body) => ("!", body),
        _ => ("!", "the shell sent a malformed reply"),
    };
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

fn kernel_command(line: &str) {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match command {
        "" | "help" => {
            for (usage, what) in HELP {
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
        "clear" => print(format_args!("{FORM_FEED}")),
        other => log!("unknown kernel command :{other} (try :help)"),
    }
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

/// M6 exit test, now through the shell: the harness types "policy rr" and "ask
/// hello". The policy must be installed on every CPU, the llm service must have
/// answered the shell, and the shell must have answered both lines. The counts come
/// from the kernel's IPC layer, not from anything a program prints.
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
    // The installed policy on every CPU, not the active one: a watchdog fallback
    // also runs round-robin and must not count as the command having worked.
    let switched = || threads::installed_policy_everywhere("round-robin");
    let llm_answered_shell = || ipc::replies("llm").1 >= 1;
    let shell_answered = || ipc::replies("shell").0 >= 2;
    for _ in 0..TIMEOUT_MS / 10 {
        if switched() && llm_answered_shell() && shell_answered() {
            log!("terminal: policy switched and LLM reply shown through the shell");
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    if !switched() {
        return Err("terminal: the policy command did not switch to round-robin");
    }
    if !llm_answered_shell() {
        return Err("terminal: the llm service never answered the shell");
    }
    Err("terminal: the shell did not answer both lines")
}
