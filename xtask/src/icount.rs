//! `cargo xtask icount [--shift N] [--boots N] [--timeout SECS]`: the M10a-2
//! measurement spike. Boots the `icount-probe` case under QEMU's instruction
//! counter several times and requires every boot to print the same digest.
//!
//! The measurement line is single-threaded TCG with `-icount`, one CPU, a guest
//! clock that follows the instruction count (`sleep=off`, so idle time costs no host
//! time, and `align=off`), an RTC on that clock, and no network card, so no
//! host-timed packet ever reaches the guest. It is refused together with KVM or
//! WHPX. Each boot gets a fresh copy of the OVMF variable store, so one boot's
//! NVRAM writes cannot reach the next. The probe compares only times taken
//! relative to a tick-aligned start, since firmware and boot-disk I/O before the
//! kernel stay host-timed. Each boot also reports host seconds per virtual second,
//! the number that fixes `shift` and the trial's window length.

use crate::esp::{self, Module};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Virtual nanoseconds per guest instruction are 2^shift. 3 gives 125 million
/// instructions per virtual second; the CI job reports what that costs.
pub const DEFAULT_SHIFT: u32 = 3;
/// At shift 6 a virtual millisecond is 15,625 instructions. Much fewer and the
/// debug kernel's 1 kHz tick handler cannot finish between two ticks, so the
/// guest livelocks in it and the boot only ends at the timeout.
pub const MAX_SHIFT: u32 = 6;
pub const DEFAULT_BOOTS: usize = 2;
/// The probe's seed. Determinism is checked by repeating one seed.
const SEED: u64 = 1;

/// The QEMU command line of the measurement mode.
pub fn measure_args(firmware: &crate::Firmware, shift: u32) -> Vec<String> {
    measure_args_with(firmware, shift, crate::disk::BASE_MEMORY_MIB)
}

/// [`measure_args`] with `memory_mib` of guest memory, for a boot with a disk.
pub fn measure_args_with(firmware: &crate::Firmware, shift: u32, memory_mib: u64) -> Vec<String> {
    let memory = format!("{memory_mib}M");
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
        "-nic",
        "none",
        "-smp",
        "1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push("-drive".into());
    args.push(format!(
        "if=pflash,format=raw,readonly=on,file={}",
        firmware.code.display()
    ));
    args.push("-drive".into());
    args.push("if=pflash,format=raw,file=ovmf-vars.fd".into());
    args.push("-drive".into());
    args.push("format=raw,file=fat:rw:esp".into());
    args.extend(
        [
            "-accel",
            "tcg,thread=single",
            "-icount",
            &format!("shift={shift},align=off,sleep=off"),
            "-cpu",
            "max",
            "-rtc",
            "base=2026-01-01T00:00:00,clock=vm",
            "-display",
            "none",
        ]
        .map(String::from),
    );
    args
}

/// What one boot printed, and how long the host took between its markers.
pub struct Captured {
    pub lines: Vec<String>,
    pub passed: bool,
    pub measure_host_ms: Option<u128>,
}

