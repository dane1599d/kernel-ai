//! Build and run tool for Kernel AI.
//!
//! cargo xtask build [--release] [--spikes]
//! cargo xtask run   [--release] [--spikes] [--accel auto|kvm|whpx|tcg]
//! cargo xtask test  [--release] [--accel auto|kvm|whpx|tcg] [--timeout SECS]
//!
//! `test` boots the kernel headless once per test case (boot checks, page fault,
//! double fault) with the spikes enabled. Each case passes only if the
//! kernel reports success through QEMU's isa-debug-exit device.

mod lines;

use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{env, fs, thread};

const LIMINE_VERSION: &str = "12.9.3";
const LIMINE_SHA256: &str = "9f42fe9ea2e84d71056529969b1d5c24ba9a2287ffcc01bcce58b0ef8aa98b1d";
const KERNEL_TARGET: &str = "x86_64-unknown-none";
/// QEMU exit status for `isa-debug-exit` value 0x10: (0x10 << 1) | 1.
const QEMU_PASS: i32 = 33;
const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Task {
    Build,
    Run,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accel {
    Auto,
    Kvm,
    Whpx,
    Tcg,
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    task: Task,
    release: bool,
    spikes: bool,
    accel: Accel,
    timeout: Duration,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = parse_args(&args).and_then(|options| execute(&options));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let task = match args.first().map(String::as_str) {
        Some("build") => Task::Build,
        Some("run") => Task::Run,
        Some("test") => Task::Test,
        other => return Err(format!("expected build, run or test, got {other:?}")),
    };
    let mut options = Options {
        task,
        release: false,
        spikes: task == Task::Test,
        accel: Accel::Auto,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
    };
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--release" => options.release = true,
            "--spikes" => options.spikes = true,
            "--accel" => {
                options.accel = match rest.next().map(String::as_str) {
                    Some("auto") => Accel::Auto,
                    Some("kvm") => Accel::Kvm,
                    Some("whpx") => Accel::Whpx,
                    Some("tcg") => Accel::Tcg,
                    other => {
                        return Err(format!("--accel expects auto|kvm|whpx|tcg, got {other:?}"));
                    }
                }
            }
            "--timeout" => {
                let secs = rest
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or("--timeout expects a number of seconds")?;
                options.timeout = Duration::from_secs(secs);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

fn execute(options: &Options) -> Result<(), String> {
    let root = workspace_root();
    let kernel = build_kernel(&root, options)?;
    if options.task == Task::Build {
        println!("xtask: kernel built at {}", kernel.display());
        return Ok(());
    }
    let target_dir = root.join("target");
    let line_table = lines::build(&kernel, &root)?;
    let efi = ensure_limine(&target_dir)?;
    let esp = target_dir.join("esp");
    let qemu = find_qemu()?;
    let accel = resolve_accel(options.accel);
    if options.task == Task::Run {
        make_esp(&esp, &kernel, &line_table, &efi, "")?;
        let firmware = find_firmware(&qemu, &target_dir)?;
        let args = qemu_args(&firmware, accel, false);
        println!("xtask: {} {}", qemu.display(), args.join(" "));
        return run_interactive(&qemu, &args, &target_dir);
    }
    let mut failures = Vec::new();
    for (name, cmdline) in TEST_CASES {
        println!("xtask: test case {name} ({cmdline})");
        make_esp(&esp, &kernel, &line_table, &efi, cmdline)?;
        let firmware = find_firmware(&qemu, &target_dir)?;
        let args = qemu_args(&firmware, accel, true);
        if let Err(message) = run_test(&qemu, &args, &target_dir, accel, options.timeout) {
            eprintln!("xtask: test case {name}: {message}");
            failures.push(name);
        }
    }
    if failures.is_empty() {
        println!("xtask: all {} test cases passed", TEST_CASES.len());
        Ok(())
    } else {
        Err(format!("failed test cases: {}", failures.join(", ")))
    }
}

/// Each test case boots the kernel once with this command line. The kernel itself
/// decides pass or fail and reports it through isa-debug-exit.
const TEST_CASES: [(&str, &str); 3] = [
    ("boot", "test"),
    ("page-fault", "test=page-fault"),
    ("double-fault", "test=double-fault"),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

fn build_kernel(root: &Path, options: &Options) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args(["build", "-p", "kernel", "--target", KERNEL_TARGET]);
    if options.release {
        command.arg("--release");
    }
    if options.spikes {
        command.args(["--features", "spikes"]);
    }
    let status = command
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err("kernel build failed".into());
    }
    let profile = if options.release { "release" } else { "debug" };
    Ok(root
        .join("target")
        .join(KERNEL_TARGET)
        .join(profile)
        .join("kernel"))
}

/// Downloads Limine's prebuilt binaries once, verifies the pinned checksum, and returns BOOTX64.EFI.
fn ensure_limine(target_dir: &Path) -> Result<PathBuf, String> {
    let dir = target_dir.join(format!("limine-{LIMINE_VERSION}"));
    let efi = dir.join("limine-binary").join("BOOTX64.EFI");
    if efi.exists() {
        return Ok(efi);
    }
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let archive = "limine-binary.tar.gz";
    let url = format!(
        "https://github.com/Limine-Bootloader/Limine/releases/download/v{LIMINE_VERSION}/{archive}"
    );
    println!("xtask: downloading {url}");
    run_checked(
        Command::new("curl")
            .current_dir(&dir)
            .args(["-sSLf", "-o", archive, &url]),
    )?;
    let bytes = fs::read(dir.join(archive)).map_err(|e| format!("cannot read {archive}: {e}"))?;
    let actual = sha256_hex(&bytes);
    if actual != LIMINE_SHA256 {
        let _ = fs::remove_file(dir.join(archive));
        return Err(format!(
            "Limine checksum mismatch: expected {LIMINE_SHA256}, got {actual}"
        ));
    }
    run_checked(
        Command::new(tar_program())
            .current_dir(&dir)
            .args(["-xzf", archive]),
    )?;
    if efi.exists() {
        Ok(efi)
    } else {
        Err(format!("{} missing after extraction", efi.display()))
    }
}

/// On Windows, use the system bsdtar; Git's GNU tar misreads drive-letter paths.
fn tar_program() -> &'static str {
    if cfg!(windows) {
        "C:\\Windows\\System32\\tar.exe"
    } else {
        "tar"
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn limine_conf(cmdline: &str) -> String {
    format!(
        "timeout: 0\n\n/Kernel AI\n    protocol: limine\n    path: boot():/boot/kernel\n    cmdline: {cmdline}\n    module_path: boot():/boot/kernel.lines\n"
    )
}

/// Lays out the EFI system partition that QEMU exposes as a virtual FAT drive.
fn make_esp(
    esp: &Path,
    kernel: &Path,
    line_table: &[u8],
    efi: &Path,
    cmdline: &str,
) -> Result<(), String> {
    if esp.exists() {
        fs::remove_dir_all(esp).map_err(|e| format!("cannot clear {}: {e}", esp.display()))?;
    }
    let boot_efi = esp.join("EFI").join("BOOT");
    let limine_dir = esp.join("boot").join("limine");
    for dir in [&boot_efi, &limine_dir] {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    copy(efi, &boot_efi.join("BOOTX64.EFI"))?;
    copy(kernel, &esp.join("boot").join("kernel"))?;
    fs::write(esp.join("boot").join("kernel.lines"), line_table)
        .map_err(|e| format!("cannot write kernel.lines: {e}"))?;
    fs::write(limine_dir.join("limine.conf"), limine_conf(cmdline))
        .map_err(|e| format!("cannot write limine.conf: {e}"))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))
}

fn find_qemu() -> Result<PathBuf, String> {
    if let Ok(path) = env::var("QEMU") {
        return Ok(PathBuf::from(path));
    }
    let on_path = Command::new("qemu-system-x86_64")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if on_path {
        return Ok(PathBuf::from("qemu-system-x86_64"));
    }
    let windows_default = PathBuf::from("C:\\Program Files\\qemu\\qemu-system-x86_64.exe");
    if windows_default.exists() {
        return Ok(windows_default);
    }
    Err("QEMU not found: install qemu-system-x86_64 or set QEMU to its path".into())
}

/// UEFI firmware: a read-only code image plus a fresh writable copy of the variable store.
struct Firmware {
    code: PathBuf,
    vars: PathBuf,
}

fn find_firmware(qemu: &Path, target_dir: &Path) -> Result<Firmware, String> {
    let qemu_share = qemu
        .parent()
        .map(|dir| dir.join("share"))
        .unwrap_or_default();
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();
    if let (Ok(code), Ok(vars)) = (env::var("OVMF_CODE"), env::var("OVMF_VARS")) {
        candidates.push((code.into(), vars.into()));
    }
    for (code, vars) in [
        (
            "/usr/share/OVMF/OVMF_CODE_4M.fd",
            "/usr/share/OVMF/OVMF_VARS_4M.fd",
        ),
        (
            "/usr/share/OVMF/OVMF_CODE.fd",
            "/usr/share/OVMF/OVMF_VARS.fd",
        ),
        (
            "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
            "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
        ),
        (
            "/usr/share/edk2/ovmf/OVMF_CODE.fd",
            "/usr/share/edk2/ovmf/OVMF_VARS.fd",
        ),
    ] {
        candidates.push((code.into(), vars.into()));
    }
    candidates.push((
        qemu_share.join("edk2-x86_64-code.fd"),
        qemu_share.join("edk2-i386-vars.fd"),
    ));
    let (code, vars_template) = candidates
        .into_iter()
        .find(|(code, vars)| code.exists() && vars.exists())
        .ok_or("UEFI firmware not found: install OVMF or set OVMF_CODE and OVMF_VARS")?;
    let vars = target_dir.join("ovmf-vars.fd");
    copy(&vars_template, &vars)?;
    Ok(Firmware { code, vars })
}

fn resolve_accel(requested: Accel) -> Accel {
    if requested != Accel::Auto {
        return requested;
    }
    let kvm_usable = cfg!(target_os = "linux")
        && fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .is_ok();
    if kvm_usable { Accel::Kvm } else { Accel::Tcg }
}

/// QEMU runs with `target/` as its working directory, so drive paths stay relative
/// and free of drive-letter colons, which QEMU's option parser would misread.
fn qemu_args(firmware: &Firmware, accel: Accel, headless: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-machine",
        "q35",
        "-m",
        "512M",
        "-smp",
        "1",
        "-no-reboot",
        "-serial",
        "stdio",
        "-device",
        "isa-debug-exit,iobase=0xf4,iosize=0x04",
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
    debug_assert!(firmware.vars.ends_with("ovmf-vars.fd"));
    args.push("-drive".into());
    args.push("format=raw,file=fat:rw:esp".into());
    let (accel_name, cpu) = match accel {
        Accel::Kvm => ("kvm", "host"),
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

fn run_interactive(qemu: &Path, args: &[String], target_dir: &Path) -> Result<(), String> {
    let status = Command::new(qemu)
        .current_dir(target_dir)
        .args(args)
        .status()
        .map_err(|e| format!("cannot start QEMU: {e}"))?;
    println!("xtask: QEMU exited with {status}");
    Ok(())
}

fn run_test(
    qemu: &Path,
    args: &[String],
    target_dir: &Path,
    accel: Accel,
    timeout: Duration,
) -> Result<(), String> {
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
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            println!("  | {line}");
            if line.contains("boot-ok") {
                let _ = ready_tx.send(started.elapsed());
            }
        }
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
    let _ = reader.join();
    if let Ok(elapsed) = ready_rx.try_recv() {
        println!(
            "xtask: boot-to-ready {} ms (accel {:?}, includes firmware)",
            elapsed.as_millis(),
            accel
        );
    }
    if status.code() == Some(QEMU_PASS) {
        println!("xtask: PASS");
        Ok(())
    } else {
        Err(format!("FAIL: QEMU exited with {status}"))
    }
}

fn run_checked(command: &mut Command) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|e| format!("cannot run {command:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command:?} failed with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_task_enables_spikes_by_default() {
        let options = parse_args(&args(&["test"])).unwrap();
        assert_eq!(options.task, Task::Test);
        assert!(options.spikes);
        assert_eq!(options.accel, Accel::Auto);
    }

    #[test]
    fn run_task_parses_flags() {
        let options = parse_args(&args(&[
            "run",
            "--release",
            "--accel",
            "tcg",
            "--timeout",
            "5",
        ]))
        .unwrap();
        assert!(options.release);
        assert!(!options.spikes);
        assert_eq!(options.accel, Accel::Tcg);
        assert_eq!(options.timeout, Duration::from_secs(5));
    }

    #[test]
    fn rejects_unknown_task_and_options() {
        assert!(parse_args(&args(&["deploy"])).is_err());
        assert!(parse_args(&args(&["run", "--fast"])).is_err());
        assert!(parse_args(&args(&["run", "--accel", "hyperv"])).is_err());
    }

    #[test]
    fn limine_conf_carries_cmdline() {
        let conf = limine_conf("test");
        assert!(conf.contains("protocol: limine"));
        assert!(conf.contains("path: boot():/boot/kernel"));
        assert!(conf.contains("cmdline: test"));
        assert!(conf.contains("module_path: boot():/boot/kernel.lines"));
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn qemu_args_use_relative_drives_and_debug_exit() {
        let firmware = Firmware {
            code: PathBuf::from("code.fd"),
            vars: PathBuf::from("target/ovmf-vars.fd"),
        };
        let list = qemu_args(&firmware, Accel::Tcg, true);
        assert!(list.contains(&"format=raw,file=fat:rw:esp".to_string()));
        assert!(list.contains(&"isa-debug-exit,iobase=0xf4,iosize=0x04".to_string()));
        assert!(list.windows(2).any(|w| w[0] == "-accel" && w[1] == "tcg"));
        assert!(
            list.windows(2)
                .any(|w| w[0] == "-display" && w[1] == "none")
        );
    }
}
