//! The disks a boot case builds for itself: the update disk carrying this
//! build as the next release, and a kaifs data disk holding a model, with
//! its hint damaged when the case asks.

use crate::{SLOT_RELEASE_SEQ, blklog, cases, esp, fs_tool, image};
use std::fs;
use std::path::Path;

/// This build as a slot image of the next release, and that release's number:
/// what an update disk carries, and what the HTTPS mock serves as the online
/// repository's `slot.img`. The image's bundle carries its own `kai.slot`
/// naming that release.
pub(crate) fn update_image(
    root: &Path,
    target_dir: &Path,
    kernel: &Path,
    modules: &[esp::Module],
    efi: &Path,
) -> Result<(Vec<u8>, u32), String> {
    let release = SLOT_RELEASE_SEQ + 1;
    let record = image::slots::slot_record(root, release)?;
    let mut files: Vec<&esp::Module> = modules.iter().filter(|m| m.path != "kai.slot").collect();
    let slot_file = esp::Module {
        path: "kai.slot".into(),
        bytes: record.clone().into_bytes(),
    };
    files.push(&slot_file);
    let staging = target_dir.join("esp-update");
    esp::make_esp(&staging, kernel, &files, efi, "")?;
    Ok((image::slots::slot_image(&staging, &record)?, release))
}

/// Builds `target/update.img` for a case: this build as a slot image of the
/// next release, signed by the run's test root (`image/update_disk.rs`). The
/// image's bundle carries its own `kai.slot` naming that release.
pub(crate) fn update_disk(
    root: &Path,
    target_dir: &Path,
    kernel: &Path,
    modules: &[esp::Module],
    efi: &Path,
    update: cases::UpdateDisk,
) -> Result<String, String> {
    let (mut image, release) = update_image(root, target_dir, kernel, modules, efi)?;
    image.resize(image.len() + 512 * usize::from(update.uneven), 0);
    let disk =
        image::update_disk::TestRoot::signed_disk(&image, release, update.tampered, update.signer)?;
    let name = String::from("update.img");
    fs::write(target_dir.join(&name), &disk).map_err(|e| format!("cannot write {name}: {e}"))?;
    let flipped = [", one byte of the image flipped", ""][usize::from(!update.tampered)];
    println!(
        "xtask: update disk {name}: release {release}, {} bytes of image, {} MiB, signed under {:?}{flipped}",
        image.len(),
        disk.len() >> 20,
        update.signer,
    );
    Ok(name)
}

/// Makes `image` a kaifs data disk holding `model` in its model store, named
/// in `kai-modelhint 1`: `xtask fs mkfs`, then `import --model`.
/// Flips one bit of the FNV-1a64 in `image`'s `kai-modelhint 1` and writes it
/// back with a good CRC, so the hint still verifies but no longer matches the
/// model (`xtask fs mkfs` puts `kai-data` at 2 MiB).
pub(crate) fn damage_hint(image: &std::path::Path) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let shown = image.display();
    let at = SeekFrom::Start(blklog::DATA_FIRST * 512);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(|e| format!("{shown}: {e}"))?;
    let mut sector = [0u8; 512];
    file.seek(at)
        .and_then(|_| file.read_exact(&mut sector))
        .map_err(|e| format!("{shown}: {e}"))?;
    let mut hint = kabi::modelhint::Hint::decode(&sector)
        .ok_or_else(|| format!("{shown}: no kai-modelhint 1 at kai-data's start"))?;
    hint.fnv ^= 1;
    file.seek(at)
        .and_then(|_| file.write_all(&hint.encode()))
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("{shown}: {e}"))
}

pub(crate) fn store_model(image: &std::path::Path, model: &std::path::Path) -> Result<(), String> {
    let image = image.display().to_string();
    let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    fs_tool::main(&words(&["mkfs", &image, "--size", "64", "--force"]))?;
    let model = model.display().to_string();
    fs_tool::main(&words(&["import", "--model", &image, &model]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_damaged_hint_still_verifies_but_names_another_fnv() {
        use std::io::{Read, Seek, SeekFrom};
        let dir = std::env::temp_dir().join(format!("kai-hint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (image, model) = (dir.join("kai-data.img"), dir.join("model.gguf"));
        let mut bytes = b"GGUF\x03\0\0\0".to_vec();
        bytes.resize(100_000, 7);
        std::fs::write(&model, &bytes).unwrap();
        super::store_model(&image, &model).unwrap();
        let hint = || {
            let mut file = std::fs::File::open(&image).unwrap();
            let mut sector = [0u8; 512];
            file.seek(SeekFrom::Start(super::blklog::DATA_FIRST * 512))
                .unwrap();
            file.read_exact(&mut sector).unwrap();
            kabi::modelhint::Hint::decode(&sector).unwrap()
        };
        let good = hint();
        assert_eq!(good.len, 100_000);
        super::damage_hint(&image).unwrap();
        let bad = hint();
        assert_eq!(bad.fnv, good.fnv ^ 1);
        assert_eq!((bad.len, bad.sha256), (good.len, good.sha256));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
