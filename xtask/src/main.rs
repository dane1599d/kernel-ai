//! Build and run tool for Kernel AI.
//!
//! cargo xtask build [--release] [--spikes]
//! cargo xtask run   [--release] [--spikes] [--accel auto|kvm|whpx|tcg] [--cpus N] [--mem SIZE] [--llm IP:PORT] [--model NAME] [--edit] [--gpu] [--model-disk PATH | --local ID [--store DIR]] [--workload A|B [--seed N]]
//! cargo xtask test  [--release] [--accel auto|kvm|whpx|tcg] [--timeout SECS] [--cpus N] [--mem SIZE] [--shard K/N]
//! cargo xtask soak  [--accel auto|kvm|whpx|tcg]   (10-minute storm, M5 exit test)
//! (`--accel wsl` on Windows: QEMU with KVM inside WSL2; see wsl/launch.rs)
//! cargo xtask smoke   (the live, edit and boot manifest scenarios on the host only, no QEMU)
//! cargo xtask icount [--shift N] [--boots N] [--timeout SECS]   (icount determinism, M10a-2)
//! cargo xtask fluency --llm HOST:PORT --model NAME [--repeat N]   (M8b script gate, host only)
//! cargo xtask fluency --local ID [--store DIR]   (the same gate on our own engine)
//! cargo xtask setup | model chat|check|pin | bench quant   (the local LLM on the host; see models/mod.rs)
//! cargo xtask tune --sim ... | demo | check | record | ab   (M10a; see tune/mod.rs)
//! cargo xtask tokens --gguf PATH   (S10-0: real token counts of the prompt parts; see tokens.rs)
//! cargo xtask actbench --local ID [--arm act,..] [--seed N,..] [--limit N] [--gate]   (task 18 gate, S02-1, S10-1; GitHub only; see actbench/mod.rs)
//! cargo xtask latency --local ID [--store DIR] [--accel kvm]   (S02-2: Enter to busy sign and to done, real model in the guest)
//! cargo xtask doctor   (can this PC run Kernel AI in an accelerated VM, and what to fix; read-only)
//! cargo xtask vbox [--start [--headless]]   (the VirtualBox VM "Kernel AI"; see vbox/mod.rs)
//! cargo xtask wit-tables [--check]   (wasmhost/src/tables.rs from wit/: the host tables of the kai:app world)
//! cargo xtask licences check   (every licence in the tree and the dependency graph is allowed and has its text; see licences.rs)
//! cargo xtask notices [--check]   (writes or checks NOTICES.txt, the third-party notices the image carries; see notices.rs)
//! cargo xtask fs mkfs|import|ls|cat|check|replay ...   (kaifs data disks on the host, no WSL; see fs/mod.rs)
//! cargo xtask release-image [--out PATH]   (the release image and its NOTICES.txt; release.yml runs it)
//! cargo xtask limine   (fetches and checks Limine's pinned files, for the release's source tarball)
//!
//! `test` boots the kernel headless once per test case (boot and memory checks,
//! page fault, double fault, write-protect, stack overflow, 60 s timer, keyboard,
//! scheduling latency under load, policy watchdog; keys are typed through the QEMU
//! monitor) with the spikes enabled. Each case passes only if the kernel reports
//! success through QEMU's isa-debug-exit device.
//!
//! Every `build`, `run`, `test` and `soak` checks that the kernel ELF has no x87,
//! MMX, SSE or AVX instruction (see floatguard.rs).

mod actbench;
mod blklog;
mod blob;
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
#[path = "fs/mod.rs"]
mod fs_tool;
mod hang;
mod httpd;
mod icount;
mod image;
mod kai_actions;
mod latency;
mod licences;
mod lines;
mod local_ask;
mod local_script;
mod mock_dns;
mod mock_llm;
mod models;
mod notices;
mod options;
mod pkgrepo;
mod qemu;
mod release_image;
mod reuse;
mod screen;
mod smoke;
mod strip;
mod tokens;
mod tune;
mod userbuild;
mod vbox;
mod wit;
mod wit_tables;
mod wsl;

