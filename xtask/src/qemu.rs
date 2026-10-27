//! Building QEMU command lines and running QEMU, interactively or as a test.

use crate::cases::TestCase;
use crate::disk::{BASE_MEMORY_MIB, ModelDisk};
use crate::options::Sizing;
use crate::{Accel, Firmware};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// QEMU exit status for `isa-debug-exit` value 0x10: (0x10 << 1) | 1.
pub(crate) const QEMU_PASS: i32 = 33;
/// CPUs in the VM unless `--cpus` says otherwise. Emulation (TCG) gets no more CPUs
/// than the host has cores: on an overcommitted host, virtual CPUs wait for host
/// scheduling, which looks like guest latency the kernel cannot control. KVM
/// tolerates overcommit far better. `--cpus` is taken as given, up to the kernel's 8.
const DEFAULT_CPUS: usize = 4;

pub fn vm_cpus(accel: Accel, sizing: Sizing) -> usize {
    if let Some(cpus) = sizing.cpus {
        return cpus;
    }
    let host = thread::available_parallelism().map_or(1, usize::from);
    match accel {
        Accel::Tcg | Accel::Auto => DEFAULT_CPUS.min(host.max(1)),
        Accel::Kvm | Accel::Whpx => DEFAULT_CPUS,
    }
}

/// The VM `run` boots: [`model_sizing`] on this host with a model disk.
pub fn run_sizing(accel: Accel, sizing: Sizing, model: bool) -> Sizing {
    match model {
        true => {
            let host = thread::available_parallelism().map_or(1, usize::from);
            model_sizing(accel, sizing, host)
        }
        false => sizing,
    }
}

/// `run` with a model disk (L6): unless `--cpus` says otherwise, every host core
/// up to the kernel's 8 under KVM or WHPX, since the engine runs one worker per
/// vCPU; emulation keeps [`vm_cpus`]'s default.
fn model_sizing(accel: Accel, sizing: Sizing, host: usize) -> Sizing {
    match (sizing.cpus, accel) {
        (None, Accel::Kvm | Accel::Whpx) => Sizing {
            cpus: Some(host.clamp(1, crate::options::MAX_VM_CPUS)),
            ..sizing
        },
        _ => sizing,
    }
}

/// Guest memory in MiB: `--mem` when given, but never less than the model disk
/// (or the base 512 MiB without one) needs.
pub fn vm_memory_mib(disk: Option<&ModelDisk>, sizing: Sizing) -> u64 {
    let needed = disk.map_or(BASE_MEMORY_MIB, ModelDisk::memory_mib);
    sizing.mem_mib.map_or(needed, |asked| asked.max(needed))
}

/// QEMU runs with `target/` as its working directory, so drive paths stay relative
/// and free of drive-letter colons, which QEMU's option parser would misread. A
/// model disk is the exception, attached by an absolute path in a JSON
/// `-blockdev` (see `disk.rs`), with guest memory sized for it.
/// Every VM gets a virtio network card on QEMU's user-mode network: the guest is
/// 10.0.2.15, and 10.0.2.2 reaches the host's 127.0.0.1.
pub fn args(
    firmware: &Firmware,
    accel: Accel,
    headless: bool,
    disk: Option<&ModelDisk>,
    sizing: Sizing,
) -> Vec<String> {
    let memory = format!("{}M", vm_memory_mib(disk, sizing));
    let mut args: Vec<String> = [
        "-machine",
        "q35",
        "-m",
        memory.as_str(),
        "-no-reboot",
        "-serial",
        "stdio",
        "-device",
        "isa-debug-exit,iobase=0xf4,iosize=0x04",
        "-netdev",
        "user,id=net0",
        "-device",
        NET_DEVICE,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push("-smp".into());
    args.push(vm_cpus(accel, sizing).to_string());
    args.push("-drive".into());
    args.push(format!(
        "if=pflash,format=raw,readonly=on,file={}",
        firmware.code.display()
    ));
    args.push("-drive".into());
    args.push("if=pflash,format=raw,file=ovmf-vars.fd".into());
    debug_assert!(firmware.vars.ends_with("ovmf-vars.fd"));
    args.push("-drive".into());
    args.push(ESP_DRIVE.into());
    if let Some(disk) = disk {
        args.extend(disk.qemu_args());
    }
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

/// The image case: builds the GPT disk image of `esp` as `target/boot.img` and
/// boots from it in place of the directory ESP.
pub fn boot_from_image(args: &mut [String], esp: &Path, target_dir: &Path) -> Result<(), String> {
    let bytes = crate::image::build(esp, &target_dir.join(IMAGE))?;
    println!("xtask: boot image {IMAGE}, {} MiB", bytes >> 20);
    let drive = args
        .iter_mut()
        .find(|arg| *arg == ESP_DRIVE)
        .ok_or("no directory ESP drive to replace")?;
    *drive = format!("format=raw,file={IMAGE}");
    Ok(())
}

/// The directory ESP, as QEMU's virtual FAT drive.
const ESP_DRIVE: &str = "format=raw,file=fat:rw:esp";
/// The image case's disk, in `target/`.
const IMAGE: &str = "boot.img";

/// Asks the OS for a free local port. QEMU binds it right after, so a clash is unlikely.
pub fn free_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|e| format!("cannot find a free port for the QEMU monitor: {e}"))
}

