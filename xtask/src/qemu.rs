//! Building QEMU command lines and running QEMU, interactively or as a test.

use crate::cases::TestCase;
use crate::{Accel, Firmware};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// QEMU exit status for `isa-debug-exit` value 0x10: (0x10 << 1) | 1.
const QEMU_PASS: i32 = 33;
/// CPUs in the VM. Emulation (TCG) gets no more CPUs than the host has cores: on an
/// overcommitted host, virtual CPUs wait for host scheduling, which looks like
/// guest latency the kernel cannot control. KVM tolerates overcommit far better.
const MAX_CPUS: usize = 4;

pub fn vm_cpus(accel: Accel) -> usize {
    let host = thread::available_parallelism().map_or(1, usize::from);
    match accel {
        Accel::Tcg | Accel::Auto => MAX_CPUS.min(host.max(1)),
        Accel::Kvm | Accel::Whpx => MAX_CPUS,
    }
}

/// QEMU runs with `target/` as its working directory, so drive paths stay relative
/// and free of drive-letter colons, which QEMU's option parser would misread.
/// Every VM gets a virtio network card on QEMU's user-mode network: the guest is
/// 10.0.2.15, and 10.0.2.2 reaches the host's 127.0.0.1.
pub fn args(firmware: &Firmware, accel: Accel, headless: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-machine",
        "q35",
        "-m",
        "512M",
        "-no-reboot",
        "-serial",
        "stdio",
        "-device",
        "isa-debug-exit,iobase=0xf4,iosize=0x04",
        "-netdev",
        "user,id=net0",
        "-device",
        "virtio-net-pci,netdev=net0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push("-smp".into());
    args.push(vm_cpus(accel).to_string());
    args.push("-drive".into());
    args.push(format!(
        "if=pflash,format=raw,readonly=on,file={}",
        firmware.code.display()
    ));
    args.push("-drive".into());
    args.push("if=pflash,format=raw,file=ovmf-vars.fd".into());
    debug_assert!(firmware.vars.ends_with("ovmf-vars.fd"));
    args.push("-drive".into());
    args.push("format=raw,file=fat:rw:esp".into());
    let (accel_name, cpu) = match accel {
        // -cpu max exposes SMEP, SMAP and UMIP wherever the accelerator can.
        Accel::Kvm => ("kvm", "max"),
        Accel::Whpx => ("whpx", "max"),
        Accel::Tcg | Accel::Auto => ("tcg", "max"),
    };
    args.extend([
        "-accel".into(),
        accel_name.into(),
        "-cpu".into(),
        cpu.into(),
    ]);
    if headless {
        args.extend(["-display".into(), "none".into()]);
    }
    args
}

/// Asks the OS for a free local port. QEMU binds it right after, so a clash is unlikely.
pub fn free_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|e| format!("cannot find a free port for the QEMU monitor: {e}"))
}

/// Types keys into the guest through the QEMU monitor: a key name with
/// `sendkey`, and a `!` line as the monitor command after the `!`.
fn type_keys(port: u16, keys: &[&str]) {
    for _ in 0..50 {
        if let Ok(mut monitor) = TcpStream::connect(("127.0.0.1", port)) {
            thread::sleep(Duration::from_millis(200));
            for key in keys {
                let sent = match key.strip_prefix('!') {
                    Some(command) => writeln!(monitor, "{command}"),
                    None => writeln!(monitor, "sendkey {key}"),
                };
                if sent.is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(150));
            }
            println!("xtask: sent {} inputs through the QEMU monitor", keys.len());
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    eprintln!("xtask: could not reach the QEMU monitor on port {port}");
}

pub fn run_interactive(qemu: &Path, args: &[String], target_dir: &Path) -> Result<(), String> {
    let status = Command::new(qemu)
        .current_dir(target_dir)
        .args(args)
        .status()
        .map_err(|e| format!("cannot start QEMU: {e}"))?;
    println!("xtask: QEMU exited with {status}");
    Ok(())
}

pub struct TestRun<'a> {
    pub case: &'a TestCase,
    pub accel: Accel,
    pub timeout: Duration,
    /// TCP port of the QEMU human monitor, used to inject keystrokes.
    pub monitor: u16,
}

