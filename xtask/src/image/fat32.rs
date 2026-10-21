//! A FAT32 volume written in one pass from a tree of files: what the ESP of a
//! bootable disk needs, deterministically (fixed timestamps, volume ID and name
//! order), so the same files always give the same bytes.
//!
//! Every name that is not already an upper-case 8.3 name gets long-name entries
//! and a numbered short alias (`KERNEL~1LIN`). Files and directories are laid out
//! in order, each in one contiguous cluster run.

use super::SECTOR;

/// One file or directory to write.
pub enum Node {
    File { name: String, bytes: Vec<u8> },
    Dir { name: String, children: Vec<Node> },
}

impl Node {
    fn name(&self) -> &str {
        match self {
            Node::File { name, .. } | Node::Dir { name, .. } => name,
        }
    }
}

const RESERVED_SECTORS: u32 = 32;
const FATS: u32 = 2;
const ROOT_CLUSTER: u32 = 2;
const END_OF_CHAIN: u32 = 0x0FFF_FFFF;
/// 2026-01-01, 00:00:00, in FAT's date and time encoding.
const DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1;
const TIME: u16 = 0;
const ATTR_DIR: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LFN: u8 = 0x0F;
/// UCS-2 characters one long-name entry holds.
const LFN_CHARS: usize = 13;

/// The volume's geometry for `sectors` sectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub sectors: u32,
    pub sectors_per_cluster: u32,
    pub fat_sectors: u32,
}

impl Geometry {
    /// Microsoft's FAT32 table: one-sector clusters up to 260 MB, 4 KiB up to 8 GB.
    /// Below 32.5 MiB a volume cannot have the 65,525 clusters FAT32 needs.
    pub fn for_sectors(sectors: u32) -> Result<Self, String> {
        let sectors_per_cluster = match sectors {
            0..=66_600 => return Err(format!("{sectors} sectors is too small for FAT32")),
            66_601..=532_480 => 1,
            532_481..=16_777_216 => 8,
            _ => return Err(format!("{sectors} sectors is too large for this writer")),
        };
        // From the FAT specification: the FAT size that covers the data area.
        let tmp1 = sectors - RESERVED_SECTORS;
        let tmp2 = (256 * sectors_per_cluster + FATS) / 2;
        let fat_sectors = tmp1.div_ceil(tmp2);
        Ok(Self {
            sectors,
            sectors_per_cluster,
            fat_sectors,
        })
    }

    fn data_start(&self) -> u32 {
        RESERVED_SECTORS + FATS * self.fat_sectors
    }

    pub fn clusters(&self) -> u32 {
        (self.sectors - self.data_start()) / self.sectors_per_cluster
    }

    fn cluster_bytes(&self) -> usize {
        (self.sectors_per_cluster * SECTOR) as usize
    }

    fn cluster_offset(&self, cluster: u32) -> usize {
        let sector = self.data_start() + (cluster - 2) * self.sectors_per_cluster;
        sector as usize * SECTOR as usize
    }
}

/// Whether `name` is already a valid upper-case 8.3 name.
fn is_short(name: &str) -> bool {
    let valid = |part: &str, max: usize| {
        !part.is_empty()
            && part.len() <= max
            && part
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b"_-~".contains(&b))
    };
    match name.split_once('.') {
        Some((base, ext)) => valid(base, 8) && valid(ext, 3),
        None => valid(name, 8),
    }
}

fn pad11(base: &str, ext: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    out[..base.len()].copy_from_slice(base.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    out
}

/// The short name for `name`, the `n`th alias in its directory if it needs one.
fn short_name(name: &str, n: usize) -> [u8; 11] {
    if is_short(name) {
        let (base, ext) = name.split_once('.').unwrap_or((name, ""));
        return pad11(base, ext);
    }
    let clean = |part: &str| -> String {
        part.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .map(|c| c.to_ascii_uppercase())
            .collect()
    };
    let (base, ext) = match name.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() => (clean(base), clean(ext)),
        _ => (clean(name), String::new()),
    };
    let tail = format!("~{n}");
    let keep = 8 - tail.len();
    let base: String = format!("{}{tail}", &base[..base.len().min(keep)]);
    pad11(&base, &ext[..ext.len().min(3)])
}

fn checksum(short: &[u8; 11]) -> u8 {
    short
        .iter()
        .fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b))
}