use options::{Accel, Options, Task, parse_args};
use sha2::{Digest, Sha256};

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use std::{env, fs};

pub(crate) const LIMINE_VERSION: &str = "12.9.3";
const LIMINE_SHA256: &str = "9f42fe9ea2e84d71056529969b1d5c24ba9a2287ffcc01bcce58b0ef8aa98b1d";
pub(crate) const KERNEL_TARGET: &str = "x86_64-unknown-none";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fluency") => fluency::main(&args[1..], &workspace_root().join("target")),
        Some("setup" | "model" | "bench") => models::main(&args),
        Some("tune") => tune::main(&args[1..]),
        Some("tokens") => tokens::main(&args[1..]),
        Some("actbench") => actbench::main(&args[1..], &workspace_root().join("target")),
        Some("latency") => latency::main(&args[1..]),
        Some("doctor") => doctor::main(&args[1..]),
        Some("vbox") => vbox::main(&args[1..]),
        Some("licences") => licences::main(&args[1..]),
        Some("notices") => notices::main(&args[1..]),
        Some("wit-check") => wit::main(&args[1..]),
        Some("wit-tables") => wit_tables::main(&args[1..]),
        Some("fs") => fs_tool::main(&args[1..]),
        Some("pkgrepo") => pkgrepo::main(&args[1..]),
        Some("release-image") => release_image::main(&args[1..]),
        // The release's source tarball carries Limine's pinned files (ADR 0009 D7).
        Some("limine") => ensure_limine(&workspace_root().join("target"))
            .map(|efi| println!("xtask: Limine {LIMINE_VERSION} at {}", efi.display())),
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
    // With `--shard`, shard 1 alone runs the host scenarios; with `XTASK_CASE`,
    // none does.
    if options.task == Task::Test
        && options.shard.is_none_or(|(k, _)| k == 1)
        && env::var("XTASK_CASE").is_err()
    {
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
        let mut args = qemu::args(&firmware, accel, false, disk.as_ref(), sizing);
        let data = options
            .data_disk
            .clone()
            .unwrap_or_else(|| target_dir.join(blklog::IMAGE));
        if options.fresh_data || !data.exists() {
            blklog::make_disk(&data, Some(options.cache_mib))?;
            println!("xtask: a fresh kai-data disk at {}", data.display());
        }
        let data = match options.data_disk {
            // QEMU runs in target/, so the default needs no host path (or WSL path).
            None => String::from(blklog::IMAGE),
            Some(_) => data.display().to_string(),
        };
        args.extend(blklog::run_args(&data));
        if options.gpu {
            args.extend(["-vga", "none", "-device", "virtio-gpu-pci"].map(String::from));
        }
        let args = wsl::wrap(wsl.as_ref(), args)?;
        println!("xtask: {} {}", qemu.display(), args.join(" "));
        return qemu::run_interactive(&qemu, &args, &target_dir);
    }
    // Tests talk to a local stand-in for the LLM server, reached from the guest at
    // 10.0.2.2 through QEMU's user-mode network.
    let mock = mock_llm::start()?;
    let mock_dns = mock_dns::start()?;
    let blob = blob::start()?;
    let llm = format!("10.0.2.2:{}", mock.port);
    let llm_args = llm_cmdline(Some(&llm), Some(mock_llm::MODEL));
    let mut failures = Vec::new();
    let cases = if options.task == Task::Soak {
        cases::SOAK_CASES.iter().collect()
    } else {
        cases::test_cases()
    };
    let cases = cases
        .into_iter()
        .enumerate()
        .filter(|&(index, _)| options::in_shard(options.shard, index))
        .map(|(_, case)| case)
        // `XTASK_CASE=name` boots only that case.
        .filter(|case| env::var("XTASK_CASE").map_or(true, |only| only == case.name))
        .collect::<Vec<_>>();
    for &first in &cases {
        // A case may name a second boot on the same data disk (`then`), booted
        // once the first has passed (P2 plan section 4.2).
        let (mut next, mut fresh) = (Some(first), true);
        let mut boot = 1;
        while let Some(case) = next.take() {
            let service = match case.build {
                true => Some(edit_smoke::service(&root, case.name)?),
                false => None,
            };
            let server = if case.server { llm_args.as_str() } else { "" };
            let mut cmdline = format!("{} {server}", case.cmdline);
            let cutting = case.cuts > 0 && boot <= case.cuts;
            if case.cuts > 0 {
                let last = if cutting { "" } else { " last" };
                cmdline = format!("{cmdline} boot={boot}{last}");
            }
            if let Some(rtt) = case.blob_rtt_ms {
                cmdline = format!("{cmdline} blob=10.0.2.2:{} rtt_ms={rtt}", blob.port);
            }
            if case.dns {
                cmdline = format!("{cmdline} {}", mock_dns.cmdline());
            }
            let disk = match disk::for_case(case, &target_dir) {
                Ok(disk) => disk,
                Err(message) => {
                    eprintln!("xtask: test case {}: {message}", case.name);
                    failures.push(case.name);
                    break;
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
            let record = image::slots::slot_record(&root, SLOT_RELEASE_SEQ);
            if case.slots {
                modules.push(esp::Module {
                    // `kernel/src/slots.rs` finds it by this path.
                    path: "kai.slot".into(),
                    bytes: record.clone().into_bytes(),
                });
                // The trust anchor `updated` checks the update disk against.
                modules.push(esp::Module {
                    path: "kai.root.json".into(),
                    bytes: image::update_disk::test_root()?.root_json()?,
                });
            }
            let update_disk = match case.update {
                Some(update) => Some(update_disk(
                    &root,
                    &target_dir,
                    &kernel,
                    &modules,
                    &efi,
                    update,
                )?),
                None => None,
            };
            esp::make_esp(&esp, &kernel, &modules, &efi, &cmdline)?;
            modules.truncate(shared);
            let firmware = wsl::firmware(wsl.as_ref(), &qemu, &target_dir)?;
            // A chained slot boot starts on the variables the last boot left.
            let saved_vars = target_dir.join(SLOT_VARS);
            if case.slots && !fresh {
                esp::copy(&saved_vars, &firmware.vars)?;
            }
            let monitor = qemu::free_port()?;
            let sizing = options::Sizing {
                mem_mib: options.sizing.mem_mib.max(case.memory_mib),
                ..options.sizing
            };
            let mut args = qemu::args(&firmware, accel, true, disk.as_ref(), sizing);
            if let Some(data) = case.data {
                if fresh {
                    blklog::prepare(&target_dir, data)?;
                }
                args.extend(blklog::qemu_args(data));
            }
            if case.image {
                qemu::boot_from_image(&mut args, &esp, &target_dir)?;
            }
            if case.slots {
                qemu::boot_from_slots(&mut args, &esp, &target_dir, &cmdline, &record)?;
            }
            if let Some(name) = &update_disk {
                args.extend(qemu::update_disk_args(name));
            }
            if case.gpu {
                args.extend(screen::gpu_args());
            }
            if case.bridge {
                qemu::net_behind_bridge(&mut args);
            }
            if let Some(rtt) = case.blob_rtt_ms.filter(|&rtt| rtt > 0) {
                args.extend(qemu::rtt_args(rtt));
            }
            if case.screen.is_some() {
                screen::clear(&target_dir);
            }
            args.push("-monitor".into());
            args.push(format!("tcp:127.0.0.1:{monitor},server=on,wait=off"));
            let (input, qmp) = qemu::input_args(case)?;
            args.extend(input);
            let args = wsl::wrap(wsl.as_ref(), args)?;
            let run = qemu::TestRun {
                case,
                accel,
                timeout: case
                    .min_timeout
                    .map_or(options.timeout, |least| least.max(options.timeout)),
                monitor,
                qmp,
                exact: disk
                    .as_ref()
                    .map_or_else(Vec::new, disk::ModelDisk::exact_lines),
                contains: disk
                    .as_ref()
                    .map_or_else(Vec::new, disk::ModelDisk::console_text),
                cut: cutting.then(|| (format!("files: boot {boot}:"), cut_delay(case.name, boot))),
            };
            let requests = mock.requests();
            let after = |service: &Option<buildsvc::BuildService>| match (case.after, service) {
                (Some(check), Some(service)) => check(service),
                _ => Ok(()),
            };
            let result = qemu::run_test(&qemu, &args, &target_dir, &run);
            let result = match case.slots {
                true => result.and_then(|()| esp::copy(&firmware.vars, &saved_vars)),
                false => result,
            };
            let result = result
                .and_then(|()| after(&service))
                .and_then(|()| match case.data {
                    Some(
                        data @ blklog::Data {
                            log: Some(expect), ..
                        },
                    ) => blklog::check_data(&target_dir, data, expect),
                    _ => Ok(()),
                })
                .and_then(|()| match case.screen {
                    Some(expect) => screen::check(&target_dir, expect),
                    None => Ok(()),
                })
                .and_then(|()| match mock.requests() - requests {
                    sent if !case.server && sent > 0 => Err(format!(
                        "FAIL: booted without a server, yet the mock LLM got {sent} requests"
                    )),
                    _ => Ok(()),
                });
            if let Err(message) = result {
                eprintln!("xtask: test case {}: {message}", case.name);
                failures.push(case.name);
                break;
            }
            if cutting {
                boot += 1;
                (next, fresh) = (Some(case), false);
                continue;
            }
            (next, fresh) = (case.then, false);
        }
    }
    if let Some(before) = &real_repo {
        buildsvc::verify::unchanged(&root, before)?;
    }
    println!(
        "xtask: the mock LLM server answered {} chat requests; the mock DNS server got {} queries",
        mock.chats(),
        mock_dns.queries()
    );
    if failures.is_empty() {
        println!("xtask: all {} test cases passed", cases.len());
        Ok(())
    } else {
        Err(format!("failed test cases: {}", failures.join(", ")))
    }
}

/// When the power goes in boot `boot` of a case with cuts: 0.2 to 2 s after its
/// line, seeded by the case and the boot, so a failure repeats.
pub(crate) fn cut_delay(name: &str, boot: u32) -> std::time::Duration {
    let mut x = name
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64 ^ u64::from(boot), |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
        | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    std::time::Duration::from_millis(200 + x % 1800)
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

pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

pub(crate) fn build_kernel(root: &Path, options: &Options) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .env("KAI_COMMIT", esp::commit_id(root))
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
pub(crate) fn ensure_limine(target_dir: &Path) -> Result<PathBuf, String> {
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

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
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

/// Builds `target/update.img` for a case: this build as a slot image of the
/// next release, signed by the run's test root (`image/update_disk.rs`). The
/// image's bundle carries its own `kai.slot` naming that release.
fn update_disk(
    root: &Path,
    target_dir: &Path,
    kernel: &Path,
    modules: &[esp::Module],
    efi: &Path,
    update: cases::UpdateDisk,
) -> Result<String, String> {
    let release = SLOT_RELEASE_SEQ + 1;
    let record = image::slots::slot_record(root, release);
    let mut files: Vec<&esp::Module> = modules.iter().filter(|m| m.path != "kai.slot").collect();
    let slot_file = esp::Module {
        path: "kai.slot".into(),
        bytes: record.clone().into_bytes(),
    };
    files.push(&slot_file);
    let staging = target_dir.join("esp-update");
    esp::make_esp(&staging, kernel, &files, efi, "")?;
    let image = image::slots::slot_image(&staging, &record)?;
    let disk = image::update_disk::test_root()?.disk(&image, release, update.tampered)?;
    let name = String::from("update.img");
    fs::write(target_dir.join(&name), &disk).map_err(|e| format!("cannot write {name}: {e}"))?;
    println!(
        "xtask: update disk {name}: release {release}, {} MiB{}",
        disk.len() >> 20,
        if update.tampered {
            ", one byte of the image flipped"
        } else {
            ""
        }
    );
    Ok(name)
}

/// The release a test build's slots hold (ADR 0008 `release_seq`).
const SLOT_RELEASE_SEQ: u32 = 1;
/// Where a chained slot case keeps the variable store between its boots.
const SLOT_VARS: &str = "slot-vars.fd";

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
