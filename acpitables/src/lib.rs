//! Minimal ACPI reader: walks RSDP, then XSDT or RSDT, and extracts what the kernel
//! needs from the MADT (local APIC, I/O APICs, ISA interrupt overrides, CPUs) and the
//! HPET table. No AML. Host-tested against synthetic tables. [`hpet_counter`]
//! reads the HPET's main counter so a device that splits the read never tears it.

#![no_std]

extern crate alloc;

pub mod hpet_counter;

use alloc::vec::Vec;

/// Reads physical memory. The kernel maps pages on demand; tests use a fake.
pub trait PhysMem {
    fn read(&self, phys: u64, len: usize) -> Option<&[u8]>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Unreadable,
    BadRsdp,
    BadChecksum(&'static str),
    NoMadt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoApic {
    pub id: u8,
    pub address: u64,
    pub gsi_base: u32,
}

/// An ISA IRQ routed to a different global system interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Override {
    pub irq: u8,
    pub gsi: u32,
    /// MPS INTI flags: bits 0-1 polarity, bits 2-3 trigger mode.
    pub flags: u16,
}

/// A PCIe enhanced configuration (ECAM) window from the MCFG table. `base` is the
/// address bus 0 would have even when `start_bus` is higher, so function
/// (bus, device, function), for `start_bus <= bus <= end_bus`, has its 4 KiB
/// configuration space at `base + (bus << 20 | device << 15 | function << 12)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ecam {
    pub base: u64,
    pub segment: u16,
    pub start_bus: u8,
    pub end_bus: u8,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Platform {
    pub local_apic: u64,
    /// APIC IDs of enabled processors.
    pub cpus: Vec<u8>,
    pub io_apics: Vec<IoApic>,
    pub overrides: Vec<Override>,
    /// HPET register block, if the firmware has an HPET table.
    pub hpet: Option<u64>,
    /// PCIe configuration windows, if the firmware has an MCFG table.
    pub ecam: Vec<Ecam>,
}

/// How an ISA IRQ reaches the I/O APIC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub gsi: u32,
    pub active_low: bool,
    pub level_triggered: bool,
}

impl Platform {
    /// ISA IRQs are edge-triggered and active-high unless the MADT overrides them.
    pub fn isa_route(&self, irq: u8) -> Route {
        match self.overrides.iter().find(|o| o.irq == irq) {
            Some(o) => Route {
                gsi: o.gsi,
                active_low: o.flags & 0b11 == 0b11,
                level_triggered: (o.flags >> 2) & 0b11 == 0b11,
            },
            None => Route {
                gsi: u32::from(irq),
                active_low: false,
                level_triggered: false,
            },
        }
    }

    /// The I/O APIC that owns a global system interrupt.
    pub fn io_apic_for(&self, gsi: u32) -> Option<IoApic> {
        self.io_apics
            .iter()
            .filter(|a| a.gsi_base <= gsi)
            .max_by_key(|a| a.gsi_base)
            .copied()
    }
}

const SDT_HEADER_LEN: usize = 36;

pub fn parse(mem: &impl PhysMem, rsdp: u64) -> Result<Platform, Error> {
    let head = mem.read(rsdp, 20).ok_or(Error::Unreadable)?;
    if &head[..8] != b"RSD PTR " {
        return Err(Error::BadRsdp);
    }
    if !checksum_ok(head) {
        return Err(Error::BadChecksum("RSDP"));
    }
    let revision = head[15];
    let rsdt = u64::from(u32_at(head, 16));
    let (root, entry_size) = if revision >= 2 {
        let full = mem.read(rsdp, 36).ok_or(Error::Unreadable)?;
        if !checksum_ok(full) {
            return Err(Error::BadChecksum("extended RSDP"));
        }
        (u64_at(full, 24), 8)
    } else {
        (rsdt, 4)
    };
    let root_table = table(mem, root)?;
    let entries = &root_table[SDT_HEADER_LEN..];
    let mut platform = Platform::default();
    let mut found_madt = false;
    for chunk in entries.chunks_exact(entry_size) {
        let address = if entry_size == 8 {
            u64_at(chunk, 0)
        } else {
            u64::from(u32_at(chunk, 0))
        };
        let Some(signature) = mem.read(address, 4) else {
            continue;
        };
        match signature {
            b"APIC" => {
                parse_madt(table(mem, address)?, &mut platform);
                found_madt = true;
            }
            b"HPET" => {
                let hpet = table(mem, address)?;
                if hpet.len() >= 52 {
                    platform.hpet = Some(u64_at(hpet, 44));
                }
            }
            b"MCFG" => parse_mcfg(table(mem, address)?, &mut platform),
            _ => {}
        }
    }
    if found_madt {
        Ok(platform)
    } else {
        Err(Error::NoMadt)
    }
}

