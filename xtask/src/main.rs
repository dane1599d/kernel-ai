//! Build and run tool for Kernel AI.
//!
//! cargo xtask build [--release] [--spikes]
//! cargo xtask run   [--release] [--spikes] [--accel auto|kvm|whpx|tcg] [--llm IP:PORT] [--model NAME]
//! cargo xtask test  [--release] [--accel auto|kvm|whpx|tcg] [--timeout SECS]
//! cargo xtask soak  [--accel auto|kvm|whpx|tcg]   (10-minute storm, M5 exit test)
//! cargo xtask fluency --llm HOST:PORT --model NAME [--repeat N]   (M8b script gate, host only)
//!
//! `test` boots the kernel headless once per test case (boot and memory checks,
//! page fault, double fault, write-protect, stack overflow, 60 s timer, keyboard,
//! scheduling latency under load, policy watchdog; keys are
//! typed through the QEMU monitor) with the spikes
//! enabled. Each case passes only if the
//! kernel reports success through QEMU's isa-debug-exit device.

mod cases;
mod fluency;
mod lines;
mod mock_llm;
mod qemu;
mod strip;

use sha2::{Digest, Sha256};

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use std::time::Duration;
use std::{env, fs};

const LIMINE_VERSION: &str = "12.9.3";
const LIMINE_SHA256: &str = "9f42fe9ea2e84d71056529969b1d5c24ba9a2287ffcc01bcce58b0ef8aa98b1d";
const KERNEL_TARGET: &str = "x86_64-unknown-none";
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const SOAK_TIMEOUT_SECS: u64 = 900;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Task {
    Build,
    Run,
    Test,
    /// Long-running cases (the 10-minute storm), run on demand rather than on every PR.
    Soak,
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
    /// LLM server as HOST:PORT for `run`, for example the Mac mini on the LAN.
    llm: Option<String>,
    model: Option<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = if args.first().map(String::as_str) == Some("fluency") {
        fluency::main(&args[1..], &workspace_root().join("target"))
    } else {
        parse_args(&args).and_then(|options| execute(&options))
    };
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
        Some("soak") => Task::Soak,
        other => {
            return Err(format!(
                "expected build, run, test, soak or fluency, got {other:?}"
            ));
        }
    };
    let mut options = Options {
        task,
        release: false,
        spikes: task == Task::Test,
        accel: Accel::Auto,
        timeout: Duration::from_secs(if task == Task::Soak {
            SOAK_TIMEOUT_SECS
        } else {
            DEFAULT_TIMEOUT_SECS
        }),
        llm: None,
        model: None,
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
            "--llm" => {
                let server = rest.next().ok_or("--llm expects HOST:PORT")?;
                if !valid_host_port(server) {
                    return Err(format!(
                        "--llm expects HOST:PORT with an IPv4 host, got {server:?}"
                    ));
                }
                options.llm = Some(server.clone());
            }
            "--model" => {
                let model = rest.next().ok_or("--model expects a model name")?;
                if model.is_empty() || model.chars().any(char::is_whitespace) {
                    return Err("--model expects a name without spaces".into());
                }
                options.model = Some(model.clone());
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

fn execute(options: &Options) -> Result<(), String> {
    let root = workspace_root();
    let kernel = build_kernel(&root, options)?;
    let programs = build_user(&root, options)?;
    if options.task == Task::Build {
        println!("xtask: kernel built at {}", kernel.display());
        println!("xtask: {} user programs built", programs.len());
        return Ok(());
    }
    let target_dir = root.join("target");
    let modules = boot_modules(&kernel, &programs, &root)?;
    let efi = ensure_limine(&target_dir)?;
    let esp = target_dir.join("esp");
    let qemu = find_qemu()?;
    let accel = resolve_accel(options.accel);
    if options.task == Task::Run {
        let cmdline = llm_cmdline(options.llm.as_deref(), options.model.as_deref());
        make_esp(&esp, &kernel, &modules, &efi, &cmdline)?;
        let firmware = find_firmware(&qemu, &target_dir)?;
        let args = qemu::args(&firmware, accel, false);
        println!("xtask: {} {}", qemu.display(), args.join(" "));
        return qemu::run_interactive(&qemu, &args, &target_dir);
    }
    // Tests talk to a local stand-in for the LLM server, reached from the guest at
    // 10.0.2.2 through QEMU's user-mode network.
    let mock = mock_llm::start()?;
    let llm = format!("10.0.2.2:{}", mock.port);
    let llm_args = llm_cmdline(Some(&llm), Some(mock_llm::MODEL));
    let mut failures = Vec::new();
    let cases = if options.task == Task::Soak {
        cases::SOAK_CASES
    } else {
        cases::TEST_CASES
    };
    for case in cases {
        let cmdline = format!("{} {llm_args}", case.cmdline);
        println!("xtask: test case {} ({cmdline})", case.name);
        make_esp(&esp, &kernel, &modules, &efi, &cmdline)?;
        let firmware = find_firmware(&qemu, &target_dir)?;
        let monitor = qemu::free_port()?;
        let mut args = qemu::args(&firmware, accel, true);
        args.push("-monitor".into());
        args.push(format!("tcp:127.0.0.1:{monitor},server=on,wait=off"));
        let run = qemu::TestRun {
            case,
            accel,
            timeout: options.timeout,
            monitor,
        };
        if let Err(message) = qemu::run_test(&qemu, &args, &target_dir, &run) {
            eprintln!("xtask: test case {}: {message}", case.name);
            failures.push(case.name);
        }
    }
    println!(
        "xtask: the mock LLM server answered {} chat requests",
        mock.chats()
    );
    if failures.is_empty() {
        println!("xtask: all {} test cases passed", cases.len());
        Ok(())
    } else {
        Err(format!("failed test cases: {}", failures.join(", ")))
    }
}

/// Kernel command-line arguments that point the kernel at an LLM server.
fn llm_cmdline(llm: Option<&str>, model: Option<&str>) -> String {
    let mut args = Vec::new();
    if let Some(llm) = llm {
        args.push(format!("llm={llm}"));
    }
    if let Some(model) = model {
        args.push(format!("model={model}"));
    }
    args.join(" ")
}

/// IPv4 address and port, for example 192.168.1.50:8080. The kernel has no DNS.
fn valid_host_port(text: &str) -> bool {
    let Some((host, port)) = text.rsplit_once(':') else {
        return false;
    };
    host.parse::<std::net::Ipv4Addr>().is_ok() && port.parse::<u16>().is_ok_and(|p| p > 0)
}

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

/// The ring-3 programs in the `user` crate, all shipped as boot modules.
const USER_PROGRAMS: &[&str] = &[
    "supervisor",
    "llm",
    "shell",
    "crasher",
    "spinner",
    "hog",
    "probe",
];

/// Builds every user program; returns (name, ELF path) pairs.
fn build_user(root: &Path, options: &Options) -> Result<Vec<(&'static str, PathBuf)>, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args(["build", "-p", "user", "--bins", "--target", KERNEL_TARGET]);
    if options.release {
        command.arg("--release");
    }
    let status = command
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err("user program build failed".into());
    }
    let profile = if options.release { "release" } else { "debug" };
    let dir = root.join("target").join(KERNEL_TARGET).join(profile);
    Ok(USER_PROGRAMS
        .iter()
        .map(|&name| (name, dir.join(name)))
        .collect())
}

/// A file Limine loads next to the kernel, at `boot/<path>` on the ESP.
struct Module {
    path: String,
    bytes: Vec<u8>,
}

/// The kernel's line table, then each user program and its line table.
fn boot_modules(
    kernel: &Path,
    programs: &[(&'static str, PathBuf)],
    root: &Path,
) -> Result<Vec<Module>, String> {
    let mut modules = vec![Module {
        path: "kernel.lines".into(),
        bytes: lines::build(kernel, root, lines::Scope::All)?,
    }];
    for (name, elf) in programs {
        let full = fs::read(elf).map_err(|e| format!("cannot read {}: {e}", elf.display()))?;
        let bytes = strip::for_boot(&full).map_err(|e| format!("{name}: {e}"))?;
        println!(
            "xtask: {name} {} KiB on the boot drive ({} KiB with debug info)",
            bytes.len() / 1024,
            full.len() / 1024
        );
        modules.push(Module {
            path: format!("user/{name}.elf"),
            bytes,
        });
        modules.push(Module {
            path: format!("user/{name}.lines"),
            bytes: lines::build(elf, root, lines::Scope::Workspace)?,
        });
    }
    Ok(modules)
}

fn limine_conf(cmdline: &str, module_paths: &[&str]) -> String {
    let mut conf = format!(
        "timeout: 0\n\n/Kernel AI\n    protocol: limine\n    path: boot():/boot/kernel\n    cmdline: {cmdline}\n"
    );
    for path in module_paths {
        conf.push_str(&format!("    module_path: boot():/boot/{path}\n"));
    }
    conf
}

/// Lays out the EFI system partition that QEMU exposes as a virtual FAT drive.
fn make_esp(
    esp: &Path,
    kernel: &Path,
    modules: &[Module],
    efi: &Path,
    cmdline: &str,
) -> Result<(), String> {
    if esp.exists() {
        fs::remove_dir_all(esp).map_err(|e| format!("cannot clear {}: {e}", esp.display()))?;
    }
    let boot_efi = esp.join("EFI").join("BOOT");
    let limine_dir = esp.join("boot").join("limine");
    let user_dir = esp.join("boot").join("user");
    for dir in [&boot_efi, &limine_dir, &user_dir] {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    copy(efi, &boot_efi.join("BOOTX64.EFI"))?;
    copy(kernel, &esp.join("boot").join("kernel"))?;
    for module in modules {
        fs::write(esp.join("boot").join(&module.path), &module.bytes)
            .map_err(|e| format!("cannot write {}: {e}", module.path))?;
    }
    let paths: Vec<&str> = modules.iter().map(|m| m.path.as_str()).collect();
    fs::write(limine_dir.join("limine.conf"), limine_conf(cmdline, &paths))
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
    fn limine_conf_carries_cmdline_and_modules() {
        let conf = limine_conf("test", &["kernel.lines", "user/llm.elf"]);
        assert!(conf.contains("protocol: limine"));
        assert!(conf.contains("path: boot():/boot/kernel\n"));
        assert!(conf.contains("cmdline: test"));
        assert!(conf.contains("module_path: boot():/boot/kernel.lines\n"));
        assert!(conf.contains("module_path: boot():/boot/user/llm.elf\n"));
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
        let list = qemu::args(&firmware, Accel::Tcg, true);
        assert!(list.contains(&"format=raw,file=fat:rw:esp".to_string()));
        assert!(list.contains(&"isa-debug-exit,iobase=0xf4,iosize=0x04".to_string()));
        assert!(list.windows(2).any(|w| w[0] == "-accel" && w[1] == "tcg"));
        assert!(
            list.windows(2)
                .any(|w| w[0] == "-display" && w[1] == "none")
        );
    }
}
