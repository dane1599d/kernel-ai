//! `cargo xtask vbox --powercut [--name NAME]`: S04-3's VirtualBox half (P2
//! task 9, ADR 0004). The VM boots `filecheck step=churn` on its `kai-data.vdi`
//! 21 times, and VirtualBox powers it off hard 20 times mid-write. Each boot's
//! filecheck checks every file before it writes, numbering the boot from the
//! marks it left, so the command line, and the boot image, stay the same.
//!
//! A boot passes on `filecheck: boot N: ... files whole after the cut`; a
//! `FAIL` or `FATAL` line, a wrong boot number or no line in time fails the run.
//! A VM power-off is not a host power cut: what VirtualBox handed the host's
//! cache before it stopped still reaches the disk (QEMU's replay in `fs-actions`
//! stays the proof of every flush point).

use super::manage::{self, step, vm};
use super::{SERIAL_LOG, path_str};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

/// The power-offs; one boot more checks the last.
pub const CUTS: u32 = 20;
/// The kernel command line: the churn, with filecheck numbering the boots.
pub const CMDLINE: &str = "test=files-churn";
/// How long one boot may take to reach its line.
const BOOT_TIMEOUT: Duration = Duration::from_secs(300);
/// How long the VM may take to stop after a power-off.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(200);

/// Boots the VM `CUTS + 1` times, powering it off after each line but the last.
pub fn run(dir: &Path) -> Result<(), String> {
    let serial = dir.join(SERIAL_LOG);
    for boot in 1..=CUTS + 1 {
        stopped()?;
        // A new log each boot, so a line from the boot before cannot pass this one.
        let _ = fs::remove_file(&serial);
        step(&["startvm", vm(), "--type", "headless"])?;
        let line = wait_line(&serial, boot);
        let delay = crate::cut_delay("vbox-powercut", boot);
        if line.is_ok() && boot <= CUTS {
            // The churn is writing: cut it somewhere in its next actions.
            std::thread::sleep(delay);
        }
        step(&["controlvm", vm(), "poweroff"])?;
        let line = line?;
        match boot <= CUTS {
            true => println!("xtask: boot {boot}: {line}; powered off {delay:?} later"),
            false => println!("xtask: boot {boot}: {line}"),
        }
    }
    stopped()?;
    println!("xtask: vbox powercut: PASS, {CUTS} hard power-offs, every file whole after each");
    Ok(())
}

/// Waits for boot `boot`'s verdict in the serial log.
fn wait_line(serial: &Path, boot: u32) -> Result<String, String> {
    let start = Instant::now();
    while start.elapsed() < BOOT_TIMEOUT {
        let text = fs::read(serial).unwrap_or_default();
        if let Some(verdict) = verdict(&String::from_utf8_lossy(&text), boot) {
            return verdict;
        }
        std::thread::sleep(POLL);
    }
    Err(format!(
        "boot {boot}: no filecheck line in {BOOT_TIMEOUT:?} (see {})",
        serial.display()
    ))
}

/// Boot `boot`'s verdict in `log` so far: its whole-files line, or the failure.
fn verdict(log: &str, boot: u32) -> Option<Result<String, String>> {
    for line in log.lines() {
        let line = line.trim();
        if line.contains("FAIL") || line.contains("FATAL") {
            return Some(Err(format!("boot {boot}: {line}")));
        }
        let Some(rest) = line
            .split_once("filecheck: boot ")
            .map(|(_, rest)| rest)
            .filter(|rest| rest.contains("files whole after the cut"))
        else {
            continue;
        };
        let number = rest.split(':').next().and_then(|n| n.parse::<u32>().ok());
        return Some(match number {
            Some(n) if n == boot => Ok(format!("boot {rest}")),
            _ => Err(format!(
                "boot {boot}: filecheck counted another boot: {line}"
            )),
        });
    }
    None
}

/// Waits until the VM is off.
fn stopped() -> Result<(), String> {
    let start = Instant::now();
    loop {
        let state = manage::vm_info()
            .and_then(|info| info.get("VMState").cloned())
            .unwrap_or_default();
        if state == "poweroff" || state == "aborted" {
            return Ok(());
        }
        if start.elapsed() > STOP_TIMEOUT {
            return Err(format!("the VM is still {state:?}"));
        }
        std::thread::sleep(POLL);
    }
}

/// Makes `kai-data.vdi` at `vdi` if it is not there: the GPT the CI data disk
/// has (`blklog::make_disk`), a 64 MiB `kai-data` and a `kai-cache`, blank, so
/// `fs` formats it on first boot. An existing one is kept, data and all.
pub fn data_disk(vdi: &Path) -> Result<(), String> {
    if vdi.exists() {
        println!("xtask: keeping {}", vdi.display());
        return Ok(());
    }
    let raw = vdi.with_extension("img");
    crate::blklog::make_disk(&raw, Some(crate::blklog::CACHE_MIB))?;
    let made = step(&[
        "convertfromraw",
        path_str(&raw)?,
        path_str(vdi)?,
        "--format",
        "VDI",
    ]);
    let _ = fs::remove_file(&raw);
    made.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::verdict;

    #[test]
    fn a_boot_passes_on_its_own_line_only() {
        let log = "  | [filecheck] filecheck: boot 3: 120 files whole after the cut\n";
        assert_eq!(
            verdict(log, 3),
            Some(Ok("boot 3: 120 files whole after the cut".into()))
        );
        assert!(matches!(verdict(log, 4), Some(Err(_))));
        assert_eq!(verdict("booting\n", 1), None);
    }

    #[test]
    fn a_failure_line_fails_the_boot() {
        let log = "filecheck: FAIL f12 is torn: 10 bytes\n";
        assert!(matches!(verdict(log, 1), Some(Err(e)) if e.contains("torn")));
        assert!(matches!(verdict("FATAL on cpu 0: x\n", 2), Some(Err(_))));
    }
}
