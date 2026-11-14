//! `cargo xtask release-image [--out PATH]` (ADR 0009 D7, ADR 0008 section 8;
//! measured by spike S09-1 with S08-2): the release-profile slot disk a `v*`
//! tag ships (ESP, `kai-r`, `kai-a`, `kai-b` and an empty `kai-data`,
//! `image/slots.rs`), with no model
//! and an empty command line, each slot holding release `release/seq`, and its
//! size and SHA-256. Two runners that print the same hash have built the same
//! image. `release/seq` is the committed `release_seq` (ADR 0008 §2): it only
//! rises, and `release.yml` refuses a tag whose number is not above the last.
//! `NOTICES.txt` is written beside it, byte-identical to the image's copy, as
//! the release's second asset. `release.yml` runs it.
//!
//! When `release/pkgrepo/` is committed (P4 task 18's interim key files,
//! `release_repo`), the image carries it as `pkgrepo/`, checked first: its
//! `models` role, signed with the founder's `release.key`, names the catalog's
//! models, so they import. Its `1.root.json` is also the image's `kai.root.json`,
//! the root update disks and the online repository are checked against (root
//! ceremony step 6.1): one root for both. With none committed, the image has
//! neither, and imports nothing.
//!
//! `--test-root` builds the same image with one difference: a `pkgrepo/` whose
//! `models` role, signed by a fresh test root, names only `qwen3-0.6b`
//! (`pkgrepo::release_test`), in place of the committed one. `kai.root.json`
//! stays the founder's. It exists for the case that boots the release and
//! imports that model (`release_boot.rs`) while no repository is committed.
//! Each boot module's hash is printed, so `release.yml` can check that the two
//! builds differ only beneath `pkgrepo/`.

use crate::{components, esp, floatguard, image, options, userbuild};
use image::modules::{Image, TRUST_ANCHOR};
use std::fs;
use std::path::PathBuf;

