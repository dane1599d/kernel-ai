//! `mkfs`, `import`, `ls`, `cat` and `check` on a data disk image.
//!
//! Paths are the guest's (ADR 0004 §5): `/home/...` is the `home` volume,
//! `/system/models/...` the `models` volume, and the rest of `/system/...` the
//! `system` volume. `/` lists the three.

use super::disk::{self, FileDev};
use kaifs::check::{Cache, check};
use kaifs::fs::{HOME, MODELS, SYSTEM};
use kaifs::ns_check::check_namespace;
use kaifs::repair_fs::{mount_to_repair, repair_fs};
use kaifs::{Error, FileType, Fs, ROOT};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Where each volume is mounted, longest first, so `/system/models` wins over
/// `/system`.
pub const MOUNTS: [(&str, u64); 3] = [
    ("/system/models", MODELS),
    ("/system", SYSTEM),
    ("/home", HOME),
];

/// The folders every data disk starts with (P2 plan decision 5), by volume.
const FOLDERS: [(u64, &str); 5] = [
    (HOME, "/user"),
    (HOME, "/.trash"),
    (SYSTEM, "/settings"),
    (SYSTEM, "/state"),
    (SYSTEM, "/pkg"),
];

/// Pieces a file is read and written in, so a large one never sits in memory.
const PIECE: usize = 1 << 20;

/// The volume a guest path lies in, and the path inside it.
pub fn split(path: &str) -> Result<(u64, String), String> {
    if !path.starts_with('/') {
        return Err(format!("{path}: a guest path starts with /home or /system"));
    }
    let path = match path.len() > 1 {
        true => path.trim_end_matches('/'),
        false => path,
    };
    for (mount, vol) in MOUNTS {
        if path == mount {
            return Ok((vol, "/".into()));
        }
        if let Some(rest) = path.strip_prefix(mount).filter(|r| r.starts_with('/')) {
            return Ok((vol, rest.into()));
        }
    }
    Err(format!(
        "{path}: not under /home, /system or /system/models"
    ))
}

/// `n` of `what`, plural when it is not one.
fn count(n: u64, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

fn fail(what: &str) -> impl Fn(Error) -> String + '_ {
    move |e| format!("{what}: {e:?}")
}

pub fn open(image: &Path, write: bool) -> Result<Fs<FileDev>, String> {
    let dev = FileDev::open(image, write)?;
    Fs::mount(dev).map_err(fail(&image.display().to_string()))
}

/// `mkfs`: a new data disk of `bytes` at `image`, its three volumes and their
/// first folders.
pub fn mkfs(image: &Path, bytes: u64, label: &str, force: bool) -> Result<String, String> {
    let region = disk::create(image, bytes, label, force)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(|e| format!("{}: {e}", image.display()))?;
    let dev = FileDev::new(file, region);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut seed = Sha256::new();
    seed.update(label.as_bytes());
    seed.update(now.to_le_bytes());
    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(&seed.finalize()[..16]);
    let mut fs = Fs::mkfs(dev, uuid, label).map_err(fail("mkfs"))?;
    fs.clock = now;
    for (vol, path) in FOLDERS {
        fs.set_volume(vol).map_err(fail(path))?;
        fs.mkdir(ROOT, &path[1..]).map_err(fail(path))?;
    }
    let generation = fs.commit().map_err(fail("commit"))?;
    Ok(format!(
        "{}: kaifs format 1, {} MiB of kai-data at 2 MiB, generation {generation}, volumes system, home and models",
        image.display(),
        (region.blocks * 4096) >> 20
    ))
}

/// The inode at an inner path of the active volume.
fn resolve(fs: &mut Fs<FileDev>, inner: &str, shown: &str) -> Result<u64, String> {
    fs.resolve(inner).map_err(|e| match e {
        Error::NotFound => format!("{shown}: no such file or folder"),
        e => format!("{shown}: {e:?}"),
    })
}

/// The name a host file has on the disk.
fn name_of(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{}: no name", path.display()))?;
    name.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{}: a name that is not UTF-8", path.display()))
}

/// Copies the host file `host` over the disk file `ino`, a piece at a time.
fn copy_in(fs: &mut Fs<FileDev>, host: &Path, ino: u64) -> Result<u64, String> {
    let shown = host.display();
    let mut file = File::open(host).map_err(|e| format!("{shown}: {e}"))?;
    fs.truncate(ino, 0).map_err(fail(&shown.to_string()))?;
    let mut piece = vec![0u8; PIECE];
    let mut at = 0u64;
    loop {
        let n = file.read(&mut piece).map_err(|e| format!("{shown}: {e}"))?;
        if n == 0 {
            return Ok(at);
        }
        fs.write_at(ino, at, &piece[..n])
            .map_err(fail(&shown.to_string()))?;
        at += n as u64;
    }
}

/// Copies `host`, a file or a folder and all below it, to `name` in the disk
/// folder `dir`. Returns the files and bytes it copied.
fn import_into(
    fs: &mut Fs<FileDev>,
    host: &Path,
    dir: u64,
    name: &str,
) -> Result<(u64, u64), String> {
    let meta = std::fs::symlink_metadata(host).map_err(|e| format!("{}: {e}", host.display()))?;
    let existing = fs.lookup(dir, name).map_err(fail(name))?;
    if meta.is_dir() {
        let folder = match existing {
            Some(e) if e.kind == FileType::Dir => e.ino,
            Some(_) => return Err(format!("{name}: a file on the disk where a folder goes")),
            None => fs.mkdir(dir, name).map_err(fail(name))?,
        };
        let mut children: Vec<_> = std::fs::read_dir(host)
            .map_err(|e| format!("{}: {e}", host.display()))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{}: {e}", host.display()))?;
        children.sort_by_key(|c| c.file_name());
        let (mut files, mut bytes) = (0, 0);
        for child in children {
            let path = child.path();
            let (f, b) = import_into(fs, &path, folder, &name_of(&path)?)?;
            (files, bytes) = (files + f, bytes + b);
        }
        return Ok((files, bytes));
    }
    if !meta.is_file() {
        return Err(format!(
            "{}: neither a file nor a folder (links are not imported)",
            host.display()
        ));
    }
    let ino = match existing {
        Some(e) if e.kind == FileType::File => e.ino,
        Some(_) => return Err(format!("{name}: a folder on the disk where a file goes")),
        None => fs.create(dir, name).map_err(fail(name))?,
    };
    Ok((1, copy_in(fs, host, ino)?))
}