pub fn run_test(
    qemu: &Path,
    args: &[String],
    target_dir: &Path,
    run: &TestRun,
) -> Result<(), String> {
    let (accel, timeout, monitor) = (run.accel, run.timeout, run.monitor);
    let stages = run.case.keys;
    let expect = run.case.expect;
    let started = Instant::now();
    let mut child = Command::new(qemu)
        .current_dir(target_dir)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start QEMU: {e}"))?;
    let stdout = child.stdout.take().ok_or("QEMU stdout was not captured")?;
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut measure_start = None;
        let mut stage = 0;
        let mut missing: Vec<&'static str> = expect.to_vec();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            println!("  | {line}");
            missing.retain(|text| !line.contains(text));
            // Text from ring 3 (`[name] ...`) or the shell (`= ...`, `! ...`, `? ...`) may
            // say anything, so it never counts as a kernel marker.
            if from_programs(&line) {
                continue;
            }
            if line.contains("boot-ok") {
                let _ = ready_tx.send(started.elapsed());
            } else if line.contains("ready for keys") {
                if let Some(&keys) = stages.get(stage) {
                    stage += 1;
                    thread::spawn(move || type_keys(monitor, keys));
                }
            } else if line.contains("measure-start") {
                measure_start = Some(Instant::now());
            } else if line.contains("measure-end")
                && let Some(start) = measure_start
            {
                println!(
                    "xtask: host wall time for the measurement: {} ms",
                    start.elapsed().as_millis()
                );
            }
        }
        missing
    });
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("waiting on QEMU: {e}"))?
        {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timed out after {} s", timeout.as_secs()));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let missing = reader.join().unwrap_or_else(|_| expect.to_vec());
    if let Ok(elapsed) = ready_rx.try_recv() {
        println!(
            "xtask: boot-to-ready {} ms (accel {:?}, includes firmware)",
            elapsed.as_millis(),
            accel
        );
    }
    if status.code() != Some(QEMU_PASS) {
        return Err(format!("FAIL: QEMU exited with {status}"));
    }
    if !missing.is_empty() {
        return Err(format!("FAIL: never saw {missing:?} on the console"));
    }
    println!("xtask: PASS");
    Ok(())
}

/// Whether a console line was written for a ring-3 program rather than by the
/// kernel: `[name] ...` from a program's own output, `= ...`, `! ...` and `? ...` from
/// the shell's answers, possibly after the terminal's `kai> `, `edit <name>> `,
/// `run script N? yes/no> ` and `apply <label> to <name>? yes/no> ` prompts.
pub fn from_programs(line: &str) -> bool {
    let mut rest = line;
    while let Some(after) = rest
        .strip_prefix("kai> ")
        .or_else(|| strip_confirm_prompt(rest))
        .or_else(|| strip_edit_prompt(rest))
    {
        rest = after;
    }
    ["[", "= ", "! ", "? "]
        .iter()
        .any(|lead| rest.starts_with(lead))
}

fn is_word(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The text after a leading `edit <name>> ` or `apply <label> to <name>? yes/no> `.
fn strip_edit_prompt(line: &str) -> Option<&str> {
    if let Some(after) = line.strip_prefix("edit ") {
        let (name, rest) = after.split_once("> ")?;
        return is_word(name).then_some(rest);
    }
    let after = line.strip_prefix("apply ")?;
    let (label, after) = after.split_once(" to ")?;
    let (name, rest) = after.split_once("? yes/no> ")?;
    (is_word(label) && is_word(name)).then_some(rest)
}

/// The text after a leading `run script N? yes/no> ` prompt.
fn strip_confirm_prompt(line: &str) -> Option<&str> {
    let after = line.strip_prefix("run script ")?;
    let digits = after.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    after[digits..].strip_prefix("? yes/no> ")
}

#[cfg(test)]
mod tests {
    use super::from_programs;

    #[test]
    fn program_output_is_told_apart_from_kernel_lines() {
        assert!(from_programs("[llm] up (restart 0)"));
        assert!(from_programs("kai> [shell] terminal: ready for keys"));
        assert!(from_programs("= boot-ok"));
        assert!(from_programs("kai> ! stopped: time limit"));
        assert!(!from_programs("terminal: ready for keys"));
        assert!(!from_programs("kai> terminal: ready for keys"));
        assert!(!from_programs("boot-ok"));
        assert!(from_programs("? run it? it CHANGES set_policy. yes/no"));
        assert!(from_programs("run script 12? yes/no> = boot-ok"));
        assert!(!from_programs(
            "run script 1? yes/no> terminal: ready for keys"
        ));
    }

    #[test]
    fn edit_and_apply_prompts_are_stripped_too() {
        assert!(from_programs("edit notes> = ok built e1 from 2"));
        assert!(from_programs(
            "apply e1 to notes? yes/no> = ok probation e1 (3 calls)"
        ));
        assert!(!from_programs("edit notes> boot-ok"));
        assert!(!from_programs("apply e1 to notes? yes/no> boot-ok"));
        assert!(!from_programs("edit Notes> = x"));
    }
}