/// The committed `release_seq`, `release/seq`: one positive number.
pub(crate) fn release_seq(root: &std::path::Path) -> Result<u32, String> {
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
    let (test_root, args) = match args {
        [flag, rest @ ..] if flag == "--test-root" => (true, rest),
        _ => (false, args),
    };
    let name = match test_root {
        true => "kernel-ai-test-root.img",
        false => "kernel-ai.img",
    };
    let out = match args {
        [] => root.join("target").join(name),
        [flag, path] if flag == "--out" => PathBuf::from(path),
        _ => return Err(USAGE.into()),
    };
    let options = options::parse_args(&["build".into(), "--release".into()])?;
    let kernel = crate::build_kernel(&root, &options)?;
    floatguard::check(&kernel)?;
    let programs = userbuild::build_user(&root, &options)?;
    let target_dir = root.join("target");
    let mut modules = esp::boot_modules(&kernel, &programs, &root)?;
    let versions = components::build(&root)?;
    // The built-in apps as packages, packed from this build's own components:
    // the committed repository must sign exactly these bytes.
    let packages = crate::release_repo::packages::pack(&root, &versions)?;
    let packages = crate::release_repo::packages::built(&packages);
    modules.extend(versions);
    // Nothing that exists to be driven by a boot case ships.
    components::check_no_test_only(&modules)?;
    let release_seq = release_seq(&root)?;
    let record = image::slots::slot_record(&root, release_seq);
    // R's install and the commit read the slot's own `kai-slot 1` (`kernel/src/slots.rs`).
    modules.push(esp::Module {
        path: "kai.slot".into(),
        bytes: record.clone().into_bytes(),
    });
    let test = match test_root {
        true => Some(crate::pkgrepo::release_test()?),
        false => None,
    };
    let signed = crate::release_repo::committed(&root, Some(&packages))?;
    let committed = signed.is_some();
    let repository = repository(signed, test, &packages)?;
    let modules = image::modules::image_modules(Image::Release { committed }, modules, repository)?;
    // No extra TLS root ships: a file in `netroots/` would be trusted for
    // every host by fetch and the model download, which have TLS alone
    // (K's review of #753).
    check_no_netroots(&modules)?;
    print_modules(&modules);
    let efi = crate::ensure_limine(&target_dir)?;
    let esp_dir = target_dir.join(match test_root {
        true => "release-esp-test-root",
        false => "release-esp",
    });
    esp::make_esp(&esp_dir, &kernel, &modules, &efi, "")?;
    let data_mib = image::slots::RELEASE_DATA_MIB;
    let bytes = image::slots::build_release(&esp_dir, &out, &record, data_mib)?;
    println!("xtask: release_seq {release_seq}");
    println!("xtask: kai-data {data_mib} MiB, partition 5");
    let notices = out.with_file_name(crate::notices::FILE);
    esp::copy(&esp_dir.join(crate::notices::FILE), &notices)?;
    println!("xtask: release image {}", out.display());
    println!("xtask: size {bytes} bytes ({} MiB)", bytes >> 20);
    println!("xtask: sha256 {}", sha256_file(&out)?);
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

const USAGE: &str = "usage: cargo xtask release-image [--test-root] [--out PATH]";

/// The modules for the image's repository: `signed`, the committed
/// `release/pkgrepo`, beneath `pkgrepo/` with `packages`' bytes, which it
/// signs, at their target paths, and its root as [`TRUST_ANCHOR`]. `test`,
/// `--test-root`'s repository, replaces it beneath `pkgrepo/` only:
/// [`TRUST_ANCHOR`] stays the founder's.
pub(crate) fn repository(
    signed: Option<crate::release_repo::RepoFiles>,
    test: Option<Vec<esp::Module>>,
    packages: &[(String, Vec<u8>)],
) -> Result<Vec<esp::Module>, String> {
    let mut modules = Vec::new();
    if let Some(files) = &signed {
        let (_, root) = files
            .iter()
            .find(|(path, _)| path == kaipkg::image::ROOT)
            .ok_or("the committed repository has no root")?;
        modules.push(esp::Module {
            path: TRUST_ANCHOR.into(),
            bytes: root.clone(),
        });
    }
    match (test, signed) {
        (Some(test), _) => modules.extend(test),
        (None, Some(mut files)) => {
            let targets = packages
                .iter()
                .map(|(path, bytes)| (format!("targets/{path}"), bytes.clone()));
            files.extend(targets);
            modules.extend(crate::pkgrepo::as_modules(crate::pkgrepo::Repository {
                files,
            })?)
        }
        (None, None) => println!(
            "xtask: no {} is committed: no models role",
            crate::release_repo::REPO
        ),
    }
    Ok(modules)
}

/// Fails an image with a module under `netroots/`: the extra TLS roots only
/// a boot case's bundle may carry (`cases::Https`, the HTTPS mock's CA).
fn check_no_netroots(modules: &[esp::Module]) -> Result<(), String> {
    match modules
        .iter()
        .find(|m| m.path.starts_with(kabi::net::NETROOTS))
    {
        Some(module) => Err(format!(
            "{} is an extra TLS root, which only a boot case's bundle may carry",
            module.path
        )),
        None => Ok(()),
    }
}

/// Each boot module's path, length and SHA-256, in path order: two builds
/// that differ only beneath `pkgrepo/` print the same lines but those.
fn print_modules(modules: &[esp::Module]) {
    let mut lines: Vec<String> = modules
        .iter()
        .map(|m| {
            let sha = crate::sha256_hex(&m.bytes);
            format!("xtask: module {} {} {sha}", m.path, m.bytes.len())
        })
        .collect();
    lines.sort();
    for line in lines {
        println!("{line}");
    }
}

/// The SHA-256 of the file at `path`, read a piece at a time: the image is
/// gigabytes, most of them `kai-data`'s free space.
fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let shown = path.display();
    let mut file = fs::File::open(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    let mut hash = Sha256::new();
    let mut piece = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut piece)
            .map_err(|e| format!("cannot read {shown}: {e}"))?;
        if n == 0 {
            break;
        }
        hash.update(&piece[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(path: &str) -> esp::Module {
        esp::Module {
            path: path.into(),
            bytes: b"x".to_vec(),
        }
    }

    #[test]
    fn a_release_refuses_an_extra_tls_root() {
        let clean = [module("user/fetch.elf"), module("pkgrepo/root.json")];
        assert_eq!(check_no_netroots(&clean), Ok(()));
        let with_root = [module("user/fetch.elf"), module("netroots/1.der")];
        let refused = check_no_netroots(&with_root).unwrap_err();
        assert!(refused.contains("netroots/1.der"), "{refused}");
    }

    /// The committed repository's root is `kai.root.json` too, byte for byte,
    /// the packages it signs go beside it at their target paths, and
    /// `--test-root` swaps the repository alone.
    #[test]
    fn the_founders_root_is_the_trust_anchor_and_stays_under_a_test_root() {
        let signed = vec![
            (kaipkg::image::ROOT.to_string(), b"founder".to_vec()),
            ("metadata/timestamp.json".to_string(), b"ts".to_vec()),
        ];
        let packages = [("notes-1.0.0.kpkg".to_string(), b"kpkg".to_vec())];
        let real = repository(Some(signed.clone()), None, &packages).unwrap();
        let paths: Vec<&str> = real.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                TRUST_ANCHOR,
                "pkgrepo/metadata/1.root.json",
                "pkgrepo/metadata/timestamp.json",
                "pkgrepo/targets/notes-1.0.0.kpkg"
            ]
        );
        assert_eq!(real[0].bytes, real[1].bytes);
        assert_eq!(real[3].bytes, b"kpkg");
        let test = vec![module("pkgrepo/metadata/1.root.json")];
        let tested = repository(Some(signed), Some(test), &packages).unwrap();
        assert_eq!(tested.len(), 2);
        assert_eq!(
            (tested[0].path.as_str(), &tested[0].bytes[..]),
            (TRUST_ANCHOR, &b"founder"[..])
        );
        assert_eq!(tested[1].bytes, b"x");
        assert!(repository(None, None, &packages).unwrap().is_empty());
    }
}
