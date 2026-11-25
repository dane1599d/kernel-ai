//! The host side of a run: the workspace, the kernel build, Limine, QEMU and
//! its firmware, and the accelerator.

use crate::options::{Accel, Options};
use crate::{KERNEL_TARGET, LIMINE_SHA256, LIMINE_VERSION, esp, wsl};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

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
            // GitHub refuses release downloads now and then (a 403 or a 5xx under
            // CI load, 2026-11-14); the checksum below holds whatever arrives.
            .args([
                "-sSLf",
                "--retry",
                "5",
                "--retry-all-errors",
                "--retry-delay",
                "5",
                "-o",
                archive,
                &url,
            ]),
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
pub(crate) fn tar_program() -> &'static str {
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

pub(crate) fn find_qemu() -> Result<PathBuf, String> {
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
pub(crate) struct Firmware {
    pub(crate) code: PathBuf,
    pub(crate) vars: PathBuf,
}

pub(crate) fn find_firmware(qemu: &Path, target_dir: &Path) -> Result<Firmware, String> {
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

pub(crate) fn resolve_accel(requested: Accel) -> Accel {
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

pub(crate) fn run_checked(command: &mut Command) -> Result<(), String> {
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
    use crate::{options, qemu};

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
