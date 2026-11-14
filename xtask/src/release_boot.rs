//! `cargo xtask release-boot --image PATH --model PATH [--accel kvm|tcg]
//! [--timeout SECS]`: boots a release image as it is (the release itself
//! once `release/pkgrepo` is committed, else a `release-image --test-root`
//! one), its one disk on virtio-blk, with the model file attached read-only as the
//! import disk (`kai-import`), and passes once the console shows the desk, the
//! shell's answer to `models import` and `updated`'s import line
//! ([`crate::cases::release::RELEASE_BOOT`]). With no `test=` mode the kernel
//! never exits by itself, so QEMU is stopped then. `release.yml` runs it.
//!
//! A fresh image's first boot is the recovery install: R finds no boot record,
//! installs the release in `kai-a` and `kai-b` as trials, arms `kai-a` and
//! resets. So this boot, unlike the cases', runs without `-no-reboot`: the
//! same QEMU, disk and firmware variables go on into `kai-a`, as a person's
//! VM does, and the desk and the keys come on that boot.

use crate::cases::release::RELEASE_BOOT;
use crate::options::{Accel, Sizing};
use crate::pkgrepo::models;
use crate::qemu::{self, TestRun};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str =
    "usage: cargo xtask release-boot --image PATH --model PATH [--accel kvm|tcg] [--timeout SECS]";
/// The import alone moves 639 MB in 7 KiB pushes.
const DEFAULT_TIMEOUT_S: u64 = 900;

struct Args {
    image: PathBuf,
    model: PathBuf,
    accel: Accel,
    timeout: Duration,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let (mut image, mut model) = (None, None);
    let (mut accel, mut timeout) = (Accel::Auto, Duration::from_secs(DEFAULT_TIMEOUT_S));
    let mut words = args.iter();
    while let Some(flag) = words.next() {
        let value = words.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--image" => image = Some(PathBuf::from(value)),
            "--model" => model = Some(PathBuf::from(value)),
            "--accel" => {
                accel = match value.as_str() {
                    "kvm" => Accel::Kvm,
                    "tcg" => Accel::Tcg,
                    _ => return Err(USAGE.into()),
                }
            }
            "--timeout" => {
                let secs = value
                    .parse()
                    .map_err(|_| format!("{value} is not seconds"))?;
                timeout = Duration::from_secs(secs);
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Args {
        image: image.ok_or(USAGE)?,
        model: model.ok_or(USAGE)?,
        accel,
        timeout,
    })
}

pub fn main(args: &[String]) -> Result<(), String> {
    let args = parse(args)?;
    let signed = models::release_test()?.remove(0);
    let length = fs::metadata(&args.model)
        .map_err(|e| format!("cannot read {}: {e}", args.model.display()))?
        .len();
    if length != signed.length {
        return Err(format!(
            "{} is {length} bytes; {} is {}",
            args.model.display(),
            signed.id,
            signed.length
        ));
    }
    let absolute = |path: &PathBuf| {
        fs::canonicalize(path).map_err(|e| format!("cannot find {}: {e}", path.display()))
    };
    let (image, model) = (absolute(&args.image)?, absolute(&args.model)?);
    let target_dir = crate::workspace_root().join("target");
    fs::create_dir_all(&target_dir).map_err(|e| format!("cannot make target/: {e}"))?;
    let qemu = crate::find_qemu()?;
    let firmware = crate::find_firmware(&qemu, &target_dir)?;
    let accel = crate::resolve_accel(args.accel);
    let mut qemu_args = qemu::args(&firmware, accel, true, None, Sizing::default());
    // The first boot's reset, after the recovery install, restarts the VM.
    qemu_args.retain(|arg| arg != "-no-reboot");
    qemu::boot_from_disk(&mut qemu_args, &image.to_string_lossy())?;
    qemu_args.extend(qemu::import_disk_args(&model.to_string_lossy()));
    let monitor = qemu::free_port()?;
    qemu_args.push("-monitor".into());
    qemu_args.push(format!("tcp:127.0.0.1:{monitor},server=on,wait=off"));
    println!(
        "xtask: release boot of {} with {} as the import disk",
        image.display(),
        signed.id
    );
    let run = TestRun {
        case: &RELEASE_BOOT,
        accel,
        timeout: args.timeout,
        monitor,
        qmp: None,
        exact: Vec::new(),
        contains: vec![format!(
            "updated: imported {} ({}), {} bytes",
            signed.id,
            &signed.sha256[..12],
            signed.length
        )],
        cut: None,
        capture: None,
        forbid: forbidden(&crate::workspace_root()),
    };
    qemu::run_test(&qemu, &qemu_args, &target_dir, &run)
}

/// What `pkg` prints when the image's repository does not check out.
const PKG_FAIL: &str = "pkg: FAIL";

/// What the release boot may never show (founder test 2, test 5). Once the
/// founder's repository is committed, the image carries it, so a `pkg: FAIL`
/// fails the boot; before that the image has none and `pkg` says so.
fn forbidden(root: &std::path::Path) -> Vec<String> {
    let signed = root
        .join(crate::release_repo::REPO)
        .join(kaipkg::image::ROOT);
    match signed.exists() {
        true => vec![String::from(PKG_FAIL)],
        false => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_image_and_the_model_are_both_needed() {
        let both = parse(&words(
            "--image a.img --model m.gguf --accel kvm --timeout 60",
        ))
        .unwrap();
        assert_eq!(both.accel, Accel::Kvm);
        assert_eq!(both.timeout, Duration::from_secs(60));
        assert!(parse(&words("--image a.img")).is_err());
        assert!(parse(&words("--model m.gguf")).is_err());
        assert!(parse(&words("--image a.img --model m.gguf --accel whpx")).is_err());
        assert!(parse(&words("--image a.img --model")).is_err());
    }

    #[test]
    fn the_case_waits_on_named_lines_and_stops_on_its_lines() {
        const { assert!(RELEASE_BOOT.stops) };
        assert_eq!(RELEASE_BOOT.keys.len(), RELEASE_BOOT.keys_on.len());
        assert!(RELEASE_BOOT.expect.iter().all(|text| !text.is_empty()));
        assert!(RELEASE_BOOT.data.is_none() && RELEASE_BOOT.import.is_none());
    }

    /// `pkg: FAIL` fails the release boot once a root is committed, and only then.
    #[test]
    fn pkg_fail_is_forbidden_once_the_founders_root_is_committed() {
        let dir = std::env::temp_dir().join(format!("kai-release-boot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(forbidden(&dir).is_empty());
        let root = dir
            .join(crate::release_repo::REPO)
            .join(kaipkg::image::ROOT);
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::write(&root, b"{}").unwrap();
        assert_eq!(forbidden(&dir), [PKG_FAIL]);
        let _ = fs::remove_dir_all(&dir);
    }
}
