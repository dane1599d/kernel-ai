//! `cargo xtask image-hashes list IMAGE` and `compare LABEL=LISTING...`: the
//! release image's hashes by partition and by file, for `repro.yml`. Three
//! builds of one commit (two runners, and the offline rebuild from the source
//! tarball) must be the same bytes; when they are not, a bare image SHA-256
//! says nothing about where. `list` prints one line per piece of the image,
//! `compare` reads each build's listing and names every partition and file
//! whose line differs or is missing, and fails.
//!
//! The listing, one `image: ...` line each, in this order:
//!   `image: size BYTES`
//!   `image: outside SHA`         the bytes in no partition (MBR, GPT, gaps)
//!   `image: part NAME FIRST SECTORS SHA`
//!   `image: file PART PATH LEN SHA`   each file of a FAT32 partition
//!   `image: module PART FILE#MODULE LEN SHA`   each module of a `.bundle` file
//!
//! `kai-data` is not FAT32: it has its `part` line alone. Boot modules travel
//! packed in `boot/kai.bundle`, so a `module` line names the one inside it that
//! differs. A file or module path may hold spaces (`compare` takes the value
//! from the end of the line); a partition name may not (the release image's are
//! `kai-r`, `kai-a`, `kai-b` and `kai-data`).

use crate::sha256_hex;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const USAGE: &str = "usage: cargo xtask image-hashes list IMAGE
       cargo xtask image-hashes compare LABEL=LISTING LABEL=LISTING...";
const SECTOR: u64 = 512;

pub fn main(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 2 => {
            for line in list(Path::new(&args[1]))? {
                println!("{line}");
            }
            Ok(())
        }
        Some("compare") if args.len() >= 3 => {
            let mut listings = Vec::new();
            for arg in &args[1..] {
                let (label, path) = arg.split_once('=').ok_or(USAGE)?;
                let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                listings.push((label.to_string(), text));
            }
            let report = compare(&listings);
            match report.is_empty() {
                true => {
                    println!("image-hashes: all {} listings agree", listings.len());
                    Ok(())
                }
                false => {
                    for line in &report {
                        eprintln!("{line}");
                    }
                    Err(format!("{} pieces of the image differ", report.len()))
                }
            }
        }
        _ => Err(USAGE.into()),
    }
}

/// Each piece of the image at `path`, as the `image:` lines above.
pub fn list(path: &Path) -> Result<Vec<String>, String> {
    let shown = path.display();
    let mut file = fs::File::open(path).map_err(|e| format!("{shown}: {e}"))?;
    let size = file.metadata().map_err(|e| format!("{shown}: {e}"))?.len();
    let table = diskmap::read(&mut FileDisk(file.try_clone().map_err(|e| e.to_string())?))
        .map_err(|e| format!("{shown}: no partition table ({e:?})"))?;
    let mut parts = table.partitions.clone();
    parts.sort_by_key(|p| p.first);
    let mut lines = vec![format!("image: size {size}")];
    // Everything outside the partitions, hashed in order.
    let mut outside = Sha256::new();
    let mut at = 0;
    for part in &parts {
        hash_range(&mut file, at, part.first * SECTOR, &mut outside)?;
        at = (part.last + 1) * SECTOR;
    }
    hash_range(&mut file, at, size, &mut outside)?;
    lines.push(format!(
        "image: outside {}",
        hex(outside.finalize().as_slice())
    ));
    for part in &parts {
        let (from, to) = (part.first * SECTOR, (part.last + 1) * SECTOR);
        let mut hash = Sha256::new();
        hash_range(&mut file, from, to, &mut hash)?;
        lines.push(format!(
            "image: part {} {} {} {}",
            part.name,
            part.first,
            part.sectors(),
            hex(hash.finalize().as_slice())
        ));
        lines.extend(file_lines(&part.name, &fat32_files(&mut file, from)?));
    }
    Ok(lines)
}