/// Returns a whole SDT after checking its length and checksum.
fn table(mem: &impl PhysMem, address: u64) -> Result<&[u8], Error> {
    let header = mem.read(address, SDT_HEADER_LEN).ok_or(Error::Unreadable)?;
    let len = u32_at(header, 4) as usize;
    if len < SDT_HEADER_LEN {
        return Err(Error::BadChecksum("table length"));
    }
    let bytes = mem.read(address, len).ok_or(Error::Unreadable)?;
    if checksum_ok(bytes) {
        Ok(bytes)
    } else {
        Err(Error::BadChecksum("system description table"))
    }
}

fn parse_madt(madt: &[u8], platform: &mut Platform) {
    platform.local_apic = u64::from(u32_at(madt, 36));
    let mut offset = 44;
    while offset + 2 <= madt.len() {
        let (kind, len) = (madt[offset], madt[offset + 1] as usize);
        if len < 2 || offset + len > madt.len() {
            break;
        }
        let entry = &madt[offset..offset + len];
        match (kind, len) {
            (0, 8..) if u32_at(entry, 4) & 1 == 1 => platform.cpus.push(entry[3]),
            (1, 12..) => platform.io_apics.push(IoApic {
                id: entry[2],
                address: u64::from(u32_at(entry, 4)),
                gsi_base: u32_at(entry, 8),
            }),
            (2, 10..) => platform.overrides.push(Override {
                irq: entry[3],
                gsi: u32_at(entry, 4),
                flags: u16::from_le_bytes([entry[8], entry[9]]),
            }),
            (5, 12..) => platform.local_apic = u64_at(entry, 4),
            _ => {}
        }
        offset += len;
    }
}

/// MCFG: the 36-byte header, 8 reserved bytes, then 16-byte allocation entries
/// (base address u64, segment group u16, start bus u8, end bus u8, reserved u32).
fn parse_mcfg(mcfg: &[u8], platform: &mut Platform) {
    const FIRST_ENTRY: usize = SDT_HEADER_LEN + 8;
    const ENTRY_LEN: usize = 16;
    if mcfg.len() < FIRST_ENTRY {
        return;
    }
    let (entries, _) = mcfg[FIRST_ENTRY..].as_chunks::<ENTRY_LEN>();
    for entry in entries {
        platform.ecam.push(Ecam {
            base: u64_at(entry, 0),
            segment: u16::from_le_bytes([entry[8], entry[9]]),
            start_bus: entry[10],
            end_bus: entry[11],
        });
    }
}

fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, &b| sum.wrapping_add(b)) == 0
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from(u32_at(bytes, offset)) | (u64::from(u32_at(bytes, offset + 4)) << 32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Fake physical memory: one flat buffer starting at physical address 0.
    struct Flat(Vec<u8>);

    impl PhysMem for Flat {
        fn read(&self, phys: u64, len: usize) -> Option<&[u8]> {
            self.0.get(phys as usize..phys as usize + len)
        }
    }

    fn fix_checksum(bytes: &mut [u8], at: usize) {
        bytes[at] = 0;
        let sum = bytes.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        bytes[at] = 0u8.wrapping_sub(sum);
    }

    fn sdt(signature: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; SDT_HEADER_LEN];
        t[..4].copy_from_slice(signature);
        t.extend_from_slice(body);
        let len = t.len() as u32;
        t[4..8].copy_from_slice(&len.to_le_bytes());
        fix_checksum(&mut t, 9);
        t
    }

    fn madt_body() -> Vec<u8> {
        let mut b = vec![];
        b.extend_from_slice(&0xFEE0_0000u32.to_le_bytes()); // local APIC
        b.extend_from_slice(&1u32.to_le_bytes()); // flags
        b.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]); // CPU 0, enabled
        b.extend_from_slice(&[0, 8, 1, 1, 0, 0, 0, 0]); // CPU 1, disabled
        b.extend_from_slice(&[1, 12, 7, 0]); // I/O APIC id 7
        b.extend_from_slice(&0xFEC0_0000u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&[2, 10, 0, 0]); // IRQ0 -> GSI 2
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&[2, 10, 0, 9]); // IRQ9 -> GSI 9, level, active low
        b.extend_from_slice(&9u32.to_le_bytes());
        b.extend_from_slice(&0b1111u16.to_le_bytes());
        b
    }

    fn hpet_body() -> Vec<u8> {
        let mut b = vec![0u8; 4]; // event timer block id
        b.extend_from_slice(&[0, 64, 0, 0]); // generic address: memory space
        b.extend_from_slice(&0xFED0_0000u64.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 0]); // number, min tick, protection
        b
    }

    fn mcfg_body() -> Vec<u8> {
        let mut b = vec![0u8; 8]; // reserved
        b.extend_from_slice(&0xB000_0000u64.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // segment 0
        b.extend_from_slice(&[0, 255]); // buses 0..=255
        b.extend_from_slice(&[0, 0, 0, 0]);
        b
    }

    /// Lays out RSDP v2 at 0, XSDT at 0x100, MADT at 0x200, HPET at 0x400 and
    /// MCFG at 0x600.
    fn machine(include_hpet: bool) -> Flat {
        let mut mem = vec![0u8; 0x800];
        let madt = sdt(b"APIC", &madt_body());
        let hpet = sdt(b"HPET", &hpet_body());
        let mcfg = sdt(b"MCFG", &mcfg_body());
        let mut pointers = 0x200u64.to_le_bytes().to_vec();
        if include_hpet {
            pointers.extend_from_slice(&0x400u64.to_le_bytes());
        }
        pointers.extend_from_slice(&0x600u64.to_le_bytes());
        let xsdt = sdt(b"XSDT", &pointers);
        mem[0x100..0x100 + xsdt.len()].copy_from_slice(&xsdt);
        mem[0x200..0x200 + madt.len()].copy_from_slice(&madt);
        mem[0x400..0x400 + hpet.len()].copy_from_slice(&hpet);
        mem[0x600..0x600 + mcfg.len()].copy_from_slice(&mcfg);
        let mut rsdp = vec![0u8; 36];
        rsdp[..8].copy_from_slice(b"RSD PTR ");
        rsdp[15] = 2;
        rsdp[20..24].copy_from_slice(&36u32.to_le_bytes());
        rsdp[24..32].copy_from_slice(&0x100u64.to_le_bytes());
        fix_checksum(&mut rsdp[..20], 8);
        fix_checksum(&mut rsdp, 32);
        mem[..36].copy_from_slice(&rsdp);
        Flat(mem)
    }

    #[test]
    fn parses_madt_and_hpet() {
        let platform = parse(&machine(true), 0).unwrap();
        assert_eq!(platform.local_apic, 0xFEE0_0000);
        assert_eq!(platform.cpus, vec![0]);
        assert_eq!(platform.io_apics.len(), 1);
        assert_eq!(platform.io_apics[0].address, 0xFEC0_0000);
        assert_eq!(platform.overrides.len(), 2);
        assert_eq!(platform.hpet, Some(0xFED0_0000));
        assert_eq!(
            platform.ecam,
            vec![Ecam {
                base: 0xB000_0000,
                segment: 0,
                start_bus: 0,
                end_bus: 255
            }]
        );
    }

    #[test]
    fn routes_isa_irqs_through_overrides() {
        let platform = parse(&machine(false), 0).unwrap();
        assert_eq!(platform.hpet, None);
        let timer = platform.isa_route(0);
        assert_eq!(
            (timer.gsi, timer.active_low, timer.level_triggered),
            (2, false, false)
        );
        let keyboard = platform.isa_route(1);
        assert_eq!((keyboard.gsi, keyboard.active_low), (1, false));
        let acpi = platform.isa_route(9);
        assert!(acpi.active_low && acpi.level_triggered);
        assert_eq!(platform.io_apic_for(1).map(|a| a.id), Some(7));
    }

    #[test]
    fn rejects_corrupt_tables() {
        let mut mem = machine(true);
        mem.0[0x200 + 40] ^= 0xFF; // corrupt the MADT body
        assert_eq!(
            parse(&mem, 0),
            Err(Error::BadChecksum("system description table"))
        );
        let mut mem = machine(true);
        mem.0[0] = b'X';
        assert_eq!(parse(&mem, 0), Err(Error::BadRsdp));
        assert_eq!(parse(&Flat(vec![0; 8]), 0), Err(Error::Unreadable));
    }
}