/// Runs QEMU to the end and returns every console line it printed.
pub fn run_capture(
    qemu: &Path,
    args: &[String],
    target_dir: &Path,
    timeout: Duration,
) -> Result<Captured, String> {
    let started = Instant::now();
    let mut child = Command::new(qemu)
        .current_dir(target_dir)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start QEMU: {e}"))?;
    let stdout = child.stdout.take().ok_or("QEMU stdout was not captured")?;
    let (done_tx, done_rx) = mpsc::channel();
    // The echo runs apart from the reader, so a slow terminal or CI log never
    // stops QEMU's output being read. A full pipe would make the guest's serial
    // writes wait on the host, and its timing differ from boot to boot.
    let (echo_tx, echo_rx) = mpsc::channel::<String>();
    let echo = thread::spawn(move || {
        for line in echo_rx {
            println!("  | {line}");
        }
    });
    let reader = thread::spawn(move || {
        let (mut lines, mut start, mut measured) = (Vec::new(), None, None);
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = echo_tx.send(line.clone());
            let line = line.trim_end_matches('\r').to_string();
            if !crate::qemu::from_programs(&line) {
                if line.contains("measure-start") {
                    start = Some(Instant::now());
                } else if let (true, Some(at)) = (line.contains("measure-end"), start) {
                    measured = Some(at.elapsed().as_millis());
                }
            }
            lines.push(line);
        }
        let _ = done_tx.send(());
        (lines, measured)
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
    let _ = done_rx.recv_timeout(Duration::from_secs(5));
    let (lines, measure_host_ms) = reader.join().unwrap_or_default();
    // The reader's sender is gone, so the echo ends once it has printed the rest.
    let _ = echo.join();
    Ok(Captured {
        lines,
        passed: status.code() == Some(crate::qemu::QEMU_PASS),
        measure_host_ms,
    })
}

/// One boot's result, read from its console lines.
#[derive(Debug, PartialEq, Eq)]
pub struct Probe {
    pub seed: u64,
    pub digest: String,
    /// The per-part digests, for telling where two boots diverged.
    pub parts: String,
    pub virtual_us: u64,
}

pub fn parse_probe(lines: &[String]) -> Result<Probe, String> {
    let field = |prefix: &str| {
        lines
            .iter()
            .filter(|line| !crate::qemu::from_programs(line))
            .find_map(|line| line.split_once(prefix).map(|(_, rest)| rest.trim()))
            .ok_or(format!("the probe never printed {prefix:?}"))
    };
    let seed = field("icount-probe: seed ")?
        .split(';')
        .next()
        .and_then(|seed| seed.parse().ok())
        .ok_or("cannot read the probe's seed")?;
    let digest = field("icount-probe: digest ")?.to_string();
    let parts = field("icount-probe: compute ")?.to_string();
    let timing = lines
        .iter()
        .filter(|line| !crate::qemu::from_programs(line))
        .find(|line| line.contains("icount-probe: ") && line.contains(" us of virtual time"))
        .ok_or("the probe never printed its virtual time")?;
    let virtual_us = timing
        .split("; ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|us| us.parse().ok())
        .ok_or(format!("cannot read the virtual time in {timing:?}"))?;
    Ok(Probe {
        seed,
        digest,
        parts,
        virtual_us,
    })
}

/// Host seconds per virtual second, to two decimals.
pub fn host_per_virtual(host_ms: u128, virtual_us: u64) -> String {
    if virtual_us == 0 {
        return "n/a".into();
    }
    let hundredths = host_ms * 100_000 / u128::from(virtual_us);
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// Requires every boot's digest to equal the first's; the error shows each boot's
/// per-part digests.
pub fn compare(probes: &[Probe]) -> Result<(), String> {
    let first = probes.first().ok_or("no boot finished")?;
    if probes.iter().all(|probe| probe.digest == first.digest) {
        return Ok(());
    }
    let detail: Vec<String> = probes
        .iter()
        .enumerate()
        .map(|(i, probe)| format!("boot {}: {} (compute {})", i + 1, probe.digest, probe.parts))
        .collect();
    Err(format!(
        "FAIL: the icount boots differ, so the measurement line is not deterministic: {}",
        detail.join("; ")
    ))
}

/// Boots the probe `options.boots` times with what `execute` already built.
pub fn run(
    qemu: &Path,
    target_dir: &Path,
    kernel: &Path,
    modules: &[Module],
    efi: &Path,
    options: &crate::Options,
) -> Result<(), String> {
    let (shift, boots) = (options.shift, options.boots);
    let cmdline = format!("test=icount-probe seed={SEED}");
    let esp_dir = target_dir.join("esp");
    let mut probes = Vec::new();
    for boot in 1..=boots {
        esp::make_esp(&esp_dir, kernel, modules, efi, &cmdline)?;
        // A fresh variable store for every boot.
        let firmware = crate::find_firmware(qemu, target_dir)?;
        let args = measure_args(&firmware, shift);
        println!("xtask: icount boot {boot} of {boots}: {}", args.join(" "));
        let captured = run_capture(qemu, &args, target_dir, options.timeout)?;
        if !captured.passed {
            return Err(format!("FAIL: icount boot {boot} did not pass the probe"));
        }
        let probe = parse_probe(&captured.lines)?;
        if probe.seed != SEED {
            return Err(format!(
                "FAIL: icount boot {boot} ran seed {}, not the seed {SEED} it was given",
                probe.seed
            ));
        }
        let host = captured.measure_host_ms.ok_or(format!(
            "icount boot {boot} never printed both measure markers"
        ))?;
        println!(
            "xtask: icount boot {boot}: digest {}, {} ms of virtual time in {host} ms of host time, \
             {} host s per virtual s at shift {shift}",
            probe.digest,
            probe.virtual_us / 1000,
            host_per_virtual(host, probe.virtual_us)
        );
        probes.push(probe);
    }
    compare(&probes)?;
    println!(
        "xtask: PASS: {boots} icount boots printed the same digest {}",
        probes[0].digest
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(String::from).collect()
    }

    const BOOT: &str = "timer: APIC 62500 counts/ms\n\
        icount-probe: seed 7; measure-start\n\
        icount-probe: measure-end\n\
        [shell] icount-probe: digest 0000000000000bad\n\
        icount-probe: aligned at tick 812 and HPET 812004000 ns (absolute, not digested)\n\
        icount-probe: compute 00000000000000aa (409600 iterations), sleepers 1 2 3\n\
        icount-probe: digest 1234abcd1234abcd\n\
        icount-probe: 1093 ticks in the window; 1093412 us of virtual time between the markers\n";

    #[test]
    fn the_measurement_line_is_single_cpu_icount_without_a_nic() {
        let firmware = crate::Firmware {
            code: PathBuf::from("code.fd"),
            vars: PathBuf::from("target/ovmf-vars.fd"),
        };
        let args = measure_args(&firmware, 3);
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(pair("-accel", "tcg,thread=single"));
        assert!(pair("-icount", "shift=3,align=off,sleep=off"));
        assert!(pair("-smp", "1"));
        assert!(pair("-nic", "none"));
        assert!(pair("-cpu", "max"));
        assert!(pair("-rtc", "base=2026-01-01T00:00:00,clock=vm"));
        assert!(
            !args
                .iter()
                .any(|a| a.contains("virtio-net") || a.contains("netdev"))
        );
        assert!(!args.iter().any(|a| a.contains("kvm") || a.contains("whpx")));
    }

    #[test]
    fn the_probe_lines_parse_and_program_text_is_ignored() {
        let probe = parse_probe(&lines(BOOT)).unwrap();
        assert_eq!(probe.seed, 7);
        assert_eq!(probe.digest, "1234abcd1234abcd");
        assert_eq!(probe.virtual_us, 1_093_412);
        assert!(probe.parts.starts_with("00000000000000aa"));
        assert!(parse_probe(&lines("boot-ok\n")).is_err());
    }

    #[test]
    fn identical_digests_pass_and_different_ones_show_both() {
        let probe = |digest: &str| Probe {
            seed: 1,
            digest: digest.into(),
            parts: format!("{digest}-parts"),
            virtual_us: 1,
        };
        assert!(compare(&[probe("a"), probe("a")]).is_ok());
        let error = compare(&[probe("a"), probe("b")]).unwrap_err();
        assert!(
            error.contains("boot 1: a") && error.contains("boot 2: b"),
            "{error}"
        );
        assert!(compare(&[]).is_err());
    }

    #[test]
    fn host_seconds_per_virtual_second_has_two_decimals() {
        assert_eq!(host_per_virtual(3400, 2_000_000), "1.70");
        assert_eq!(host_per_virtual(250, 1_000_000), "0.25");
        assert_eq!(host_per_virtual(1, 0), "n/a");
    }
}