/// A partition's `file` lines, each followed by its bundle's `module` lines.
fn file_lines(part: &str, files: &[FatFile]) -> Vec<String> {
    let mut lines = Vec::new();
    for file in files {
        let (path, len, sha) = (&file.path, file.len, &file.sha);
        lines.push(format!("image: file {part} {path} {len} {sha}"));
        for (module, len, sha) in &file.modules {
            lines.push(format!("image: module {part} {path}#{module} {len} {sha}"));
        }
    }
    lines
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `file`'s bytes from `from` to `to` into `hash`, a piece at a time.
fn hash_range<R: Read + Seek>(
    file: &mut R,
    from: u64,
    to: u64,
    hash: &mut Sha256,
) -> Result<(), String> {
    let mut piece = vec![0u8; 1 << 20];
    file.seek(SeekFrom::Start(from))
        .map_err(|e| e.to_string())?;
    let mut left = to.saturating_sub(from);
    while left > 0 {
        let n = left.min(piece.len() as u64) as usize;
        file.read_exact(&mut piece[..n])
            .map_err(|e| format!("read at {}: {e}", to - left))?;
        hash.update(&piece[..n]);
        left -= n as u64;
    }
    Ok(())
}

/// One file of a FAT32 volume: its path, length and SHA-256, and, for a
/// `kai-bundle 1` (a name ending `.bundle` that parses), each module's path,
/// length and SHA-256 in path order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FatFile {
    path: String,
    len: u32,
    sha: String,
    modules: Vec<(String, usize, String)>,
}

/// A bundle's modules as (path, length, SHA-256), in path order; none if the
/// bytes are not a bundle (its `file` line still shows any difference).
fn bundle_modules(bytes: &[u8]) -> Vec<(String, usize, String)> {
    let Ok(bundle) = kaibundle::parse(bytes) else {
        return Vec::new();
    };
    let mut modules: Vec<_> = bundle
        .entries()
        .map(|e| (e.path.to_string(), e.data.len(), sha256_hex(e.data)))
        .collect();
    modules.sort();
    modules
}

