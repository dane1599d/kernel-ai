//! The M6 terminal: a prompt with line editing and a fixed set of built-in commands.
//! A real REPL language arrives in M8; until then, commands are plain words.

use crate::console::{self, BACKSPACE};
use crate::keyboard::Decoder;
use crate::{hpet, llm, log, memory, net, serial, smp, threads};
use alloc::boxed::Box;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use sched::{Interactive, RoundRobin};

const PROMPT: &str = "kai> ";
const MAX_LINE: usize = 200;
const DELETE: char = 127u8 as char;

/// LLM replies shown so far; the terminal self-test watches it.
static REPLIES: AtomicU64 = AtomicU64::new(0);

const HELP: &[(&str, &str)] = &[
    ("help", "list commands"),
    ("ask <text>", "send <text> to the LLM and show the reply"),
    (
        "policy [interactive|rr]",
        "show or switch the scheduling policy on every CPU",
    ),
    ("threads", "scheduler statistics"),
    ("cpus", "CPUs online"),
    ("mem", "free memory and heap use"),
    ("net", "network and LLM server status"),
    ("uptime", "time since the clock started"),
    ("clear", "clear the screen"),
];

fn print(args: core::fmt::Arguments) {
    serial::_print(args);
}

/// Runs the terminal on the calling thread, forever.
pub fn run(_: usize) {
    let mut decoder = Decoder::new();
    log!("Kernel AI terminal. Type help for commands.");
    loop {
        print(format_args!("{PROMPT}"));
        let line = read_line(&mut decoder);
        execute(line.trim());
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
                    print(format_args!("{BACKSPACE}"));
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

fn execute(line: &str) {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match command {
        "" => {}
        "help" => {
            for (usage, what) in HELP {
                log!("  {usage:<26} {what}");
            }
        }
        "ask" if rest.is_empty() => log!("usage: ask <text>"),
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
        "uptime" => log!("{} s", hpet::nanos() / 1_000_000_000),
        "clear" => console::clear(),
        other => log!("unknown command: {other} (try help)"),
    }
}

fn ask(prompt: &str) {
    log!("(asking the LLM...)");
    match llm::ask(prompt) {
        Ok(reply) => {
            log!("ai: {reply}");
            REPLIES.fetch_add(1, Ordering::SeqCst);
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
            match llm::health() {
                Ok((status, rtt)) => log!("health: HTTP {status} in {rtt} ms"),
                Err(error) => log!("health: {error}"),
            }
        }
    }
}

/// M6 exit test: the harness types "policy rr" and "ask hello"; the policy must
/// change and an LLM reply must be shown.
pub fn self_test() -> Result<(), &'static str> {
    const TIMEOUT_MS: u64 = 60_000;
    threads::spawn("terminal", run, 0)?;
    threads::sleep_ms(200);
    log!("terminal: ready for keys");
    let switched = || threads::stats().0 == "round-robin";
    let replied = || REPLIES.load(Ordering::SeqCst) >= 1;
    for _ in 0..TIMEOUT_MS / 10 {
        if switched() && replied() {
            log!("terminal: policy switched and LLM reply shown");
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    if !switched() {
        return Err("terminal: the policy command did not switch to round-robin");
    }
    Err("terminal: no LLM reply was shown")
}
