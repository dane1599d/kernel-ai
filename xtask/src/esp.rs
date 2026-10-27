//! The boot drive: the ESP with Limine, the kernel and its modules, and `NOTICES.txt`.

use crate::{lines, notices, strip};
use std::fs;
use std::path::{Path, PathBuf};

/// A file Limine loads next to the kernel, at `boot/<path>` on the ESP.
pub struct Module {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// The kernel's line table, then each user program and its line table.
pub fn boot_modules(
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

/// The one module Limine loads besides the kernel, under `boot/`.
pub const BUNDLE: &str = "kai.bundle";

pub fn limine_conf(cmdline: &str, module_paths: &[&str]) -> String {
    let mut conf = format!(
        "timeout: 0\n\n/{}\n    protocol: limine\n    path: boot():/boot/kernel\n    cmdline: {cmdline}\n",
        kabi::PRODUCT_NAME
    );
    for path in module_paths {
        conf.push_str(&format!("    module_path: boot():/boot/{path}\n"));
    }
    conf
}

/// Lays out the EFI system partition that QEMU exposes as a virtual FAT drive.
pub fn make_esp<M: core::borrow::Borrow<Module>>(
    esp: &Path,
    kernel: &Path,
    modules: &[M],
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
    // Every module goes into one `kai-bundle 1` (ADR 0008), so adding a program
    // never changes limine.conf.
    let files: Vec<(&str, &[u8])> = modules
        .iter()
        .map(M::borrow)
        .map(|m| (m.path.as_str(), m.bytes.as_slice()))
        .collect();
    let bundle = kaibundle::build(&files).map_err(|e| format!("kai.bundle: {e:?}"))?;
    fs::write(esp.join("boot").join(BUNDLE), &bundle)
        .map_err(|e| format!("cannot write {BUNDLE}: {e}"))?;
    fs::write(
        limine_dir.join("limine.conf"),
        limine_conf(cmdline, &[BUNDLE]),
    )
    .map_err(|e| format!("cannot write limine.conf: {e}"))?;
    // The notices the image owes (ADR 0009 D6), under a line naming this build.
    let root = crate::workspace_root();
    let text = fs::read_to_string(root.join(notices::FILE))
        .map_err(|e| format!("cannot read {}: {e}", notices::FILE))?;
    fs::write(
        esp.join(notices::FILE),
        notices::image_line(&root) + text.replace("\r\n", "\n").as_str(),
    )
    .map_err(|e| format!("cannot write {}: {e}", notices::FILE))
}

/// The commit the kernel is built from, for its banner: the short hash, and
/// `-dirty` with uncommitted changes; "unknown" without git.
pub fn commit_id(root: &Path) -> String {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    match (
        git(&["rev-parse", "--short=12", "HEAD"]),
        git(&["status", "--porcelain"]),
    ) {
        (Some(hash), Some(changes)) if !changes.is_empty() => format!("{hash}-dirty"),
        (Some(hash), _) => hash,
        (None, _) => "unknown".into(),
    }
}

pub fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))
}
