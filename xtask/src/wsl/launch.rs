//! `--accel wsl` for `run`, `test` and `soak`: QEMU with KVM inside a WSL2
//! distro, started from xtask on Windows.
//!
//! xtask still builds everything on Windows and fills `target/` (the ESP folder,
//! the fixtures, the OVMF variable store). Only QEMU runs in the distro, through
//! `wsl.exe -d <distro> --cd <target> -e <qemu> <args>`: `-e` hands each argument
//! over as it is, with no Linux shell to reinterpret it, and with `target/` (as
//! the distro sees it, junctions resolved) as the working directory, the relative
//! drive paths `qemu::args` writes hold unchanged. The serial console comes back
//! on wsl.exe's stdout: to the Windows terminal for `run`, to the harness for
//! `test`. `run`'s QEMU window appears through WSLg. Killing wsl.exe ends QEMU.
//!
//! A model disk of 64 MiB or more is copied once into the distro's own ext4 disk
//! (`~/.cache/kai-models`) and attached from there: a boot reads the whole model,
//! and reading it through `/mnt` (WSL's 9P share of Windows drives) costs that
//! transfer on every boot. The copy is kept while its size and modification time
//! match the original. Smaller files, such as the test fixtures, are read in place.

use super::check::{self, Item, Needs, Place};
use super::path::{argv, base_path, check_memory, should_cache, to_wsl, translate_args};
use super::probe::{self, Report, START_LIMIT};
use crate::Firmware;
use crate::options::{Options, Task};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Copying a 2.5 GB model through 9P can take minutes.
const CACHE_LIMIT: Duration = Duration::from_secs(1800);
/// Extra time a test boot's `timeout` in WSL allows beyond xtask's own limit.
const TIMEOUT_GRACE_SECS: u64 = 60;

/// Copies `$1` into `~/.cache/kai-models` unless a copy of the same size and
/// time is there, and prints the copy's path. The distro's disk is a VHDX that
/// grows on the Windows drive mounted at `$2`, so that drive needs room for the
/// file plus 1 GiB. A copy cut short leaves nothing behind.
const CACHE_SCRIPT: &str = r#"src="$1"; dir="$HOME/.cache/kai-models"; dst="$dir/$(basename "$src")"
mkdir -p "$dir" || exit 1
if [ -f "$dst" ] && [ "$(stat -c '%s %Y' "$src")" = "$(stat -c '%s %Y' "$dst")" ]; then echo "$dst"; exit 0; fi
need=$(( $(stat -c %s "$src") / 1024 + 1048576 ))
free=$(df -Pk "$2" | awk 'NR==2 {print $4}')
if [ "$free" -lt "$need" ]; then echo "only $free KiB free on the drive under $2" >&2; exit 3; fi
tmp="$dst.part.$$"; trap 'rm -f "$tmp"' EXIT
cp -p "$src" "$tmp" && mv -f "$tmp" "$dst" && echo "$dst""#;

pub struct Wsl {
    exe: PathBuf,
    distro: String,
    report: Report,
    /// `target/` as the distro sees it.
    target: String,
    wslconfig: String,
    /// For `test` and `soak`, the longest a boot may take, in seconds.
    limit: Option<u64>,
}

impl Wsl {
    /// The distro for `--accel wsl`, or None without it. Refuses, with the
    /// doctor's fixes, when the boot would need something the distro lacks.
    pub fn for_options(options: &Options, target_dir: &Path) -> Result<Option<Self>, String> {
        if !options.wsl {
            return Ok(None);
        }
        let needs = Needs {
            window: options.task == Task::Run,
            loopback: options.task != Task::Run || options.edit,
        };
        // A boot xtask reads ends by itself within the longest case's time limit.
        let cases = match options.task {
            Task::Soak => crate::cases::SOAK_CASES,
            _ => crate::cases::TEST_CASES,
        };
        let longest = cases
            .iter()
            .filter_map(|case| case.min_timeout)
            .fold(options.timeout, Duration::max);
        let limit = (options.task != Task::Run).then(|| longest.as_secs() + TIMEOUT_GRACE_SECS);
        let mut wsl = Self::prepare(target_dir, needs)?;
        wsl.limit = limit;
        Ok(Some(wsl))
    }

    fn prepare(target_dir: &Path, needs: Needs) -> Result<Self, String> {
        if !cfg!(windows) {
            return Err(
                "--accel wsl starts QEMU in WSL from Windows; on Linux use --accel kvm".into(),
            );
        }
        let exe = probe::exe().ok_or("--accel wsl needs WSL, which is not installed")?;
        let found = probe::find()
            .map_err(|e| format!("--accel wsl needs a WSL2 distro; WSL: {}", e.describe()))?;
        let distro = found.name;
        if found.version != 2 {
            return Err(format!(
                "{distro} runs as WSL 1, which has no KVM; convert it: wsl --set-version {distro} 2"
            ));
        }
        println!("xtask: checking {distro} in WSL for KVM, QEMU and OVMF");
        let (report, _) = probe::ask(&distro)?;
        let config = check::read_wslconfig();
        let place = Place {
            distro: Some(&distro),
            config: &config,
        };
        let nested = check::nested(&config, report.virt_flags);
        let blockers: Vec<Item> = std::iter::once(nested)
            .chain(check::linux(&report, place, needs))
            .filter(|item| !item.ok)
            .collect();
        if !blockers.is_empty() {
            check::print_items(&blockers);
            return Err(format!(
                "{distro} in WSL cannot boot Kernel AI yet; the fixes are above, and \
                 cargo xtask doctor shows the whole checklist"
            ));
        }
        // A junction (target -> D:\...) is followed, so the distro reads the real drive.
        let real = target_dir
            .canonicalize()
            .map_err(|e| format!("cannot resolve {}: {e}", target_dir.display()))?;
        let target = to_wsl(&real.to_string_lossy(), &report.mount_root, &distro)?;
        Ok(Self {
            exe,
            distro,
            report,
            target,
            wslconfig: config.path,
            limit: None,
        })
    }

