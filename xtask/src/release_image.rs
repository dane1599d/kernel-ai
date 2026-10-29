//! `cargo xtask release-image [--out PATH]` (ADR 0009 D7, ADR 0008 section 8;
//! measured by spike S09-1 with S08-2): the release-profile slot disk a `v*`
//! tag ships (ESP, `kai-r`, `kai-a`, `kai-b`, `image/slots.rs`), with no model
//! and an empty command line, each slot holding release `release/seq`, and its
//! size and SHA-256. Two runners that print the same hash have built the same
//! image. `release/seq` is the committed `release_seq` (ADR 0008 §2): it only
//! rises, and `release.yml` refuses a tag whose number is not above the last.
//! `NOTICES.txt` is written beside it, byte-identical to the image's copy, as
//! the release's second asset. `release.yml` runs it.

use crate::{components, esp, floatguard, image, options, userbuild};
use std::fs;
use std::path::PathBuf;

/// The committed `release_seq`, `release/seq`: one positive number.
fn release_seq(root: &std::path::Path) -> Result<u32, String> {
    let path = root.join("release").join("seq");
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    text.trim()
        .parse()
        .ok()
        .filter(|seq| *seq > 0)
        .ok_or_else(|| {
            format!(
                "{} holds {:?}, not a positive number",
                path.display(),
                text.trim()
            )
        })
}

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
    let release_seq = release_seq(&root)?;
    let record = image::slots::slot_record(&root, release_seq);
    // R's install and the commit read the slot's own `kai-slot 1` (`kernel/src/slots.rs`).
    modules.push(esp::Module {
        path: "kai.slot".into(),
        bytes: record.clone().into_bytes(),
    });
    let efi = crate::ensure_limine(&target_dir)?;
    let esp_dir = target_dir.join("release-esp");
    esp::make_esp(&esp_dir, &kernel, &modules, &efi, "")?;
    let bytes = image::slots::build(&esp_dir, &out, "", &record)?;
    println!("xtask: release_seq {release_seq}");
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