/// `import`: copies the host file or folder `host` to the guest path `to`, in
/// one commit, replacing files that are there.
pub fn import(image: &Path, host: &Path, to: &str) -> Result<String, String> {
    let (vol, inner) = split(to)?;
    let mut fs = open(image, true)?;
    fs.set_volume(vol).map_err(fail(to))?;
    let (parent, name) = match inner.rfind('/') {
        _ if inner == "/" => {
            return Err(format!(
                "{to}: import into a folder below the volume's root"
            ));
        }
        Some(0) => ("/", &inner[1..]),
        Some(at) => (&inner[..at], &inner[at + 1..]),
        None => unreachable!("inner paths start with /"),
    };
    let dir = resolve(&mut fs, parent, to)?;
    let (files, bytes) = import_into(&mut fs, host, dir, name)?;
    let generation = fs.commit().map_err(fail("commit"))?;
    Ok(format!(
        "{to}: {}, {}, in generation {generation}",
        count(files, "file"),
        count(bytes, "byte")
    ))
}

/// `ls`: the names in a guest folder, with their type and size.
pub fn ls(image: &Path, path: &str) -> Result<Vec<String>, String> {
    if path == "/" {
        return Ok(vec![
            "home/".into(),
            "system/".into(),
            "system/models/".into(),
        ]);
    }
    let (vol, inner) = split(path)?;
    let mut fs = open(image, false)?;
    fs.set_volume(vol).map_err(fail(path))?;
    let ino = resolve(&mut fs, &inner, path)?;
    let inode = fs.stat(ino).map_err(fail(path))?;
    if inode.kind == FileType::File {
        return Ok(vec![format!("{:>12}  {path}", inode.size)]);
    }
    let mut out = Vec::new();
    for listed in fs.list(ino, None, usize::MAX).map_err(fail(path))? {
        let child = fs
            .stat(listed.entry.ino)
            .map_err(fail(&listed.entry.name))?;
        out.push(match child.kind {
            FileType::Dir => format!("{:>12}  {}/", "", listed.entry.name),
            FileType::File => format!("{:>12}  {}", child.size, listed.entry.name),
        });
    }
    out.sort_by(|a, b| a[14..].cmp(&b[14..]));
    Ok(out)
}

/// `cat`: a guest file's bytes to `out`, a piece at a time.
pub fn cat(image: &Path, path: &str, out: &mut dyn Write) -> Result<u64, String> {
    let (vol, inner) = split(path)?;
    let mut fs = open(image, false)?;
    fs.set_volume(vol).map_err(fail(path))?;
    let ino = resolve(&mut fs, &inner, path)?;
    let size = fs.stat(ino).map_err(fail(path))?.size;
    let mut at = 0;
    while at < size {
        let piece = fs.read_at(ino, at, PIECE).map_err(fail(path))?;
        out.write_all(&piece).map_err(|e| format!("{path}: {e}"))?;
        at += piece.len() as u64;
    }
    Ok(size)
}

/// `check`: everything a mount reaches verifies, and every volume keeps the
/// namespace's rules. With `repair`, kaifs's whole-disk rules
/// ([`kaifs::repair_fs`]): a disk that does not mount is mounted from its newest
/// commit whose volume table reads, its allocator rebuilt; one that does not
/// check is repaired, space, history and each volume's namespace; and the
/// result is checked again.
pub fn check_image(image: &Path, fix: bool) -> Result<Vec<String>, String> {
    let shown = image.display().to_string();
    let (mut fs, mut lines) = match fix {
        true => mount_to_repair(FileDev::open(image, true)?).map_err(fail(&shown))?,
        false => (open(image, false)?, Vec::new()),
    };
    let mut cache = Cache::default();
    let found = check(&mut fs, &mut cache)
        .and_then(|r| check_namespace(&mut fs, &mut cache).map(|ns| (r, ns)));
    match found {
        Ok((report, ns)) => {
            lines.push(format!("generation {}: checks", report.generation));
            for (id, name) in fs.volumes() {
                let v = &ns.volumes[&id];
                lines.push(format!(
                    "  {name}: {}, {}, {}, {}",
                    count(v.live.files, "file"),
                    count(v.live.dirs, "folder"),
                    count(v.live.bytes, "byte"),
                    count(v.snaps.len() as u64, "snapshot")
                ));
            }
            lines.push(format!(
                "  blocks: {} reached, {} free, {} pending, {} held by nothing (the GC's)",
                report.reachable_blocks,
                report.free_blocks,
                report.pending_blocks,
                report.leaked_blocks
            ));
            Ok(lines)
        }
        Err(why) if !fix => Err(format!("{shown}: does not check: {why}")),
        Err(why) => {
            lines.push(format!("does not check: {why}"));
            let fixed = repair_fs(&mut fs).map_err(fail("repair"))?;
            lines.extend(fixed.iter().map(|line| format!("  {line}")));
            drop(fs);
            lines.extend(check_image(image, false)?);
            Ok(lines)
        }
    }
}