    /// WSL's own wsl.exe, by its full path, so never System32's install stub.
    pub fn program(&self) -> PathBuf {
        self.exe.clone()
    }

    /// The distro's OVMF, with a fresh copy of its variable store made in
    /// `target/` by `cp` inside the distro.
    fn firmware(&self, target_dir: &Path) -> Result<Firmware, String> {
        let (code, vars) = self.report.ovmf.clone().ok_or("no OVMF in WSL")?;
        let out = probe::run(
            probe::wsl().args([
                "-d",
                &self.distro,
                "--cd",
                &self.target,
                "-e",
                "cp",
                "-f",
                &vars,
                "ovmf-vars.fd",
            ]),
            // WSL may have stopped while xtask built, and its start takes over a minute.
            START_LIMIT,
        )?;
        if !out.success {
            return Err(format!(
                "cannot copy {vars} into {}: {}",
                self.target,
                out.stderr.trim()
            ));
        }
        Ok(Firmware {
            code: PathBuf::from(code),
            vars: target_dir.join("ovmf-vars.fd"),
        })
    }

    /// `wsl.exe`'s arguments for running QEMU with `args` in the distro.
    fn wrap(&self, args: Vec<String>) -> Result<Vec<String>, String> {
        check_memory(&args, self.report.mem_kib, &self.wslconfig)?;
        let args = translate_args(args, |windows| self.disk_path(windows))?;
        let qemu = self.report.qemu.as_deref().ok_or("no QEMU in WSL")?;
        Ok(argv(&self.distro, &self.target, qemu, self.limit, args))
    }

    /// A model disk as the distro reads it: a big file on a Windows drive is
    /// copied into the distro once; if that fails, it is read through `/mnt`.
    fn disk_path(&self, windows: &str) -> Result<String, String> {
        // Junctions are followed here too: fixtures live under target/.
        let real = Path::new(windows).canonicalize().map_or_else(
            |_| windows.to_string(),
            |p| p.to_string_lossy().into_owned(),
        );
        let linux = to_wsl(&real, &self.report.mount_root, &self.distro)?;
        let len = std::fs::metadata(windows).map_or(0, |m| m.len());
        if !should_cache(&real, len) {
            return Ok(linux);
        }
        let Some(drive) = self.vhdx_drive() else {
            println!("xtask: reading {linux} through /mnt: the distro's disk was not found");
            return Ok(linux);
        };
        println!(
            "xtask: copying {linux} ({} MB) into {}'s own disk, ~/.cache/kai-models, unless \
             it is there already, so boots read it at full speed rather than through /mnt; \
             the copy takes that much space on {drive}",
            len / 1_000_000,
            self.distro
        );
        let mount = to_wsl(&format!("{drive}\\"), &self.report.mount_root, &self.distro)?;
        let script = [
            "-d",
            &self.distro,
            "-e",
            "sh",
            "-c",
            CACHE_SCRIPT,
            "sh",
            &linux,
            &mount,
        ];
        let out = probe::run(probe::wsl().args(script), CACHE_LIMIT);
        match out {
            Ok(o) if o.success => match o.stdout.lines().last().map(str::trim) {
                Some(path) if path.starts_with('/') => Ok(path.to_string()),
                _ => Ok(linux),
            },
            Ok(o) => {
                println!(
                    "xtask: not copied ({}); reading {linux} through /mnt",
                    o.stderr.trim()
                );
                Ok(linux)
            }
            Err(e) => {
                println!("xtask: not copied ({e}); reading {linux} through /mnt");
                Ok(linux)
            }
        }
    }

    /// The Windows drive (`D:`) holding the distro's virtual disk, which grows
    /// with every file written in it. Read from the registry, which needs no
    /// administrator and changes nothing.
    fn vhdx_drive(&self) -> Option<String> {
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss";
        let out = probe::run(
            Command::new("reg")
                .args(["query", key, "/s"])
                .stderr(Stdio::null()),
            probe::SHORT_LIMIT,
        )
        .ok()?;
        let base = base_path(&out.stdout, &self.distro)?;
        let plain = base.strip_prefix(r"\\?\").unwrap_or(&base);
        let drive = plain.get(..2)?;
        (drive.as_bytes()[0].is_ascii_alphabetic() && drive.ends_with(':'))
            .then(|| drive.to_string())
    }
}

/// The program to run a wrapped QEMU command line: for `--accel wsl` boots whose
/// console xtask reads, wsl.exe gets a hidden console of its own, so it cannot
/// switch the user's terminal into raw mode; QEMU's errors come on stdout.
pub fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    if Some(program) == probe::exe().as_deref() {
        probe::hide_console(&mut command);
        command.stdin(Stdio::null()).stderr(Stdio::null());
    }
    command
}

/// The firmware QEMU boots: the distro's with `--accel wsl`, else the host's.
pub fn firmware(wsl: Option<&Wsl>, qemu: &Path, target_dir: &Path) -> Result<Firmware, String> {
    match wsl {
        Some(wsl) => wsl.firmware(target_dir),
        None => crate::find_firmware(qemu, target_dir),
    }
}

/// QEMU's arguments as the program `qemu` from `program()` takes them: unchanged,
/// or behind `wsl.exe -d <distro> --cd <target> -e <qemu>` with `--accel wsl`.
pub fn wrap(wsl: Option<&Wsl>, args: Vec<String>) -> Result<Vec<String>, String> {
    match wsl {
        Some(wsl) => wsl.wrap(args),
        None => Ok(args),
    }
}