/// The 32-byte directory entries naming one child: long-name entries (last part
/// first), then the short entry.
fn entries_for(name: &str, short: [u8; 11], attr: u8, cluster: u32, size: u32) -> Vec<[u8; 32]> {
    let mut out = Vec::new();
    if !is_short(name) {
        let units: Vec<u16> = name.encode_utf16().collect();
        let parts = units.len().div_ceil(LFN_CHARS);
        let sum = checksum(&short);
        for part in (0..parts).rev() {
            let mut entry = [0u8; 32];
            entry[0] = (part + 1) as u8 | if part + 1 == parts { 0x40 } else { 0 };
            entry[11] = ATTR_LFN;
            entry[13] = sum;
            let start = part * LFN_CHARS;
            for i in 0..LFN_CHARS {
                // After the name: one 0x0000, then 0xFFFF padding.
                let unit = match start + i {
                    at if at < units.len() => units[at],
                    at if at == units.len() => 0,
                    _ => 0xFFFF,
                };
                let at = match i {
                    0..=4 => 1 + 2 * i,
                    5..=10 => 14 + 2 * (i - 5),
                    _ => 28 + 2 * (i - 11),
                };
                entry[at..at + 2].copy_from_slice(&unit.to_le_bytes());
            }
            out.push(entry);
        }
    }
    out.push(short_entry(short, attr, cluster, size));
    out
}

