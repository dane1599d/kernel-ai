//! Build and run tool for Kernel AI.
//!
//! cargo xtask build [--release] [--spikes]
//! cargo xtask run   [--release] [--spikes] [--accel auto|kvm|whpx|tcg] [--cpus N] [--mem SIZE] [--llm IP:PORT] [--model NAME] [--edit] [--model-disk PATH | --local ID [--store DIR]] [--workload A|B [--seed N]]
//! cargo xtask test  [--release] [--accel auto|kvm|whpx|tcg] [--timeout SECS] [--cpus N] [--mem SIZE]
//! cargo xtask soak  [--accel auto|kvm|whpx|tcg]   (10-minute storm, M5 exit test)
//! (`--accel wsl` on Windows: QEMU with KVM inside WSL2; see wsl/launch.rs)
//! cargo xtask smoke   (the live, edit and boot manifest scenarios on the host only, no QEMU)
//! cargo xtask icount [--shift N] [--boots N] [--timeout SECS]   (icount determinism, M10a-2)
//! cargo xtask fluency --llm HOST:PORT --model NAME [--repeat N]   (M8b script gate, host only)
//! cargo xtask fluency --local ID [--store DIR]   (the same gate on our own engine)
//! cargo xtask setup | model chat|check|pin | bench quant   (the local LLM on the host; see models/mod.rs)
//! cargo xtask tune --sim ... | demo | check | record | ab   (M10a; see tune/mod.rs)
//! cargo xtask doctor   (can this PC run Kernel AI in an accelerated VM, and what to fix; read-only)
//!
//! `test` boots the kernel headless once per test case (boot and memory checks,
//! page fault, double fault, write-protect, stack overflow, 60 s timer, keyboard,
//! scheduling latency under load, policy watchdog; keys are
//! typed through the QEMU monitor) with the spikes
//! enabled. Each case passes only if the
//! kernel reports success through QEMU's isa-debug-exit device.
//!
//! Every `build`, `run`, `test` and `soak` checks that the kernel ELF has no x87,
//! MMX, SSE or AVX instruction (see floatguard.rs).

mod boot_smoke;
mod buildsvc;
mod cases;
mod components;
mod disk;
mod doctor;
mod edit_smoke;
mod esp;
mod floatguard;
mod fluency;
mod httpd;
mod icount;
mod image;
mod lines;
mod local_ask;
mod local_script;
mod mock_llm;
mod models;
mod options;
mod qemu;
mod smoke;
mod strip;
mod tune;
mod userbuild;
mod wsl;

use options::{Accel, Options, Task, parse_args};
use sha2::{Digest, Sha256};

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use std::{env, fs};

