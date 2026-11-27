//! Building QEMU command lines and running QEMU, interactively or as a test.

use crate::cases::TestCase;
use crate::disk::{BASE_MEMORY_MIB, ModelDisk};
use crate::options::Sizing;
use crate::{Accel, Firmware, image};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

mod console;
mod keys;
mod net;
mod shutdown;

use console::{CPU_LINE, ci_warning, forbidden_verdict};
pub use console::{from_programs, without_stamp};
use keys::Typist;
#[cfg(test)]
use net::command_word;
use net::{NET_DEVICE, USER_NET};
pub use net::{forward_https, input_args, net_behind_bridge, net_without_vectors, netdev_from_env};

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
        // The hardware watchdog (ADR 0008 §5): the kernel arms it only on a
        // trial slot's boot, and its reset is QEMU's default action. At slot
        // 0x1e, which q35 leaves free, so no other device's address moves.
        "-device",
        "i6300esb,addr=0x1e",
        "-netdev",
        USER_NET,
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

/// The slot cases: builds the R/A/B disk of `esp` (ADR 0008) as
/// `target/slots.img` and boots from it in place of the directory ESP. With
/// `keep`, the disk keeps what the last boot left: `kai-data` if `boot.data`,
/// else slot B (a resumed download).
pub fn boot_from_slots(
    args: &mut Vec<String>,
    esp: &Path,
    target_dir: &Path,
    cmdline: &str,
    record: &str,
    keep: bool,
    boot: BootDisk,
) -> Result<(), String> {
    let out = target_dir.join(SLOTS_IMAGE);
    let sector = if boot.four_k { 4096 } else { 512 };
    let bytes = match keep {
        _ if boot.data => image::slot_data::build(esp, &out, cmdline, record, sector, !keep)?,
        true if boot.four_k => return Err("a 4Kn slot disk does not keep slot B".into()),
        true => image::slots::build_keeping_b(esp, &out, cmdline, record)?,
        false => image::slots::build(esp, &out, cmdline, record, sector)?,
    };
    let on = if boot.nvme { "NVMe" } else { "virtio-blk" };
    println!(
        "xtask: slot disk {SLOTS_IMAGE}, {} MiB, {sector}-byte sectors on {on}",
        bytes >> 20
    );
    boot_from_disk_on(args, SLOTS_IMAGE, boot)
}

/// The boot disk's device (G58): virtio-blk or NVMe, of 512-byte or 4 KiB blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootDisk {
    pub nvme: bool,
    pub four_k: bool,
    /// `kai-data` as partition 5 (G58 part C), not a disk of its own.
    pub data: bool,
}

impl BootDisk {
    /// The `-device` for the drive `slots`, serial `kai-boot`, booted first.
    fn device(self) -> String {
        let kind = if self.nvme { "nvme" } else { "virtio-blk-pci" };
        let blocks = [crate::disk::BLOCKS_4K, ""][usize::from(!self.four_k)];
        format!("{kind},drive=slots,serial=kai-boot,bootindex=0{blocks}")
    }
}

/// Boots from the disk image `name` (relative to `target/`, or absolute) in
/// place of the directory ESP, on virtio-blk, so the kernel reaches the slots
/// (`updated`'s SLOT_* calls) as an installed system reaches its one disk.
pub fn boot_from_disk(args: &mut Vec<String>, name: &str) -> Result<(), String> {
    boot_from_disk_on(args, name, BootDisk::default())
}

/// [`boot_from_disk`] on the device `boot` names.
pub fn boot_from_disk_on(args: &mut Vec<String>, name: &str, boot: BootDisk) -> Result<(), String> {
    let at = args.iter().position(|arg| arg == ESP_DRIVE);
    let at = at.ok_or("no directory ESP drive to replace")?;
    args[at] = format!("if=none,id=slots,format=raw,file={name}");
    args.splice(at + 1..at + 1, ["-device".to_string(), boot.device()]);
    Ok(())
}

/// The update disk `name` in `target/`, read-only, found by its serial.
pub fn update_disk_args(name: &str) -> [String; 4] {
    [
        "-drive".into(),
        format!("if=none,id=update,format=raw,readonly=on,file={name}"),
        "-device".into(),
        "virtio-blk-pci,drive=update,serial=kai-update".into(),
    ]
}

