//! Zip archives written byte by byte (APPNOTE 4.3), so a test can make any
//! entry a hostile archive would hold: any name, flags, method, mode or
//! sizes, two entries with one name, or two sharing their data.

#![allow(dead_code)]

use std::io::Write;

/// One entry as its headers will say.
#[derive(Clone)]
pub struct Raw {
    pub name: Vec<u8>,
    /// The bytes stored after its local header.
    pub data: Vec<u8>,
    pub method: u16,
    pub flags: u16,
    pub crc: u32,
    /// The uncompressed size its headers say.
    pub size: u32,
    /// A Unix mode, in the external attributes' high half.
    pub mode: Option<u32>,
    /// Its central record points at this entry's local header instead.
    pub same_data_as: Option<usize>,
}

impl Raw {
    /// A stored file.
    pub fn stored(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            method: 0,
            flags: 0,
            crc: crc(data),
            size: data.len() as u32,
            mode: Some(0o100_644),
            same_data_as: None,
        }
    }

    /// A deflated file.
    pub fn deflated(name: &str, data: &[u8]) -> Self {
        let mut coder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        coder.write_all(data).unwrap();
        Self {
            data: coder.finish().unwrap(),
            method: 8,
            ..Self::stored(name, data)
        }
    }

    /// A folder.
    pub fn folder(name: &str) -> Self {
        Self {
            mode: Some(0o040_755),
            ..Self::stored(name, b"")
        }
    }
}

pub fn crc(data: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(data);
    crc.sum()
}

fn u16le(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn u32le(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

/// The archive of `entries`, in order.
pub fn archive(entries: &[Raw]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for entry in entries {
        offsets.push(out.len() as u32);
        if entry.same_data_as.is_some() {
            continue;
        }
        u32le(&mut out, 0x0403_4b50);
        u16le(&mut out, 20);
        u16le(&mut out, entry.flags);
        u16le(&mut out, entry.method);
        u32le(&mut out, 0); // time and date
        u32le(&mut out, entry.crc);
        u32le(&mut out, entry.data.len() as u32);
        u32le(&mut out, entry.size);
        u16le(&mut out, entry.name.len() as u16);
        u16le(&mut out, 0);
        out.extend_from_slice(&entry.name);
        out.extend_from_slice(&entry.data);
    }
    let central = out.len() as u32;
    for (n, entry) in entries.iter().enumerate() {
        let (from, at) = match entry.same_data_as {
            Some(other) => (&entries[other], offsets[other]),
            None => (entry, offsets[n]),
        };
        u32le(&mut out, 0x0201_4b50);
        // Made by Unix (3), version 2.0.
        u16le(&mut out, if entry.mode.is_some() { 0x0314 } else { 0x0014 });
        u16le(&mut out, 20);
        u16le(&mut out, entry.flags);
        u16le(&mut out, entry.method);
        u32le(&mut out, 0);
        u32le(&mut out, entry.crc);
        u32le(&mut out, from.data.len() as u32);
        u32le(&mut out, entry.size);
        u16le(&mut out, entry.name.len() as u16);
        u16le(&mut out, 0); // extra
        u16le(&mut out, 0); // comment
        u16le(&mut out, 0); // disk
        u16le(&mut out, 0); // internal attributes
        u32le(&mut out, entry.mode.map_or(0, |m| m << 16));
        u32le(&mut out, at);
        out.extend_from_slice(&entry.name);
    }
    let size = out.len() as u32 - central;
    u32le(&mut out, 0x0605_4b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, entries.len() as u16);
    u16le(&mut out, entries.len() as u16);
    u32le(&mut out, size);
    u32le(&mut out, central);
    u16le(&mut out, 0);
    out
}
