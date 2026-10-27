//! `cargo xtask release-image [--out PATH]` (ADR 0009 D7, ADR 0008 section 8;
//! measured by spike S09-1 with S08-2): the release-profile boot image a `v*`
//! tag ships, with no model and an empty command line, and its size and
//! SHA-256. Two runners that print the same hash have built the same image.
//! `NOTICES.txt` is written beside it, byte-identical to the image's copy, as
//! the release's second asset. `release.yml` runs it.

use crate::{components, esp, floatguard, image, options, userbuild};
use std::fs;
use std::path::PathBuf;

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let out = match args {
        [] => root.join("target").join("kernel-ai.img"),
        [flag, path] if flag == "--out" => PathBuf::from(path),
        _ => return Err("usage: cargo xtask release-image [--out PATH]".into()),
    };
    let options = options::parse_args(&["build".into(), "--release".into()])?;
    let kernel = crate::build_kernel(&root, &options)?;
    floatguard::check(&kernel)?;
    let programs = userbuild::build_user(&root, &options)?;
    let target_dir = root.join("target");
    let mut modules = esp::boot_modules(&kernel, &programs, &root)?;
    modules.extend(components::build(&root)?);
    let efi = crate::ensure_limine(&target_dir)?;
    let esp_dir = target_dir.join("release-esp");
    esp::make_esp(&esp_dir, &kernel, &modules, &efi, "")?;
    let bytes = image::build(&esp_dir, &out)?;
    let disk = fs::read(&out).map_err(|e| format!("cannot read {}: {e}", out.display()))?;
    let notices = out.with_file_name(crate::notices::FILE);
    esp::copy(&esp_dir.join(crate::notices::FILE), &notices)?;
    println!("xtask: release image {}", out.display());
    println!("xtask: size {bytes} bytes ({} MiB)", bytes >> 20);
    println!("xtask: sha256 {}", crate::sha256_hex(&disk));
    // Each part's hash, so a mismatch between runners names the part that moved.
    for part in [
        "boot/kernel",
        "boot/kai.bundle",
        "NOTICES.txt",
        "EFI/BOOT/BOOTX64.EFI",
    ] {
        let path = esp_dir.join(part);
        let bytes = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        println!(
            "xtask: part {part} {} {}",
            bytes.len(),
            crate::sha256_hex(&bytes)
        );
    }
    Ok(())
}
