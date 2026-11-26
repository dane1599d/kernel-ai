//! Loading a stock bzImage (Linux's x86 boot protocol, 2.12 or later) into
//! guest RAM for its 32-bit entry, with the zero page, e820 map, command
//! line, initramfs, a flat GDT and an MP table (MP spec 1.4) for one CPU.
//! No firmware and no ACPI. Throwaway: `vmm` ports linux-loader and
//! Firecracker's MP table writer (ADR 0006 item 4).

/// Guest physical addresses of what the loader writes.
pub const GDT: u64 = 0x500;
pub const BOOT_PARAMS: u64 = 0x7000;
pub const CMDLINE: u64 = 0x2_0000;
/// The last KiB of base memory, where Linux looks for the MP floating pointer.
pub const MP_TABLE: u64 = 0x9_FC00;
pub const KERNEL: u64 = 0x10_0000;
/// The local APIC's and IOAPIC's register pages (see `vm::mod`).
pub const LAPIC: u64 = 0xFEE0_0000;
pub const IOAPIC: u64 = 0xFEC0_0000;

const E820_RAM: u32 = 1;
const E820_RESERVED: u32 = 2;

/// What the vCPU starts with.
pub struct Entry {
    pub rip: u64,
    pub rsi: u64,
}

fn put(ram: &mut [u8], at: u64, bytes: &[u8]) -> Result<(), &'static str> {
    let at = at as usize;
    ram.get_mut(at..at + bytes.len())
        .ok_or("s06-3: a load runs past guest RAM")?
        .copy_from_slice(bytes);
    Ok(())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Loads `bzimage` and `initrd` into `ram` (guest physical 0 up), with
/// `cmdline`; the entry point and its RSI.
pub fn load(ram: &mut [u8], bzimage: &[u8], initrd: &[u8], cmdline: &str) -> Result<Entry, &'static str> {
    let b = bzimage;
    if b.len() < 0x1000 || u16_at(b, 0x1FE) != 0xAA55 || &b[0x202..0x206] != b"HdrS" {
        return Err("s06-3: guest/bzImage is not a bzImage");
    }
    let version = u16_at(b, 0x206);
    if version < 0x020C {
        return Err("s06-3: boot protocol before 2.12");
    }
    let setup_sects = match b[0x1F1] {
        0 => 4,
        n => n as usize,
    };
    let kernel = b
        .get((setup_sects + 1) * 512..)
        .ok_or("s06-3: bzImage shorter than its setup")?;
    let code32 = u32_at(b, 0x214) as u64;
    let cmdline_max = u32_at(b, 0x238) as usize;
    let initrd_max = u32_at(b, 0x22C) as u64;
    let ram_end = ram.len() as u64;
    if cmdline.len() >= cmdline_max.min(4095) {
        return Err("s06-3: command line too long");
    }
    put(ram, KERNEL, kernel)?;
    put(ram, CMDLINE, cmdline.as_bytes())?;
    put(ram, CMDLINE + cmdline.len() as u64, &[0])?;
    // The initramfs at the top of RAM the kernel may use for it.
    let top = (initrd_max + 1).min(ram_end) & !0xFFF;
    let initrd_at = top
        .checked_sub(initrd.len() as u64)
        .ok_or("s06-3: initrd larger than guest RAM")?
        & !0xFFF;
    // The decompressor runs at its 16 MiB load address (or above) and
    // needs init_size bytes there.
    let init_size = u32_at(b, 0x260) as u64;
    if initrd_at < (16 << 20) + init_size + (2 << 20) {
        return Err("s06-3: initrd would overlap the kernel's decompression");
    }
    put(ram, initrd_at, initrd)?;

    // The zero page: the setup header as the bzImage has it, then ours.
    let mut zp = [0u8; 4096];
    let header_end = 0x202 + b[0x201] as usize;
    zp[0x1F1..header_end].copy_from_slice(&b[0x1F1..header_end]);
    zp[0x210] = 0xFF; // type_of_loader: undefined.
    zp[0x211] |= 0x80; // loadflags: CAN_USE_HEAP.
    zp[0x224..0x226].copy_from_slice(&(0xFE00u16).to_le_bytes()); // heap_end_ptr
    zp[0x218..0x21C].copy_from_slice(&(initrd_at as u32).to_le_bytes());
    zp[0x21C..0x220].copy_from_slice(&(initrd.len() as u32).to_le_bytes());
    zp[0x228..0x22C].copy_from_slice(&(CMDLINE as u32).to_le_bytes());
    let e820 = [
        (0, MP_TABLE, E820_RAM),
        (MP_TABLE, 0xA_0000 - MP_TABLE, E820_RESERVED),
        (0xF_0000, 0x1_0000, E820_RESERVED),
        (KERNEL, ram_end - KERNEL, E820_RAM),
        (IOAPIC, 0x1000, E820_RESERVED),
        (LAPIC, 0x1000, E820_RESERVED),
    ];
    zp[0x1E8] = e820.len() as u8;
    for (i, (addr, size, kind)) in e820.iter().enumerate() {
        let at = 0x2D0 + i * 20;
        zp[at..at + 8].copy_from_slice(&addr.to_le_bytes());
        zp[at + 8..at + 16].copy_from_slice(&size.to_le_bytes());
        zp[at + 16..at + 20].copy_from_slice(&kind.to_le_bytes());
    }
    put(ram, BOOT_PARAMS, &zp)?;

    // __BOOT_CS (0x10) and __BOOT_DS (0x18), flat 4 GiB.
    let gdt: [u64; 4] = [0, 0, 0x00CF_9B00_0000_FFFF, 0x00CF_9300_0000_FFFF];
    for (i, d) in gdt.iter().enumerate() {
        put(ram, GDT + i as u64 * 8, &d.to_le_bytes())?;
    }
    mp_table(ram)?;
    crate::log!(
        "s06-3: bzImage protocol {}.{:02}, {} KiB kernel at {KERNEL:#x}, entry {code32:#x}; initrd {} KiB at {initrd_at:#x}; {} MiB RAM",
        version >> 8,
        version & 0xFF,
        kernel.len() / 1024,
        initrd.len() / 1024,
        ram_end >> 20
    );
    Ok(Entry {
        rip: code32,
        rsi: BOOT_PARAMS,
    })
}

