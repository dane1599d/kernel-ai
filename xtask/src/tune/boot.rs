//! What `tune demo`, `tune ab` and `tune record` boot: the kernel, the ring-3
//! programs and the component versions, built as `cargo xtask test` builds them,
//! with QEMU, its firmware and Limine found the same way. Options they do not
//! know themselves (`--accel`, `--cpus`, `--mem`, `--timeout`, `--release`) are
//! read as `test` reads them.

use crate::esp::{self, Module};
use crate::options::{Accel, Options};
use std::path::{Path, PathBuf};

pub struct Booter {
    pub options: Options,
    pub target_dir: PathBuf,
    kernel: PathBuf,
    modules: Vec<Module>,
    efi: PathBuf,
    pub qemu: PathBuf,
    pub accel: Accel,
}

impl Booter {
    /// Builds everything; `flags` are `test`'s own options.
    pub fn build(flags: &[String]) -> Result<Self, String> {
        let mut args = vec![String::from("test")];
        args.extend_from_slice(flags);
        let options = crate::options::parse_args(&args)?;
        let root = crate::workspace_root();
        let kernel = crate::build_kernel(&root, &options)?;
        crate::floatguard::check(&kernel)?;
        let programs = crate::userbuild::build_user(&root, &options)?;
        let versions = crate::components::build(&root)?;
        let target_dir = root.join("target");
        let mut modules = esp::boot_modules(&kernel, &programs, &root)?;
        modules.extend(versions);
        let efi = crate::ensure_limine(&target_dir)?;
        let qemu = crate::find_qemu()?;
        let accel = crate::resolve_accel(options.accel);
        Ok(Self {
            options,
            target_dir,
            kernel,
            modules,
            efi,
            qemu,
            accel,
        })
    }

    /// Writes the boot drive with `cmdline`, and returns fresh firmware (a new
    /// variable store every boot).
    pub fn prepare(&self, cmdline: &str) -> Result<crate::Firmware, String> {
        let esp = self.target_dir.join("esp");
        esp::make_esp(&esp, &self.kernel, &self.modules, &self.efi, cmdline)?;
        crate::find_firmware(&self.qemu, &self.target_dir)
    }

    pub fn target(&self) -> &Path {
        &self.target_dir
    }
}

/// Splits `args` into the ones a tune task names in `own` (with their values)
/// and the rest, for [`Booter::build`].
pub fn split(args: &[String], own: &[&str]) -> (Vec<(String, String)>, Vec<String>) {
    let (mut mine, mut rest) = (Vec::new(), Vec::new());
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if own.contains(&arg.as_str()) {
            mine.push((arg.clone(), iter.next().cloned().unwrap_or_default()));
        } else {
            rest.push(arg.clone());
        }
    }
    (mine, rest)
}