/// Types keys into the guest through the QEMU monitor: a key name with
/// `sendkey`, a `!` line as the monitor command after the `!`, and an `@` line
/// as a QMP command on `qmp`.
fn type_keys(port: u16, qmp: Option<u16>, keys: &[&str]) {
    let mut qmp = qmp.map(Qmp::new);
    for _ in 0..50 {
        if let Ok(mut monitor) = TcpStream::connect(("127.0.0.1", port)) {
            thread::sleep(Duration::from_millis(200));
            for key in keys {
                let sent = match (key.strip_prefix('!'), key.strip_prefix('@')) {
                    (Some(command), _) => writeln!(monitor, "{command}"),
                    (_, Some(command)) => match qmp.as_mut() {
                        Some(qmp) => qmp.run(command),
                        None => Err(std::io::Error::other("no QMP socket for an @ line")),
                    },
                    _ => writeln!(monitor, "sendkey {key}"),
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

/// A QMP connection, opened on first use: QMP takes one client at a time and
/// wants its capabilities negotiated first.
struct Qmp {
    port: u16,
    stream: Option<BufReader<TcpStream>>,
}

impl Qmp {
    fn new(port: u16) -> Self {
        Self { port, stream: None }
    }

    /// Sends one command and waits for its answer, skipping events. The first
    /// command connects and negotiates capabilities.
    fn run(&mut self, command: &str) -> std::io::Result<()> {
        if self.stream.is_none() {
            let stream = TcpStream::connect(("127.0.0.1", self.port))?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            let mut reader = BufReader::new(stream);
            let mut greeting = String::new();
            reader.read_line(&mut greeting)?;
            Self::send(&mut reader, r#"{"execute":"qmp_capabilities"}"#)?;
            self.stream = Some(reader);
        }
        let reader = self.stream.as_mut().expect("connected just above");
        Self::send(reader, command)
    }

    fn send(reader: &mut BufReader<TcpStream>, command: &str) -> std::io::Result<()> {
        writeln!(reader.get_mut(), "{command}")?;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Err(std::io::Error::other("QMP closed the connection"));
            }
            if line.contains("\"return\"") {
                return Ok(());
            }
            if line.contains("\"error\"") {
                return Err(std::io::Error::other(format!("QMP: {}", line.trim())));
            }
        }
    }
}

/// The network card's `-device` value.
const NET_DEVICE: &str = "virtio-net-pci,netdev=net0";

/// Moves the network card behind a PCIe root port of its own (bus 1).
pub fn net_behind_bridge(args: &mut Vec<String>) {
    if let Some(at) = args.iter().position(|arg| arg == NET_DEVICE) {
        args[at] = format!("{NET_DEVICE},bus=kai-rp0");
        let port = [
            "pcie-root-port,id=kai-rp0,chassis=1".into(),
            "-device".into(),
        ];
        args.splice(at..at, port);
    }
}

/// The `qemu-xhci` controller with the case's USB devices, and a QMP socket
/// when the case sends `@` lines; the port goes to the test run.
pub fn input_args(case: &TestCase) -> Result<(Vec<String>, Option<u16>), String> {
    let mut args = Vec::new();
    if !case.usb.is_empty() {
        args.extend(["-device".into(), "qemu-xhci,id=xhci".into()]);
        for device in case.usb {
            args.extend(["-device".into(), format!("{device},bus=xhci.0")]);
        }
    }
    let wants_qmp = case
        .keys
        .iter()
        .flat_map(|stage| stage.iter())
        .any(|key| key.starts_with('@'));
    if !wants_qmp {
        return Ok((args, None));
    }
    let port = free_port()?;
    args.extend([
        "-qmp".into(),
        format!("tcp:127.0.0.1:{port},server=on,wait=off"),
    ]);
    Ok((args, Some(port)))
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
    /// TCP port of QMP, for the case's `@` lines.
    pub qmp: Option<u16>,
    /// Lines the console must show exactly, worked out on the host (the
    /// `local-ask` case's golden answer).
    pub exact: Vec<String>,
    /// More text the console must contain, besides the case's own.
    pub contains: Vec<String>,
    /// A power cut: once a kernel line containing this text appears, QEMU is
    /// killed this much later, and the boot counts as done.
    pub cut: Option<(String, Duration)>,
}

pub fn run_test(
    qemu: &Path,
    args: &[String],
    target_dir: &Path,
    run: &TestRun,
) -> Result<(), String> {
    let (accel, timeout, monitor, qmp) = (run.accel, run.timeout, run.monitor, run.qmp);
    let stages = run.case.keys;
    let expect: Vec<String> = run
        .case
        .expect
        .iter()
        .map(|text| text.to_string())
        .collect();
    let expect = [expect, run.contains.clone()].concat();
    let exact = run.exact.clone();
    let started = Instant::now();
    let mut child = crate::wsl::command(qemu)
        .current_dir(target_dir)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start QEMU: {e}"))?;
    let stdout = child.stdout.take().ok_or("QEMU stdout was not captured")?;
    let (ready_tx, ready_rx) = mpsc::channel();
    let (cut_tx, cut_rx) = mpsc::channel();
    let cut_at = run.cut.clone();
    let all = (expect.clone(), exact.clone(), None);
    // The echo runs apart from the reader, as in icount.rs: printing to a slow CI
    // log must never stop QEMU's output being read. A full pipe makes QEMU hold
    // the serial port busy, and the guest then waits for it with interrupts off,
    // so its CPUs miss ticks (#46 measured stalls of 100-770 ms under KVM).
    let (echo_tx, echo_rx) = mpsc::channel::<String>();
    let echo = thread::spawn(move || {
        for line in echo_rx {
            println!("  | {line}");
        }
    });
    let reader = thread::spawn(move || {
        let mut measure_start = None;
        let mut stage = 0;
        let (mut missing, mut unseen) = (expect, exact);
        let mut cpu_line = None;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = echo_tx.send(line.clone());
            missing.retain(|text| !line.contains(text.as_str()));
            unseen.retain(|text| line.trim_end_matches('\r') != text);
            if cpu_line.is_none() && !from_programs(&line) && line.contains(CPU_LINE) {
                cpu_line = Some(line.trim_end_matches('\r').to_string());
            }
            // Text from ring 3 (`[name] ...`) or the shell (`= ...`, `! ...`, `? ...`) may
            // say anything, and a `screen:` row copies whatever the screen shows, so
            // none of them counts as a kernel marker.
            if from_programs(&line) {
                continue;
            }
            if let Some((text, _)) = &cut_at
                && line.contains(text.as_str())
            {
                let _ = cut_tx.send(Instant::now());
            }
            if line.contains("boot-ok") {
                let _ = ready_tx.send(started.elapsed());
            } else if line.contains("skip keys") {
                stage += 1;
            } else if line.contains("ready for keys") {
                if let Some(&keys) = stages.get(stage) {
                    stage += 1;
                    thread::spawn(move || type_keys(monitor, qmp, keys));
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
        (missing, unseen, cpu_line)
    });
    let mut cut_seen = None;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("waiting on QEMU: {e}"))?
        {
            break status;
        }
        cut_seen = cut_seen.or_else(|| cut_rx.try_recv().ok());
        if let (Some(seen), Some((_, delay))) = (cut_seen, &run.cut)
            && seen.elapsed() >= *delay
        {
            // The power goes: QEMU stops at once, mid-write.
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let _ = echo.join();
            println!("xtask: power cut {} ms after the line", delay.as_millis());
            return Ok(());
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timed out after {} s", timeout.as_secs()));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let (missing, unseen, cpu_line) = reader.join().unwrap_or(all);
    // Every echoed line is out before the verdict.
    let _ = echo.join();
    if let Some(line) = cpu_line {
        println!("xtask: guest {}", &line[line.find(CPU_LINE).unwrap_or(0)..]);
    }
    if let Ok(elapsed) = ready_rx.try_recv() {
        println!(
            "xtask: boot-to-ready {} ms (accel {:?}, includes firmware)",
            elapsed.as_millis(),
            accel
        );
    }
    let reset = run.case.resets && status.code() == Some(0);
    if status.code() != Some(QEMU_PASS) && !reset {
        return Err(format!("FAIL: QEMU exited with {status}"));
    }
    if !missing.is_empty() {
        return Err(format!("FAIL: never saw {missing:?} on the console"));
    }
    if !unseen.is_empty() {
        return Err(format!("FAIL: never saw the exact lines {unseen:?}"));
    }
    println!("xtask: PASS");
    Ok(())
}

/// The kernel's boot line naming the guest's vector features and XCR0, which a
/// test repeats at its end.
const CPU_LINE: &str = "cpu: vector features:";

/// Whether a console line was written for a ring-3 program rather than by the
/// kernel: `[name] ...` from a program's own output, `= ...`, `! ...` and `? ...` from
/// the shell's answers, possibly after the terminal's `kai> `, `edit <name>> `,
/// `run script N? y/n> ` and `apply <label> to <name>? yes/no> ` prompts, which
/// start the line the terminal logs on Enter. `screenlog`'s `screen: ` lines count
/// too: a screen row copies program text and kernel lines alike.
pub fn from_programs(line: &str) -> bool {
    if line.starts_with("screen: ") {
        return true;
    }
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

/// The text after a leading `run script N? y/n> ` or `start trial N? yes/no> `
/// prompt.
fn strip_confirm_prompt(line: &str) -> Option<&str> {
    let after = line
        .strip_prefix("run script ")
        .or_else(|| line.strip_prefix("start trial "))?;
    let digits = after.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = &after[digits..];
    after
        .strip_prefix("? yes/no> ")
        .or_else(|| after.strip_prefix("? y/n> "))
}

#[cfg(test)]
mod tests {
    use super::{from_programs, model_sizing};
    use crate::Accel;
    use crate::options::Sizing;

    #[test]
    fn program_output_is_told_apart_from_kernel_lines() {
        assert!(from_programs("[llm] up (restart 0)"));
        assert!(from_programs("kai> [shell] terminal: ready for keys"));
        assert!(from_programs("= boot-ok"));
        assert!(from_programs("kai> ! stopped: time limit"));
        assert!(!from_programs("terminal: ready for keys"));
        assert!(!from_programs("kai> terminal: ready for keys"));
        assert!(!from_programs("boot-ok"));
        assert!(from_programs(
            "? It changes set_policy (no undo). Go ahead? y/n, or show"
        ));
        assert!(from_programs("run script 12? y/n> = boot-ok"));
        assert!(!from_programs(
            "run script 1? y/n> terminal: ready for keys"
        ));
        assert!(from_programs(
            "start trial 2? yes/no> [shell] tune 2: trial 1 started"
        ));
        assert!(!from_programs(
            "start trial 2? yes/no> tune-demo: ready for keys"
        ));
    }

    #[test]
    fn screen_rows_never_count_as_kernel_lines() {
        assert!(from_programs("screen: 4|terminal: ready for keys"));
        assert!(from_programs("screen: cursor 4 8"));
        assert!(!from_programs("terminal: ready for keys"));
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

    #[test]
    fn a_model_gets_every_host_core_up_to_eight_under_acceleration() {
        let none = Sizing {
            cpus: None,
            mem_mib: None,
        };
        let cpus = |accel, host| model_sizing(accel, none, host).cpus;
        assert_eq!(cpus(Accel::Kvm, 16), Some(8));
        assert_eq!(cpus(Accel::Whpx, 6), Some(6));
        assert_eq!(cpus(Accel::Tcg, 16), None);
        let asked = Sizing {
            cpus: Some(2),
            ..none
        };
        assert_eq!(model_sizing(Accel::Kvm, asked, 16).cpus, Some(2));
    }
}