fn checksum(bytes: &[u8]) -> u8 {
    0u8.wrapping_sub(bytes.iter().fold(0u8, |s, b| s.wrapping_add(*b)))
}

/// The MP floating pointer and a configuration table: CPU 0 (the BSP), the
/// ISA bus and the IOAPIC, at [`MP_TABLE`].
fn mp_table(ram: &mut [u8]) -> Result<(), &'static str> {
    let table_at = MP_TABLE + 16;
    let cpuid = core::arch::x86_64::__cpuid(1);
    let mut entries = [0u8; 20 + 8 + 8];
    // Processor: APIC 0, version 0x14, enabled and the BSP.
    entries[0..4].copy_from_slice(&[0, 0, 0x14, 0b11]);
    entries[4..8].copy_from_slice(&cpuid.eax.to_le_bytes());
    entries[8..12].copy_from_slice(&cpuid.edx.to_le_bytes());
    // Bus 0, ISA.
    entries[20..28].copy_from_slice(&[1, 0, b'I', b'S', b'A', b' ', b' ', b' ']);
    // IOAPIC 1, version 0x11, usable.
    entries[28..32].copy_from_slice(&[2, 1, 0x11, 1]);
    entries[32..36].copy_from_slice(&(IOAPIC as u32).to_le_bytes());
    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"PCMP");
    let length = (header.len() + entries.len()) as u16;
    header[4..6].copy_from_slice(&length.to_le_bytes());
    header[6] = 4;
    header[8..16].copy_from_slice(b"KERNELAI");
    header[16..28].copy_from_slice(b"S06-3 SPIKE ");
    header[34..36].copy_from_slice(&3u16.to_le_bytes());
    header[36..40].copy_from_slice(&(LAPIC as u32).to_le_bytes());
    let sum = checksum(&header).wrapping_add(checksum(&entries));
    header[7] = sum;
    let mut pointer = [0u8; 16];
    pointer[0..4].copy_from_slice(b"_MP_");
    pointer[4..8].copy_from_slice(&(table_at as u32).to_le_bytes());
    pointer[8] = 1;
    pointer[9] = 4;
    pointer[10] = checksum(&pointer);
    put(ram, MP_TABLE, &pointer)?;
    put(ram, table_at, &header)?;
    put(ram, table_at + header.len() as u64, &entries)
}