/// QEMU arguments attaching `name`, a file in `target/` as the update disk is,
/// read-only as the import disk, by its serial (`kabi::update::IMPORT_SERIAL`).
pub fn import_disk_args(name: &str) -> [String; 4] {
    [
        "-drive".into(),
        format!("if=none,id=import,format=raw,readonly=on,file={name}"),
        "-device".into(),
        format!(
            "virtio-blk-pci,drive=import,serial={}",
            kabi::update::IMPORT_SERIAL
        ),
    ]
}

/// The slot cases' disk, in `target/`.
const SLOTS_IMAGE: &str = "slots.img";

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

pub fn run_interactive(qemu: &Path, args: &[String], target_dir: &Path) -> Result<(), String> {
    let status = Command::new(qemu)
        .current_dir(target_dir)
        .args(args)
        .status()
        .map_err(|e| format!("cannot start QEMU: {e}"))?;
    println!("xtask: QEMU exited with {status}");
    Ok(())
}

/// How long a power-off case waits, once its lines are in, for QMP's
/// SHUTDOWN event.
const POWER_OFF_WAIT: Duration = Duration::from_secs(15);

/// A power-off case passes only on QEMU's `guest-shutdown`: a reset is a FAIL.
fn power_off_verdict(reason: Option<&str>) -> Result<(), String> {
    match reason {
        Some(shutdown::GUEST_SHUTDOWN) => Ok(()),
        Some(other) => Err(format!(
            "FAIL: QEMU stopped the guest for {other}, not a power-off"
        )),
        None => Err(String::from(
            "FAIL: the guest did not turn the machine off (QMP reported no shutdown)",
        )),
    }
}

/// How long a reset case waits, once QEMU has exited with status 0, for the
/// QMP listener to hand on the SHUTDOWN event QEMU sent before it went.
const RESET_WAIT: Duration = Duration::from_secs(10);