/// Every file of the FAT32 volume at byte `base` of `disk`, in path order; none
/// if the partition is not FAT32.
fn fat32_files<R: Read + Seek>(disk: &mut R, base: u64) -> Result<Vec<FatFile>, String> {
    let mut boot = [0u8; 512];
    disk.seek(SeekFrom::Start(base))
        .map_err(|e| e.to_string())?;
    if disk.read_exact(&mut boot).is_err() || &boot[82..87] != b"FAT32" {
        return Ok(Vec::new());
    }
    let u16_at = |at: usize| u16::from_le_bytes([boot[at], boot[at + 1]]) as u64;
    let u32_at = |at: usize| u32::from_le_bytes(boot[at..at + 4].try_into().unwrap()) as u64;
    let (bytes_per_sector, per_cluster, reserved) = (u16_at(11), boot[13] as u64, u16_at(14));
    let (fats, fat_sectors, root) = (boot[16] as u64, u32_at(36), u32_at(44));
    if bytes_per_sector == 0 || per_cluster == 0 || fats == 0 {
        return Err("a FAT32 volume with an empty geometry".into());
    }
    let cluster_bytes = bytes_per_sector * per_cluster;
    let data = base + (reserved + fats * fat_sectors) * bytes_per_sector;
    let mut fat = vec![0u8; (fat_sectors * bytes_per_sector) as usize];
    disk.seek(SeekFrom::Start(base + reserved * bytes_per_sector))
        .and_then(|_| disk.read_exact(&mut fat))
        .map_err(|e| format!("FAT: {e}"))?;
    let next = |cluster: u64| -> u64 {
        let at = cluster as usize * 4;
        match fat.get(at..at + 4) {
            Some(b) => u32::from_le_bytes(b.try_into().unwrap()) as u64 & 0x0FFF_FFFF,
            None => 0x0FFF_FFFF,
        }
    };
    // The bytes of the chain starting at `first`, `len` of them (`None`: all).
    let read_chain = |disk: &mut R, first: u64, len: Option<u64>| -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let (mut cluster, mut guard) = (first, 0u64);
        while (2..0x0FFF_FFF8).contains(&cluster) {
            if len.is_some_and(|len| out.len() as u64 >= len) {
                break;
            }
            guard += 1;
            if guard > (1 << 24) {
                return Err("a FAT chain that does not end".into());
            }
            let mut buf = vec![0u8; cluster_bytes as usize];
            disk.seek(SeekFrom::Start(data + (cluster - 2) * cluster_bytes))
                .and_then(|_| disk.read_exact(&mut buf))
                .map_err(|e| format!("cluster {cluster}: {e}"))?;
            out.extend_from_slice(&buf);
            cluster = next(cluster);
        }
        if let Some(len) = len {
            out.truncate(len as usize);
        }
        Ok(out)
    };
    let mut files = Vec::new();
    let mut dirs = vec![(String::new(), root)];
    while let Some((prefix, cluster)) = dirs.pop() {
        let bytes = read_chain(disk, cluster, None)?;
        let mut long: Vec<(u8, String)> = Vec::new();
        for entry in bytes.as_chunks::<32>().0 {
            if entry[0] == 0 {
                break;
            }
            if entry[0] == 0xE5 {
                long.clear();
                continue;
            }
            let attr = entry[11];
            if attr == 0x0F {
                let mut units = Vec::new();
                for at in (1..11)
                    .step_by(2)
                    .chain((14..26).step_by(2))
                    .chain((28..32).step_by(2))
                {
                    units.push(u16::from_le_bytes([entry[at], entry[at + 1]]));
                }
                let end = units
                    .iter()
                    .position(|&u| u == 0 || u == 0xFFFF)
                    .unwrap_or(units.len());
                long.push((entry[0] & 0x1F, String::from_utf16_lossy(&units[..end])));
                continue;
            }
            let name = match long.is_empty() {
                false => {
                    long.sort_by_key(|(order, _)| *order);
                    long.iter()
                        .map(|(_, part)| part.as_str())
                        .collect::<String>()
                }
                true => short_name(entry),
            };
            long.clear();
            if attr & 0x08 != 0 || name == "." || name == ".." {
                continue;
            }
            let first = (u16::from_le_bytes([entry[20], entry[21]]) as u64) << 16
                | u16::from_le_bytes([entry[26], entry[27]]) as u64;
            let len = u32::from_le_bytes(entry[28..32].try_into().unwrap());
            let path = format!("{prefix}{name}");
            match attr & 0x10 != 0 {
                true => dirs.push((format!("{path}/"), first)),
                false => {
                    let bytes = read_chain(disk, first, Some(len as u64))?;
                    let modules = match path.ends_with(".bundle") {
                        true => bundle_modules(&bytes),
                        false => Vec::new(),
                    };
                    files.push(FatFile {
                        sha: sha256_hex(&bytes),
                        path,
                        len,
                        modules,
                    });
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

/// A 8.3 entry's name: `BOOTX64.EFI`.
fn short_name(entry: &[u8]) -> String {
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim_end().to_string();
    match text(&entry[8..11]) {
        ext if ext.is_empty() => text(&entry[..8]),
        ext => format!("{}.{ext}", text(&entry[..8])),
    }
}

/// The pieces that differ between the listings: each `part`, `file`, `outside`
/// `module` or `size` line whose rest is not the same in all of them, or that
/// one lacks. Partitions come before files, files before modules. Empty when
/// all agree.
pub fn compare(listings: &[(String, String)]) -> Vec<String> {
    let parsed: Vec<(&str, BTreeMap<String, String>)> = listings
        .iter()
        .map(|(label, text)| (label.as_str(), keyed(text)))
        .collect();
    let mut keys: Vec<&String> = parsed.iter().flat_map(|(_, map)| map.keys()).collect();
    keys.sort_by_key(|key| order(key));
    keys.dedup();
    let mut report = Vec::new();
    for key in keys {
        let values: Vec<Option<&String>> = parsed.iter().map(|(_, map)| map.get(key)).collect();
        if values.iter().all(|v| *v == values[0]) {
            continue;
        }
        let what = match key.split_once(' ') {
            Some(("part", name)) => format!("partition {name}"),
            Some(("file", rest)) => match rest.split_once(' ') {
                Some((part, path)) => format!("file {path} in partition {part}"),
                None => key.clone(),
            },
            Some(("module", rest)) => match rest.split_once(' ') {
                Some((part, inside)) => match inside.split_once(".bundle#") {
                    Some((file, module)) => {
                        format!("module {module} in {file}.bundle in partition {part}")
                    }
                    None => key.clone(),
                },
                None => key.clone(),
            },
            Some(("outside", _)) | None => "the bytes outside the partitions".into(),
            Some(_) => key.clone(),
        };
        let mut line = format!("DIFFERS: {what}");
        for ((label, _), value) in parsed.iter().zip(&values) {
            line.push_str(&format!(
                "\n    {label}: {}",
                value.map_or("(missing)", |v| v)
            ));
        }
        report.push(line);
    }
    report
}

/// A listing's lines by key (all but the value) to value: a `part` line's key
/// is `part NAME` (the value is first sector, sectors and hash), a `file`
/// line's `file PART PATH` and a `module` line's `module PART FILE#MODULE`
/// (length and hash); `size` and `outside` have one value. The value is the
/// last words, so a name with spaces stays whole in the key.
fn keyed(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines().filter_map(|l| l.strip_prefix("image: ")) {
        let words: Vec<&str> = line.split(' ').collect();
        let (least, value) = match words.first() {
            Some(&"part") => (5, 3),
            Some(&"file" | &"module") => (5, 2),
            Some(&"size" | &"outside") if words.len() == 2 => (2, 1),
            _ => continue,
        };
        if words.len() < least {
            continue;
        }
        let split = words.len() - value;
        map.insert(words[..split].join(" "), words[split..].join(" "));
    }
    map
}

/// Sorts `size`, `outside`, partitions, then files.
fn order(key: &str) -> (u8, String) {
    let rank = match key.split(' ').next() {
        Some("size") => 0,
        Some("outside") => 1,
        Some("part") => 2,
        Some("file") => 3,
        _ => 4,
    };
    (rank, key.to_string())
}

struct FileDisk(fs::File);

impl diskmap::Disk for FileDisk {
    fn sectors(&self) -> u64 {
        self.0.metadata().map_or(0, |m| m.len() / SECTOR)
    }
    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
        self.0
            .seek(SeekFrom::Start(lba * SECTOR))
            .and_then(|_| self.0.read_exact(out))
            .map_err(|_| diskmap::Error::Read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::fat32::{Node, volume};
    use std::io::Cursor;

    fn file(name: &str, bytes: &[u8]) -> Node {
        Node::File {
            name: name.into(),
            bytes: bytes.to_vec(),
        }
    }

    fn tree(kernel: &[u8]) -> Vec<Node> {
        tree_with(kernel, &[7; 5000], "boot")
    }

    fn tree_with(kernel: &[u8], bundle: &[u8], dir: &str) -> Vec<Node> {
        vec![
            Node::Dir {
                name: dir.into(),
                children: vec![file("kernel", kernel), file("kai.bundle", bundle)],
            },
            file("NOTICES.txt", b"notices"),
        ]
    }

    fn bundle(build_time: &[u8]) -> Vec<u8> {
        kaibundle::build(&[("build-time.txt", build_time), ("init", b"init's bytes")]).unwrap()
    }

    /// The FAT32 reader finds every file, long names and subdirectories
    /// included, with the hash of its bytes.
    #[test]
    fn the_fat32_walk_lists_each_file_with_its_hash() {
        let bytes = volume(70_000, 0, 1, &tree(b"kernel one")).unwrap();
        let files = fat32_files(&mut Cursor::new(bytes), 0).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["NOTICES.txt", "boot/kai.bundle", "boot/kernel"]);
        assert_eq!(files[2].len, 10);
        assert_eq!(files[2].sha, sha256_hex(b"kernel one"));
        assert_eq!(files[1].sha, sha256_hex(&[7; 5000]));
        // Not a bundle's bytes: the file alone, no modules.
        assert!(files.iter().all(|f| f.modules.is_empty()));
    }

    #[test]
    fn a_partition_that_is_not_fat32_has_no_files() {
        assert!(
            fat32_files(&mut Cursor::new(vec![0u8; 4096]), 0)
                .unwrap()
                .is_empty()
        );
    }

    fn listing(kernel: &[u8], part: &str) -> String {
        listing_of(&tree(kernel), part)
    }

    fn listing_of(tree: &[Node], part: &str) -> String {
        let bytes = volume(70_000, 0, 1, tree).unwrap();
        let mut lines = vec![
            "image: size 100".to_string(),
            "image: outside aa".to_string(),
        ];
        lines.push(format!(
            "image: part kai-a 2048 70000 {}",
            sha256_hex(&bytes)
        ));
        let files = fat32_files(&mut Cursor::new(bytes), 0).unwrap();
        lines.extend(file_lines(part, &files));
        lines.join("\n")
    }

    /// One nondeterministic byte in one file: the report names the partition
    /// and that file, nothing else, with each build's value.
    #[test]
    fn a_difference_names_the_partition_and_the_file() {
        let same = listing(b"kernel one", "kai-a");
        let other = listing(b"kernel two", "kai-a");
        let agree = [
            ("a".to_string(), same.clone()),
            ("b".to_string(), same.clone()),
        ];
        assert!(compare(&agree).is_empty());
        let report = compare(&[
            ("a".to_string(), same.clone()),
            ("b".to_string(), same),
            ("offline".to_string(), other),
        ]);
        assert_eq!(report.len(), 2, "{report:#?}");
        assert!(report[0].starts_with("DIFFERS: partition kai-a"));
        assert!(report[1].starts_with("DIFFERS: file boot/kernel in partition kai-a"));
        assert!(report[1].contains("offline: 10 "));
    }

    #[test]
    fn a_piece_only_one_build_has_is_missing_in_the_others() {
        let full = listing(b"kernel one", "kai-a");
        let short: String = full
            .lines()
            .filter(|l| !l.contains("NOTICES"))
            .collect::<Vec<_>>()
            .join("\n");
        let report = compare(&[("a".to_string(), full), ("b".to_string(), short)]);
        assert_eq!(report.len(), 1);
        assert!(report[0].contains("file NOTICES.txt in partition kai-a"));
        assert!(report[0].contains("b: (missing)"));
    }

    /// A bundle's modules are listed, and one that differs is named inside it:
    /// the bundle, the file holding it and the partition are named too.
    #[test]
    fn a_difference_inside_the_bundle_names_the_module() {
        let one = listing_of(&tree_with(b"kernel", &bundle(b"1"), "boot"), "kai-a");
        let two = listing_of(&tree_with(b"kernel", &bundle(b"2"), "boot"), "kai-a");
        assert!(one.contains(&format!(
            "image: module kai-a boot/kai.bundle#init 12 {}",
            sha256_hex(b"init's bytes")
        )));
        let report = compare(&[("a".to_string(), one), ("b".to_string(), two)]);
        let named: Vec<&str> = report.iter().map(|r| r.lines().next().unwrap()).collect();
        assert_eq!(
            named,
            [
                "DIFFERS: partition kai-a",
                "DIFFERS: file boot/kai.bundle in partition kai-a",
                "DIFFERS: module build-time.txt in boot/kai.bundle in partition kai-a",
            ]
        );
        assert!(report[2].contains(&format!("a: 1 {}", sha256_hex(b"1"))));
    }

    /// A path with spaces is one key: its line differs as one piece and its
    /// length and hash are the value, in the report whole.
    #[test]
    fn a_name_with_spaces_stays_whole() {
        let one = listing_of(&tree_with(b"kernel one", &[7; 50], "EFI Boot"), "kai-a");
        let two = listing_of(&tree_with(b"kernel two", &[7; 50], "EFI Boot"), "kai-a");
        assert!(one.contains("image: file kai-a EFI Boot/kernel 10 "));
        let report = compare(&[("a".to_string(), one), ("b".to_string(), two)]);
        let named: Vec<&str> = report.iter().map(|r| r.lines().next().unwrap()).collect();
        assert_eq!(
            named,
            [
                "DIFFERS: partition kai-a",
                "DIFFERS: file EFI Boot/kernel in partition kai-a"
            ]
        );
        assert!(report[1].contains(&format!("b: 10 {}", sha256_hex(b"kernel two"))));
    }
}