const LIMINE_VERSION: &str = "12.9.3";
const LIMINE_SHA256: &str = "9f42fe9ea2e84d71056529969b1d5c24ba9a2287ffcc01bcce58b0ef8aa98b1d";
pub(crate) const KERNEL_TARGET: &str = "x86_64-unknown-none";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fluency") => fluency::main(&args[1..], &workspace_root().join("target")),
        Some("setup" | "model" | "bench") => models::main(&args),
        Some("tune") => tune::main(&args[1..]),
        Some("doctor") => doctor::main(&args[1..]),
        _ => parse_args(&args).and_then(|options| execute(&options)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

fn execute(options: &Options) -> Result<(), String> {
    let root = workspace_root();
    if options.task == Task::Smoke {
        let before = buildsvc::verify::real_repo(&root)?;
        let versions = components::build(&root)?;
        smoke::run(&versions)?;
        edit_smoke::run(&root, &versions)?;
        boot_smoke::run(&root, &versions)?;
        return buildsvc::verify::unchanged(&root, &before);
    }
    let kernel = build_kernel(&root, options)?;
    floatguard::check(&kernel)?;
    let programs = userbuild::build_user(&root, options)?;
    let versions = components::build(&root)?;
    if options.task == Task::Build {
        println!("xtask: kernel built at {}", kernel.display());
        println!("xtask: {} user programs built", programs.len());
        println!("xtask: {} component versions built", versions.len());
        return Ok(());
    }
    // The founder's repository, which no part of a test run may change.
    let real_repo = match options.task {
        Task::Test => Some(buildsvc::verify::real_repo(&root)?),
        _ => None,
    };
    if options.task == Task::Test {
        // The live-component and edit scenarios on the host first: a failure here
        // is in the host, a version, the editor or the builds, not in the kernel.
        smoke::run(&versions)?;
        edit_smoke::run(&root, &versions)?;
        boot_smoke::run(&root, &versions)?;
    }
    let target_dir = root.join("target");
    let mut modules = esp::boot_modules(&kernel, &programs, &root)?;
    modules.extend(versions);
    let efi = ensure_limine(&target_dir)?;
    let esp = target_dir.join("esp");
    // `--accel wsl`: QEMU runs inside WSL2, behind wsl.exe (wsl/launch.rs).
    let wsl = wsl::Wsl::for_options(options, &target_dir)?;
    let qemu = wsl
        .as_ref()
        .map_or_else(find_qemu, |wsl| Ok(wsl.program()))?;
    let accel = resolve_accel(options.accel);
    if options.task == Task::Icount {
        return icount::run(&qemu, &target_dir, &kernel, &modules, &efi, options);
    }
    if options.task == Task::Run {
        let mut cmdline = llm_cmdline(options.llm.as_deref(), options.model.as_deref())
            + options.workload_args().as_str();
        let path = match (&options.model_disk, &options.local) {
            (_, Some(id)) => Some(models::stored(id, options.store.as_deref())?),
            (path, None) => path.clone(),
        };
        let disk = path.as_deref().map(disk::ModelDisk::open).transpose()?;
        if let Some(disk) = &disk {
            cmdline = format!("{cmdline} {}", disk.cmdline());
            println!("xtask: {}", disk.describe());
        }
        // Kept until QEMU exits: the build service runs on its threads.
        let _service = if options.edit {
            let service = buildsvc::start_dev(&root)?;
            let manifest = service.boot_manifest(&buildsvc::boot::cache_dir(&root))?;
            modules.extend(manifest.modules());
            cmdline = format!(
                "{cmdline} {} {}",
                buildsvc::cmdline(&service),
                manifest.cmdline()
            );
            println!(
                "xtask: build service on 127.0.0.1:{}, building in .live/repo; afterwards: git fetch .live/repo kai/notes",
                service.port
            );
            Some(service)
        } else {
            None
        };
        esp::make_esp(&esp, &kernel, &modules, &efi, &cmdline)?;
        let firmware = wsl::firmware(wsl.as_ref(), &qemu, &target_dir)?;
        let sizing = qemu::run_sizing(accel, options.sizing, disk.is_some());
        let args = qemu::args(&firmware, accel, false, disk.as_ref(), sizing);
        let args = wsl::wrap(wsl.as_ref(), args)?;
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
        let service = match case.build {
            true => Some(edit_smoke::service(&root, case.name)?),
            false => None,
        };
        let server = if case.server { llm_args.as_str() } else { "" };
        let mut cmdline = format!("{} {server}", case.cmdline);
        let disk = match disk::for_case(case, &target_dir) {
            Ok(disk) => disk,
            Err(message) => {
                eprintln!("xtask: test case {}: {message}", case.name);
                failures.push(case.name);
                continue;
            }
        };
        if let Some(disk) = &disk {
            cmdline = format!("{cmdline} {}", disk.cmdline());
        }
        // The boot manifest's versions join the boot drive for this case only.
        let shared = modules.len();
        if let Some(service) = &service {
            let (versions, live) = boot_smoke::manifest_for(case, service)?;
            modules.extend(versions);
            cmdline = format!("{cmdline} {} {live}", buildsvc::cmdline(service));
        }
        println!(
            "xtask: test case {} ({})",
            case.name,
            buildsvc::redact(&cmdline)
        );
        esp::make_esp(&esp, &kernel, &modules, &efi, &cmdline)?;
        modules.truncate(shared);
        let firmware = wsl::firmware(wsl.as_ref(), &qemu, &target_dir)?;
        let monitor = qemu::free_port()?;
        let sizing = options::Sizing {
            mem_mib: options.sizing.mem_mib.max(case.memory_mib),
            ..options.sizing
        };
        let mut args = qemu::args(&firmware, accel, true, disk.as_ref(), sizing);
        if case.image {
            qemu::boot_from_image(&mut args, &esp, &target_dir)?;
        }
        args.push("-monitor".into());
        args.push(format!("tcp:127.0.0.1:{monitor},server=on,wait=off"));
        let args = wsl::wrap(wsl.as_ref(), args)?;
        let run = qemu::TestRun {
            case,
            accel,
            timeout: case
                .min_timeout
                .map_or(options.timeout, |least| least.max(options.timeout)),
            monitor,
            exact: disk
                .as_ref()
                .map_or_else(Vec::new, disk::ModelDisk::exact_lines),
            contains: disk
                .as_ref()
                .map_or_else(Vec::new, disk::ModelDisk::console_text),
        };
        let requests = mock.requests();
        let after = |service: &Option<buildsvc::BuildService>| match (case.after, service) {
            (Some(check), Some(service)) => check(service),
            _ => Ok(()),
        };
        let result = qemu::run_test(&qemu, &args, &target_dir, &run)
            .and_then(|()| after(&service))
            .and_then(|()| match mock.requests() - requests {
                sent if !case.server && sent > 0 => Err(format!(
                    "FAIL: booted without a server, yet the mock LLM got {sent} requests"
                )),
                _ => Ok(()),
            });
        if let Err(message) = result {
            eprintln!("xtask: test case {}: {message}", case.name);
            failures.push(case.name);
        }
    }
    if let Some(before) = &real_repo {
        buildsvc::verify::unchanged(&root, before)?;
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
    for (code, vars) in wsl::probe::LINUX_OVMF {
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
    esp::copy(&vars_template, &vars)?;
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

    #[test]
    fn limine_conf_carries_cmdline_and_modules() {
        let conf = esp::limine_conf("test", &["kernel.lines", "user/llm.elf"]);
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
        let list = qemu::args(&firmware, Accel::Tcg, true, None, Default::default());
        assert!(list.contains(&"format=raw,file=fat:rw:esp".to_string()));
        assert!(list.windows(2).any(|w| w[0] == "-m" && w[1] == "512M"));
        let sized = options::Sizing {
            cpus: Some(8),
            mem_mib: Some(6144),
        };
        let big = qemu::args(&firmware, Accel::Kvm, true, None, sized);
        assert!(big.windows(2).any(|w| w[0] == "-m" && w[1] == "6144M"));
        assert!(big.windows(2).any(|w| w[0] == "-smp" && w[1] == "8"));
        let small = options::Sizing {
            cpus: None,
            mem_mib: Some(256),
        };
        // Never below what the base system (or a model disk) needs.
        let floor = qemu::args(&firmware, Accel::Kvm, true, None, small);
        assert!(floor.windows(2).any(|w| w[0] == "-m" && w[1] == "512M"));
        assert!(!list.iter().any(|arg| arg == "-blockdev"));
        assert!(list.contains(&"isa-debug-exit,iobase=0xf4,iosize=0x04".to_string()));
        assert!(list.windows(2).any(|w| w[0] == "-accel" && w[1] == "tcg"));
        assert!(
            list.windows(2)
                .any(|w| w[0] == "-display" && w[1] == "none")
        );
    }
}