/// A reset case that ends QEMU with status 0 passes only on `guest-reset`:
/// under `-no-reboot` a power-off ends it with status 0 as well (bug hunt 5
/// H5-4). And only on the reset it names: the hardware watchdog's for a
/// `watchdog` case, never the watchdog's for any other, so a boot that hung
/// where it should have reset does not pass on the watchdog's reset.
fn reset_verdict(got: Option<&shutdown::Shutdown>, watchdog: bool) -> Result<(), String> {
    let Some(got) = got else {
        return Err(String::from(
            "FAIL: QEMU exited with status 0, but QMP reported no reset",
        ));
    };
    if got.reason != shutdown::GUEST_RESET {
        return Err(format!(
            "FAIL: QEMU stopped the guest for {}, not a reset",
            got.reason
        ));
    }
    match (got.watchdog, watchdog) {
        (true, false) => Err(String::from(
            "FAIL: the hardware watchdog reset the guest, not the guest itself",
        )),
        (false, true) => Err(String::from(
            "FAIL: the guest reset itself; the hardware watchdog never fired",
        )),
        _ => Ok(()),
    }
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
    /// The case's `differs` text, and where the rest of the first kernel line
    /// containing it is put.
    pub capture: Option<(&'static str, Arc<Mutex<Option<String>>>)>,
}

pub fn run_test(
    qemu: &Path,
    args: &[String],
    target_dir: &Path,
    run: &TestRun,
) -> Result<(), String> {
    let (accel, timeout, monitor, qmp) = (run.accel, run.timeout, run.monitor, run.qmp);
    let (stages, keys_on, stops) = (run.case.keys, run.case.keys_on, run.case.stops);
    let (powers_off, resets) = (run.case.powers_off, run.case.resets);
    let forbid = run.case.forbidden();
    let expect: Vec<String> = run
        .case
        .expect
        .iter()
        .map(|text| text.to_string())
        .collect();
    let expect = [expect, run.contains.clone()].concat();
    let exact: Vec<String> = run
        .exact
        .iter()
        .cloned()
        .chain(run.case.exact.iter().map(|line| line.to_string()))
        .collect();
    let started = Instant::now();
    let mut child = crate::wsl::command(qemu)
        .current_dir(target_dir)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start QEMU: {e}"))?;
    let stdout = child.stdout.take().ok_or("QEMU stdout was not captured")?;
    // Heard from the start: the event comes once, maybe before the last line.
    let shutdown = match (powers_off || resets, qmp) {
        (true, Some(port)) => Some(shutdown::listen(port)),
        (true, None) => {
            return Err(String::from("a power-off or reset case needs a QMP socket"));
        }
        (false, _) => None,
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let (cut_tx, cut_rx) = mpsc::channel();
    let (seen_tx, seen_rx) = mpsc::channel();
    let cut_at = run.cut.clone();
    let capture = run.capture.clone();
    let all = (expect.clone(), exact.clone(), None, Vec::new(), None);
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
        let mut typist = Typist::new(monitor, qmp);
        let (mut missing, mut unseen) = (expect, exact);
        let mut cpu_line = None;
        let mut warnings = Vec::new();
        let mut forbidden: Option<String> = None;
        for raw in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = echo_tx.send(raw.clone());
            let line = without_stamp(&raw).to_string();
            // A forbidden text fails the case from any program's line too.
            if forbidden.is_none() && forbid.iter().any(|text| line.contains(text)) {
                forbidden = Some(line.trim_end().to_string());
            }
            missing.retain(|text| !line.contains(text.as_str()));
            unseen.retain(|text| line.trim_end_matches('\r') != text);
            if (stops || powers_off) && missing.is_empty() && unseen.is_empty() {
                let _ = seen_tx.send(());
            }
            // A named line sends the next stage, whoever printed it.
            if let (Some(&on), Some(&keys)) = (keys_on.get(stage), stages.get(stage))
                && line.contains(on)
            {
                stage += 1;
                typist.send(keys);
            }
            if let Some(text) = ci_warning(&line) {
                warnings.push(text.to_string());
            }
            if cpu_line.is_none() && !from_programs(&line) && line.contains(CPU_LINE) {
                cpu_line = Some(line.trim_end_matches('\r').to_string());
            }
            // Text from ring 3 (`[name] ...`) or the shell (`= ...`, `! ...`, `? ...`) may
            // say anything, and a `screen:` row copies whatever the screen shows, so
            // none of them counts as a kernel marker.
            if from_programs(&line) {
                continue;
            }
            if let Some((text, slot)) = &capture
                && let Some((_, rest)) = line.split_once(text)
                && let Ok(mut slot) = slot.lock()
                && slot.is_none()
            {
                *slot = Some(rest.trim_end_matches('\r').to_string());
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
            } else if keys_on.is_empty() && line.contains("ready for keys") {
                if let Some(&keys) = stages.get(stage) {
                    stage += 1;
                    typist.send(keys);
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
        (missing, unseen, cpu_line, warnings, forbidden)
    });
    let mut cut_seen = None;
    let mut stopped = false;
    let mut shutdown_reason = None;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("waiting on QEMU: {e}"))?
        {
            break status;
        }
        if seen_rx.try_recv().is_ok() {
            // A power-off case: the guest must have turned the machine off,
            // not reset it (both pause QEMU under -no-shutdown).
            if let Some(events) = shutdown.as_ref().filter(|_| powers_off) {
                shutdown_reason = events.recv_timeout(POWER_OFF_WAIT).ok().map(|s| s.reason);
            }
            // Every expected line is in: nothing more is waited for.
            let _ = child.kill();
            stopped = true;
            break child.wait().map_err(|e| format!("waiting on QEMU: {e}"))?;
        }
        cut_seen = cut_seen.or_else(|| cut_rx.try_recv().ok());
        if let (Some(seen), Some((_, delay))) = (cut_seen, &run.cut)
            && seen.elapsed() >= *delay
        {
            // The power goes: QEMU stops at once, mid-write.
            let _ = child.kill();
            let _ = child.wait();
            let forbidden = reader.join().ok().and_then(|(.., forbidden)| forbidden);
            let _ = echo.join();
            println!("xtask: power cut {} ms after the line", delay.as_millis());
            // A cut boot still fails on a forbidden line it printed first.
            return forbidden_verdict(forbidden);
        }
        if started.elapsed() > timeout {
            // Where every vCPU is, before QEMU goes: a silent hang shows its code.
            crate::hang::dump(
                monitor,
                &target_dir.join("esp/boot").join(crate::esp::BUNDLE),
            );
            let _ = child.kill();
            let _ = child.wait();
            // A forbidden line before the hang is often why: say it too.
            let forbidden = reader.join().ok().and_then(|(.., forbidden)| forbidden);
            let _ = echo.join();
            let seen = forbidden.map_or_else(String::new, |line| {
                format!("; a forbidden line was on the console first: {line}")
            });
            return Err(format!("timed out after {} s{seen}", timeout.as_secs()));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let (missing, unseen, cpu_line, warnings, forbidden) = reader.join().unwrap_or(all);
    // Every echoed line is out before the verdict.
    let _ = echo.join();
    for text in warnings {
        println!("::warning::{text}");
    }
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
    // Before QEMU's own verdict: a forbidden line is said however the run ended.
    forbidden_verdict(forbidden)?;
    let reset = resets && !stopped && status.code() == Some(0);
    if powers_off {
        power_off_verdict(shutdown_reason.as_deref())?;
    }
    // Status 0 under -no-reboot is a reset or a power-off: only QMP's reason
    // tells them apart. QEMU sent the event before it exited.
    if let Some(events) = shutdown.as_ref().filter(|_| reset) {
        let got = events.recv_timeout(RESET_WAIT).ok();
        reset_verdict(got.as_ref(), run.case.watchdog)?;
    }
    if stopped {
        println!("xtask: every expected line seen; QEMU stopped");
    } else if status.code() != Some(QEMU_PASS) && !reset {
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

#[cfg(test)]
mod tests {
    use super::{
        Typist, ci_warning, command_word, forward_https, from_programs, input_args, model_sizing,
        power_off_verdict, reset_verdict, without_stamp,
    };
    use crate::cases::{TestCase, case};

    /// Only `guest-shutdown` passes: a reset that paused QEMU is a FAIL, and so
    /// is no event at all.
    #[test]
    fn a_power_off_passes_only_on_a_guest_shutdown() {
        assert_eq!(power_off_verdict(Some("guest-shutdown")), Ok(()));
        let reset = power_off_verdict(Some("guest-reset")).unwrap_err();
        assert!(
            reset.contains("for guest-reset, not a power-off"),
            "{reset}"
        );
        let none = power_off_verdict(None).unwrap_err();
        assert!(none.contains("QMP reported no shutdown"), "{none}");
    }

    /// A reset case's status 0 passes only on a `guest-reset`: a power-off,
    /// which also ends QEMU with status 0 under -no-reboot, is a FAIL, and so
    /// is no event at all.
    #[test]
    fn a_reset_passes_only_on_a_guest_reset() {
        let got = |reason: &str, watchdog| super::shutdown::Shutdown {
            reason: reason.into(),
            watchdog,
        };
        assert_eq!(
            reset_verdict(Some(&got("guest-reset", false)), false),
            Ok(())
        );
        let off = reset_verdict(Some(&got("guest-shutdown", false)), false).unwrap_err();
        assert!(off.contains("for guest-shutdown, not a reset"), "{off}");
        let none = reset_verdict(None, false).unwrap_err();
        assert!(none.contains("QMP reported no reset"), "{none}");
    }

    /// A `watchdog` case passes only on the watchdog's reset, and any other
    /// reset case fails on it.
    #[test]
    fn a_reset_passes_only_from_what_the_case_names() {
        let got = |watchdog| super::shutdown::Shutdown {
            reason: "guest-reset".into(),
            watchdog,
        };
        assert_eq!(reset_verdict(Some(&got(true)), true), Ok(()));
        let own = reset_verdict(Some(&got(false)), true).unwrap_err();
        assert!(own.contains("the hardware watchdog never fired"), "{own}");
        let barked = reset_verdict(Some(&got(true)), false).unwrap_err();
        assert!(
            barked.contains("the hardware watchdog reset the guest"),
            "{barked}"
        );
    }

    /// A power-off or reset case gets a QMP socket for its verdict, and may not
    /// share it with `@` lines.
    #[test]
    fn a_power_off_or_reset_case_gets_qmp_and_sends_no_at_lines() {
        for (powers_off, resets) in [(true, false), (false, true)] {
            let judged = TestCase {
                powers_off,
                resets,
                ..case("judged", "test=power-off")
            };
            let (args, port) = input_args(&judged).unwrap();
            assert!(port.is_some());
            assert!(args.iter().any(|arg| arg == "-qmp"));
            let typed = TestCase {
                powers_off,
                resets,
                keys: &[&["@{\"execute\":\"stop\"}"]],
                ..case("typed", "test=power-off")
            };
            let refused = input_args(&typed).unwrap_err();
            assert!(refused.contains("cannot send @ lines"), "{refused}");
        }
        assert_eq!(input_args(&case("plain", "test=x")).unwrap().1, None);
    }

    /// A stamped line matches as the same line without its stamp; a program's
    /// `[name] ` and anything stamp-like but not a stamp stay.
    #[test]
    fn a_stamp_comes_off_and_nothing_else_does() {
        assert_eq!(without_stamp("[12.345 h12.340] boot-ok"), "boot-ok");
        assert_eq!(
            without_stamp("[0.004 h0.004] [shell] script 1 ran"),
            "[shell] script 1 ran"
        );
        assert!(from_programs(without_stamp("[1.000 h1.000] [fs] fs: up")));
        assert!(!from_programs(without_stamp("[1.000 h1.000] fs: up")));
        for kept in [
            "[shell] script 1 ran",
            "[12.345] boot-ok",
            "[12.34 h12.340] x",
            "[a.345 h1.000] x",
            "[1.000 h1.000]boot-ok",
            "",
        ] {
            assert_eq!(without_stamp(kept), kept);
        }
    }
    use crate::Accel;
    use crate::options::Sizing;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::thread;

    /// The relay's path is one shell word in the `cmd:`, and its commas are
    /// doubled for QEMU's option parser (K's review of #753).
    #[test]
    fn the_relay_path_survives_quotes_and_commas() {
        assert_eq!(command_word("/a b/x"), "'/a b/x'");
        assert_eq!(command_word("/it's,here"), "'/it'\\''s,,here'");
        let mut args = vec!["-netdev".to_string(), "user,id=net0".to_string()];
        forward_https(&mut args, std::path::Path::new("/t/xtask"), 4321);
        assert_eq!(
            args[1],
            "user,id=net0,guestfwd=tcp:10.0.2.100:443-cmd:'/t/xtask' relay 127.0.0.1:4321"
        );
    }

    /// A monitor that serves one client at a time, as QEMU's does: stages sent
    /// together arrive in order, and the second does not even connect while the
    /// first is typing.
    #[test]
    fn stages_sent_together_are_typed_in_order_and_one_at_a_time() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let monitor = thread::spawn(move || {
            let (mut seen, mut early, mut waiting) = (Vec::new(), false, None);
            for _ in 0..2 {
                let client = match waiting.take() {
                    Some(client) => client,
                    None => listener.accept().unwrap().0,
                };
                let mut lines = BufReader::new(client).lines().map_while(Result::ok);
                seen.extend(lines.next());
                // The first key is in: nobody else may be waiting to be served.
                listener.set_nonblocking(true).unwrap();
                if let Ok((other, _)) = listener.accept() {
                    other.set_nonblocking(false).unwrap();
                    (early, waiting) = (true, Some(other));
                }
                listener.set_nonblocking(false).unwrap();
                seen.extend(lines);
            }
            (seen, early)
        });
        let mut typist = Typist::new(port, None);
        typist.send(&["a", "b"]);
        typist.send(&["c"]);
        let (seen, early) = monitor.join().unwrap();
        assert_eq!(seen, ["sendkey a", "sendkey b", "sendkey c"]);
        assert!(!early, "the second stage connected during the first");
    }

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
    fn only_a_kernel_line_becomes_a_ci_warning() {
        assert_eq!(
            ci_warning("ci-warning: desk-perf ungated (runner 2.8x slow)\r"),
            Some("desk-perf ungated (runner 2.8x slow)")
        );
        assert_eq!(ci_warning("[desktop] ci-warning: anything"), None);
        assert_eq!(ci_warning("kai> = ci-warning: anything"), None);
        assert_eq!(ci_warning("desk-perf: ci-warning: not at the start"), None);
        assert_eq!(ci_warning("ci-warning: "), None);
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