fn short_entry(short: [u8; 11], attr: u8, cluster: u32, size: u32) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[..11].copy_from_slice(&short);
    entry[11] = attr;
    entry[14..16].copy_from_slice(&TIME.to_le_bytes());
    entry[16..18].copy_from_slice(&DATE.to_le_bytes());
    entry[18..20].copy_from_slice(&DATE.to_le_bytes());
    entry[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    entry[22..24].copy_from_slice(&TIME.to_le_bytes());
    entry[24..26].copy_from_slice(&DATE.to_le_bytes());
    entry[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    entry[28..32].copy_from_slice(&size.to_le_bytes());
    entry
}

/// Bytes of directory entries for `children`, the dot entries included below the
/// root. Directories take one contiguous cluster run each, like files.
fn dir_bytes(children: &[Node], root: bool) -> usize {
    let names: usize = children
        .iter()
        .map(|node| match is_short(node.name()) {
            true => 1,
            false => node.name().encode_utf16().count().div_ceil(LFN_CHARS) + 1,
        })
        .sum();
    32 * (names + if root { 0 } else { 2 })
}

/// The writer's state: the volume bytes and the next free cluster.
struct Writer {
    geometry: Geometry,
    volume: Vec<u8>,
    fat: Vec<u32>,
    next: u32,
}

impl Writer {
    /// Takes a run of clusters for `bytes` bytes (at least one) and
    /// chains it in the FAT.
    fn allocate(&mut self, bytes: usize) -> Result<u32, String> {
        let count = bytes.div_ceil(self.geometry.cluster_bytes()).max(1) as u32;
        let first = self.next;
        if first + count > self.geometry.clusters() + 2 {
            return Err("the files do not fit the FAT32 volume".into());
        }
        for cluster in first..first + count {
            self.fat[cluster as usize] = match cluster + 1 == first + count {
                true => END_OF_CHAIN,
                false => cluster + 1,
            };
        }
        self.next += count;
        Ok(first)
    }

    fn put(&mut self, cluster: u32, bytes: &[u8]) {
        let at = self.geometry.cluster_offset(cluster);
        self.volume[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// Writes directory `children` at `cluster`, the start of its run, below
    /// `parent` (0 for the root), and everything beneath it.
    fn directory(&mut self, cluster: u32, parent: u32, children: &[Node]) -> Result<(), String> {
        let mut sorted: Vec<&Node> = children.iter().collect();
        sorted.sort_by(|a, b| a.name().cmp(b.name()));
        let mut entries: Vec<[u8; 32]> = Vec::new();
        if cluster != ROOT_CLUSTER {
            entries.push(short_entry(pad11(".", ""), ATTR_DIR, cluster, 0));
            entries.push(short_entry(pad11("..", ""), ATTR_DIR, parent, 0));
        }
        let mut aliases = 0;
        let mut subdirs = Vec::new();
        for node in sorted {
            let name = node.name();
            if !is_short(name) {
                aliases += 1;
            }
            let short = short_name(name, aliases);
            match node {
                Node::File { bytes, .. } => {
                    let size = u32::try_from(bytes.len())
                        .map_err(|_| format!("{name} is too large for FAT32"))?;
                    let first = match bytes.is_empty() {
                        true => 0,
                        false => self.allocate(bytes.len())?,
                    };
                    if first != 0 {
                        self.put(first, bytes);
                    }
                    entries.extend(entries_for(name, short, ATTR_ARCHIVE, first, size));
                }
                Node::Dir { children, .. } => {
                    let first = self.allocate(dir_bytes(children, false))?;
                    entries.extend(entries_for(name, short, ATTR_DIR, first, 0));
                    subdirs.push((first, children));
                }
            }
        }
        self.put(cluster, &entries.concat());
        // A ".." naming the root names cluster 0.
        let me = if cluster == ROOT_CLUSTER { 0 } else { cluster };
        for (first, children) in subdirs {
            self.directory(first, me, children)?;
        }
        Ok(())
    }
}

/// The volume's boot sector, for a volume that starts `hidden` sectors into its
/// disk.
fn boot_sector(geometry: Geometry, hidden: u32, volume_id: u32) -> [u8; 512] {
    let mut s = [0u8; 512];
    s[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    s[3..11].copy_from_slice(b"KERNELAI");
    s[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    s[13] = geometry.sectors_per_cluster as u8;
    s[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
    s[16] = FATS as u8;
    s[21] = 0xF8;
    s[24..26].copy_from_slice(&63u16.to_le_bytes());
    s[26..28].copy_from_slice(&255u16.to_le_bytes());
    s[28..32].copy_from_slice(&hidden.to_le_bytes());
    s[32..36].copy_from_slice(&geometry.sectors.to_le_bytes());
    s[36..40].copy_from_slice(&geometry.fat_sectors.to_le_bytes());
    s[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    s[48..50].copy_from_slice(&1u16.to_le_bytes());
    s[50..52].copy_from_slice(&6u16.to_le_bytes());
    s[64] = 0x80;
    s[66] = 0x29;
    s[67..71].copy_from_slice(&volume_id.to_le_bytes());
    s[71..82].copy_from_slice(b"KERNEL AI  ");
    s[82..90].copy_from_slice(b"FAT32   ");
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

fn fs_info(free: u32, next: u32) -> [u8; 512] {
    let mut s = [0u8; 512];
    s[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    s[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    s[488..492].copy_from_slice(&free.to_le_bytes());
    s[492..496].copy_from_slice(&next.to_le_bytes());
    s[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    s
}

/// A FAT32 volume of `sectors` sectors holding `root`'s children, for a partition
/// that starts `hidden` sectors into its disk.
pub fn volume(sectors: u32, hidden: u32, volume_id: u32, root: &[Node]) -> Result<Vec<u8>, String> {
    let geometry = Geometry::for_sectors(sectors)?;
    let mut writer = Writer {
        geometry,
        volume: vec![0; sectors as usize * SECTOR as usize],
        fat: vec![0; geometry.clusters() as usize + 2],
        next: ROOT_CLUSTER,
    };
    writer.fat[0] = 0x0FFF_FFF8;
    writer.fat[1] = END_OF_CHAIN;
    let root_cluster = writer.allocate(dir_bytes(root, true))?;
    writer.directory(root_cluster, 0, root)?;
    let free = geometry.clusters() + 2 - writer.next;
    let boot = boot_sector(geometry, hidden, volume_id);
    let info = fs_info(free, writer.next);
    let mut volume = writer.volume;
    for (sector, bytes) in [(0, &boot), (1, &info), (6, &boot), (7, &info)] {
        volume[sector * 512..(sector + 1) * 512].copy_from_slice(bytes);
    }
    let fat: Vec<u8> = writer.fat.iter().flat_map(|e| e.to_le_bytes()).collect();
    for copy in 0..FATS {
        let at = (RESERVED_SECTORS + copy * geometry.fat_sectors) as usize * SECTOR as usize;
        volume[at..at + fat.len()].copy_from_slice(&fat);
    }
    Ok(volume)
}

#[cfg(test)]
#[path = "fat32_tests.rs"]
mod tests;
