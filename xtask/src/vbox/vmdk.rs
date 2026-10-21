//! The model disk for VirtualBox without copying the model: a VMDK descriptor
//! whose extent is the GGUF file itself, read where it lies.
//!
//! A FLAT extent covers whole sectors, and a model file is rarely a whole number
//! of them (Qwen3-4B's ends 96 bytes into its last). So the descriptor maps every
//! whole sector from the file, and the last, partial one from a 512-byte tail file
//! next to the descriptor: the file's last bytes, then zeros. The guest sees the
//! file rounded up to whole sectors, as QEMU shows it. VirtualBox rejects an
//! absolute extent path, so the model is named relative to the descriptor, and
//! must be on the same drive.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

const SECTOR: u64 = 512;

/// The descriptor's text, and the tail sector to write next to it if the file
/// ends partway into one.
pub struct Vmdk {
    pub descriptor: String,
    pub tail: Option<[u8; 512]>,
}

/// The descriptor for the `len`-byte file at `model`, whose partial last sector
/// (if any) is `last` and lives in `tail_name` next to the descriptor.
pub fn describe(model: &str, len: u64, tail_name: &str, last: &[u8]) -> Vmdk {
    let whole = len / SECTOR;
    let rest = (len % SECTOR) as usize;
    let mut extents = format!("RDONLY {whole} FLAT \"{model}\" 0\n");
    let tail = (rest != 0).then(|| {
        extents.push_str(&format!("RDONLY 1 FLAT \"{tail_name}\" 0\n"));
        let mut sector = [0u8; 512];
        sector[..rest].copy_from_slice(&last[..rest]);
        sector
    });
    let sectors = whole + u64::from(tail.is_some());
    // VirtualBox wants a CHS geometry; 16 heads and 63 sectors per track is what
    // it writes itself for an IDE-shaped disk.
    let cylinders = (sectors / (16 * 63)).clamp(1, 16383);
    let descriptor = format!(
        "# Disk DescriptorFile\n\
         version=1\n\
         CID=4b414931\n\
         parentCID=ffffffff\n\
         createType=\"monolithicFlat\"\n\
         \n\
         # Extent description\n\
         {extents}\
         \n\
         # The Disk Data Base\n\
         #DDB\n\
         \n\
         ddb.virtualHWVersion = \"4\"\n\
         ddb.adapterType=\"ide\"\n\
         ddb.geometry.cylinders=\"{cylinders}\"\n\
         ddb.geometry.heads=\"16\"\n\
         ddb.geometry.sectors=\"63\"\n"
    );
    Vmdk { descriptor, tail }
}

/// The extent lines of a descriptor: what it maps, whatever VirtualBox added.
pub fn extents(descriptor: &str) -> Vec<&str> {
    descriptor
        .lines()
        .filter(|line| line.starts_with("RDONLY ") || line.starts_with("RW "))
        .collect()
}

/// `target` as a path relative to the directory `from`, with `/` separators:
/// VirtualBox takes only relative extent paths. None when no relative path exists
/// (another drive).
pub fn relative(from: &Path, target: &Path) -> Option<String> {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = target.components().collect();
    // The drive (and root) must match.
    let roots = |parts: &[Component]| {
        parts
            .iter()
            .take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
            .count()
    };
    let (a, b) = (roots(&from), roots(&to));
    if a != b || from[..a] != to[..b] {
        return None;
    }
    let common = from.iter().zip(&to).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); from.len() - common];
    for part in &to[common..] {
        parts.push(part.as_os_str().to_str()?.to_string());
    }
    Some(parts.join("/"))
}

/// The descriptor, to be written in `dir`, for the model file at `model`, reading
/// its last partial sector.
pub fn for_file(dir: &Path, model: &Path, tail_name: &str) -> Result<Vmdk, String> {
    let shown = model.display();
    let name = relative(dir, model).ok_or_else(|| {
        format!(
            "VirtualBox can read the model only by a path relative to {}, and {shown} is on another drive: set KAI_VBOX_DIR to a folder on the model's drive",
            dir.display()
        )
    })?;
    if name.contains('"') {
        return Err(format!(
            "{shown} has a quote in it, which a VMDK cannot name"
        ));
    }
    let mut file = File::open(model).map_err(|e| format!("cannot open {shown}: {e}"))?;
    let len = file
        .metadata()
        .map_err(|e| format!("cannot read {shown}: {e}"))?
        .len();
    let rest = len % SECTOR;
    let mut last = vec![0u8; rest as usize];
    file.seek(SeekFrom::Start(len - rest))
        .and_then(|_| file.read_exact(&mut last))
        .map_err(|e| format!("cannot read the end of {shown}: {e}"))?;
    Ok(describe(&name, len, tail_name, &last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_last_sector_comes_from_the_tail_file() {
        let last: Vec<u8> = (1..=96).collect();
        let vmdk = describe(
            r"D:\kai-models\q.gguf",
            2_497_281_120,
            "kai-model.tail",
            &last,
        );
        assert!(
            vmdk.descriptor
                .contains("RDONLY 4877502 FLAT \"D:\\kai-models\\q.gguf\" 0\n")
        );
        assert!(
            vmdk.descriptor
                .contains("RDONLY 1 FLAT \"kai-model.tail\" 0\n")
        );
        assert!(
            vmdk.descriptor
                .starts_with("# Disk DescriptorFile\nversion=1\n")
        );
        let tail = vmdk.tail.unwrap();
        assert_eq!(&tail[..96], &last[..]);
        assert!(tail[96..].iter().all(|&b| b == 0));
        let registered = format!("{}ddb.uuid.image=\"1b46\"\n", vmdk.descriptor);
        assert_eq!(extents(&registered), extents(&vmdk.descriptor));
        assert_eq!(extents(&vmdk.descriptor).len(), 2);
        // 4,877,503 sectors / 1,008 per cylinder.
        assert!(vmdk.descriptor.contains("ddb.geometry.cylinders=\"4838\""));
    }

    #[test]
    fn a_whole_sector_file_needs_no_tail() {
        let vmdk = describe("D:/m.gguf", 1024, "t", &[]);
        assert!(vmdk.tail.is_none());
        assert!(vmdk.descriptor.contains("RDONLY 2 FLAT \"D:/m.gguf\" 0\n"));
        assert!(!vmdk.descriptor.contains("\"t\""));
    }

    #[test]
    fn the_tail_holds_the_files_last_bytes() {
        let dir = std::env::temp_dir().join(format!("kai-vmdk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.gguf");
        let bytes: Vec<u8> = (0..1300u32).map(|i| i as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let vmdk = for_file(&dir, &path, "m.tail").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let tail = vmdk.tail.unwrap();
        assert_eq!(&tail[..276], &bytes[1024..]);
        assert!(vmdk.descriptor.contains("RDONLY 2 FLAT \"m.gguf\" 0"));
    }

    #[test]
    fn paths_are_made_relative_on_one_drive_only() {
        let rel = |a: &str, b: &str| relative(Path::new(a), Path::new(b));
        if cfg!(windows) {
            assert_eq!(
                rel(r"D:\kai-vbox", r"D:\kai-models\q.gguf").as_deref(),
                Some("../kai-models/q.gguf")
            );
            assert_eq!(rel(r"D:\kai-vbox", r"E:\q.gguf"), None);
        }
        assert_eq!(rel("/a/b", "/a/b/c.gguf").as_deref(), Some("c.gguf"));
        assert_eq!(rel("/a/b", "/x/y.gguf").as_deref(), Some("../../x/y.gguf"));
    }
}
