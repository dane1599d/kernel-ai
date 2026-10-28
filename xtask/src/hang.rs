//! Where a hung guest is: on a case's timeout, before QEMU is killed, the
//! monitor's `info registers -a` gives each vCPU's RIP, RFLAGS, privilege level
//! and halt state, and for each CPU in the kernel a read of its per-CPU block
//! (`kernel/src/percpu.rs`, at the GS base) gives the thread it runs and its
//! preemption count, and a walk of its frame pointers a short backtrace (or,
//! with no frame chain, the kernel functions named by words on its stack). Every
//! address is named from the kernel's line table, as a fault report would be.
//! A silent hang (no line for minutes, not even a case's own timeout) then shows
//! which code every CPU sits in.

use linetable::LineTable;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

/// Frames of each kernel backtrace.
const FRAMES: usize = 12;
/// Stack words read from RSP when a CPU's frame pointers give nothing.
const SCAN_WORDS: usize = 64;
/// How long one monitor command may take to answer.
const ANSWER: Duration = Duration::from_secs(3);
/// Offsets in `PerCpu` (`kernel/src/percpu.rs` asserts them).
const PREEMPT_COUNT: u64 = 36;
const CURRENT: u64 = 56;
/// RFLAGS.IF.
const IF: u64 = 1 << 9;
/// Where the kernel's code is linked, and where its stacks and data live: a
/// frame whose saved RBP or return address lies elsewhere ends the walk.
const KERNEL_TEXT: u64 = 0xffff_ffff_8000_0000;
const KERNEL_HALF: u64 = 0xffff_8000_0000_0000;

/// One vCPU's state, from `info registers -a`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cpu {
    pub index: usize,
    pub rip: u64,
    pub rflags: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub cpl: u8,
    pub halted: bool,
    pub gs_base: u64,
}

/// Prints where each vCPU is, naming addresses from the `kernel.lines` in the
/// boot drive's `bundle`. Never fails the run: a dump that cannot be had says so
/// and the timeout is reported as before.
pub fn dump(monitor: u16, bundle: &Path) {
    let bundle_bytes = std::fs::read(bundle).unwrap_or_default();
    let table_bytes = kaibundle::parse(&bundle_bytes)
        .ok()
        .and_then(|b| b.get("kernel.lines"));
    let table = table_bytes.and_then(|bytes| LineTable::parse(bytes).ok());
    if table.is_none() {
        println!("xtask: hang dump: no kernel.lines in {}", bundle.display());
    }
    let name = |addr: u64| -> String {
        let Some(table) = table.as_ref() else {
            return format!("{addr:#x}");
        };
        match table.resolve(addr) {
            Some(at) => format!(
                "{addr:#x} {} ({}:{})",
                at.symbol.unwrap_or("?"),
                at.file.unwrap_or("?"),
                at.line.unwrap_or(0)
            ),
            None => format!("{addr:#x}"),
        }
    };
    let mut monitor = match Monitor::connect(monitor) {
        Ok(monitor) => monitor,
        Err(e) => {
            println!("xtask: hang dump: cannot reach the QEMU monitor: {e}");
            return;
        }
    };
    let cpus = parse_cpus(&monitor.run("info registers -a"));
    if cpus.is_empty() {
        println!("xtask: hang dump: the monitor gave no registers");
        return;
    }
    println!("xtask: hang dump: where each vCPU was at the timeout");
    for cpu in &cpus {
        let interrupts = if cpu.rflags & IF != 0 { "on" } else { "off" };
        let state = if cpu.halted { "halted" } else { "running" };
        println!(
            "xtask: hang dump: cpu {} {state}, ring {}, interrupts {interrupts}, rip {}",
            cpu.index,
            cpu.cpl,
            name(cpu.rip)
        );
        if cpu.cpl != 0 {
            continue;
        }
        monitor.run(&format!("cpu {}", cpu.index));
        let block = words(&monitor.run(&format!("x /8gx {:#x}", cpu.gs_base)));
        if block.len() == 8 {
            let preempt = (block[(PREEMPT_COUNT / 8) as usize] >> 32) & 0xFFFF_FFFF;
            let current = block[(CURRENT / 8) as usize];
            println!(
                "xtask: hang dump: cpu {} runs thread {current}, preempt_count {preempt}",
                cpu.index
            );
        }
        // A word is code if the line table names a function at it.
        let code = |addr: u64| {
            addr >= KERNEL_TEXT
                && addr != u64::MAX
                && table
                    .as_ref()
                    .and_then(|t| t.resolve(addr))
                    .is_some_and(|at| at.symbol.is_some())
        };
        let mut rbp = cpu.rbp;
        let mut walked = 0;
        for _ in 0..FRAMES {
            if rbp < KERNEL_HALF || rbp % 8 != 0 {
                break;
            }
            let frame = words(&monitor.run(&format!("x /2gx {rbp:#x}")));
            let [next, ret] = frame[..] else {
                break;
            };
            // Not a frame-pointer frame (entry code, a function without one, or
            // a stale RBP): its "return address" names no function.
            if !code(ret) {
                break;
            }
            println!("xtask: hang dump:   from {}", name(ret));
            walked += 1;
            if next <= rbp {
                break;
            }
            rbp = next;
        }
        // No frame chain (a leaf, or entry code): words on the stack that name
        // kernel functions, most likely return addresses. A guess, marked so.
        if walked == 0 && cpu.rsp >= KERNEL_HALF {
            let stack = words(&monitor.run(&format!("x /{SCAN_WORDS}gx {:#x}", cpu.rsp)));
            for addr in stack.iter().filter(|&&a| code(a)).take(FRAMES) {
                println!("xtask: hang dump:   on its stack {}", name(*addr));
            }
        }
    }
}

/// The QEMU human monitor over TCP, one command at a time.
struct Monitor {
    stream: TcpStream,
}

impl Monitor {
    fn connect(port: u16) -> std::io::Result<Self> {
        let stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_millis(200)))?;
        let mut monitor = Self { stream };
        // The banner and the first prompt.
        monitor.read_answer();
        Ok(monitor)
    }

    /// Sends `command` and returns what the monitor printed before its next
    /// prompt (or within [`ANSWER`]).
    fn run(&mut self, command: &str) -> String {
        if writeln!(self.stream, "{command}").is_err() {
            return String::new();
        }
        self.read_answer()
    }

    fn read_answer(&mut self) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        let start = Instant::now();
        while start.elapsed() < ANSWER {
            match self.stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(_) => {}
            }
            if out.ends_with(b"(qemu) ") {
                break;
            }
        }
        clean(&String::from_utf8_lossy(&out))
    }
}

/// The monitor's text without its terminal escapes and carriage returns.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // CSI: up to and including its final letter.
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Each `CPU#n` block of `info registers -a`.
pub fn parse_cpus(text: &str) -> Vec<Cpu> {
    let mut cpus: Vec<Cpu> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(n) = line.strip_prefix("CPU#") {
            if let Ok(index) = n.trim().parse() {
                cpus.push(Cpu {
                    index,
                    ..Cpu::default()
                });
            }
            continue;
        }
        let Some(cpu) = cpus.last_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("GS =") {
            // `GS =0000 <base> <limit> <flags> ...`
            cpu.gs_base = rest.split_whitespace().nth(1).and_then(hex).unwrap_or(0);
            continue;
        }
        for (key, value) in fields(line) {
            match key {
                "RIP" => cpu.rip = hex(value).unwrap_or(0),
                "RFL" => cpu.rflags = hex(value).unwrap_or(0),
                "RBP" => cpu.rbp = hex(value).unwrap_or(0),
                "RSP" => cpu.rsp = hex(value).unwrap_or(0),
                "CPL" => cpu.cpl = value.parse().unwrap_or(0),
                "HLT" => cpu.halted = value == "1",
                _ => {}
            }
        }
    }
    cpus
}

/// `KEY=value` pairs on a register line (`R8 =` has a space before its `=`).
fn fields(line: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().rsplit(' ').next().unwrap_or("").trim();
        let after = rest[eq + 1..].trim_start();
        let end = after.find(' ').unwrap_or(after.len());
        out.push((key, &after[..end]));
        rest = &after[end..];
    }
    out
}

fn hex(text: &str) -> Option<u64> {
    u64::from_str_radix(text.trim_start_matches("0x"), 16).ok()
}

/// The values of an `x /Ngx` answer: `addr: 0x... 0x...` lines.
pub fn words(text: &str) -> Vec<u64> {
    text.lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(addr, _)| hex(addr.trim()).is_some())
        .flat_map(|(_, values)| {
            values
                .split_whitespace()
                .filter_map(hex)
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTERS: &str = "info registers -a\n\
CPU#0\n\
RAX=0000000000000000 RBX=0000000000000001 RCX=0000000000000002 RDX=0000000000000003\n\
RSI=0000000000000004 RDI=0000000000000005 RBP=ffff800000123f40 RSP=ffff800000123f00\n\
R8 =0000000000000000 R9 =0000000000000000 R10=0000000000000000 R11=0000000000000000\n\
RIP=ffffffff80012345 RFL=00000046 [---Z-P-] CPL=0 II=0 A20=1 SMM=0 HLT=0\n\
ES =0000 0000000000000000 ffffffff 00c00000\n\
GS =0000 ffffffff80400000 ffffffff 00c00000\n\
CPU#1\n\
RBP=0000000000000000 RSP=00007ffffffde000\n\
RIP=0000000000401000 RFL=00000202 [-------] CPL=3 II=0 A20=1 SMM=0 HLT=1\n\
GS =0000 0000000000000000 ffffffff 00c00000\n\
(qemu) ";

    #[test]
    fn registers_parse_per_cpu() {
        let cpus = parse_cpus(REGISTERS);
        assert_eq!(cpus.len(), 2);
        assert_eq!(cpus[0].rip, 0xffff_ffff_8001_2345);
        assert_eq!(cpus[0].rflags & IF, 0);
        assert_eq!(cpus[0].rbp, 0xffff_8000_0012_3f40);
        assert_eq!(cpus[0].rsp, 0xffff_8000_0012_3f00);
        assert_eq!(cpus[0].cpl, 0);
        assert!(!cpus[0].halted);
        assert_eq!(cpus[0].gs_base, 0xffff_ffff_8040_0000);
        assert_eq!(cpus[1].cpl, 3);
        assert!(cpus[1].halted);
        assert_ne!(cpus[1].rflags & IF, 0);
    }

    #[test]
    fn memory_words_parse_and_escapes_go() {
        let answer = "x /2gx 0xffff800000123f40\nffff800000123f40: 0xffff800000123f80 0xffffffff80011111\n(qemu) ";
        assert_eq!(
            words(answer),
            [0xffff_8000_0012_3f80, 0xffff_ffff_8001_1111]
        );
        assert_eq!(clean("\u{1b}[K(qemu) \r\nCPU#0"), "(qemu) \nCPU#0");
    }
}
