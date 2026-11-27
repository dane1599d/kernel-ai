//! The guest's instructions, as bytes with the text each must decode to.

use super::{COM1, EXPECTED_SERIAL, LOAD, PD, PDPT, PML4};

/// The mode a run of instructions executes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Real,
    Protected,
    Long,
}

/// One instruction as written: its bytes and the text it decodes to.
#[derive(Clone, Debug)]
pub struct Insn {
    pub bytes: Vec<u8>,
    pub text: String,
}

/// A run of instructions at one address.
#[derive(Clone, Debug)]
pub struct Code {
    pub mode: Mode,
    pub addr: u32,
    pub insns: Vec<Insn>,
}

/// The built guest: its image and the code in it, for the decode check.
pub struct Guest {
    pub image: Vec<u8>,
    pub code: Vec<Code>,
}

fn i(bytes: &[u8], text: impl Into<String>) -> Insn {
    Insn {
        bytes: bytes.to_vec(),
        text: text.into(),
    }
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// Where each part sits; every instruction's length is fixed, so a first pass
/// with zeros tells where the second puts them.
#[derive(Clone, Copy, Default)]
struct At {
    gdt: u32,
    gdtr: u32,
    code32: u32,
    code64: u32,
    msg: u32,
}

const CODE32_SEL: u16 = 0x08;
const DATA_SEL: u16 = 0x10;
const CODE64_SEL: u16 = 0x18;
/// Null, 32-bit code, 32-bit data, 64-bit code: flat, ring 0.
const GDT: [u64; 4] = [
    0,
    0x00CF_9A00_0000_FFFF,
    0x00CF_9200_0000_FFFF,
    0x00AF_9A00_0000_FFFF,
];
const EFER: u32 = 0xC000_0080;

fn code16(at: &At) -> Vec<Insn> {
    let gdtr = at.gdtr as u16;
    vec![
        i(&[0xFA], "cli"),
        i(&[0x31, 0xC0], "xor ax, ax"),
        i(&[0x8E, 0xD8], "mov ds, ax"),
        i(&[0x8E, 0xC0], "mov es, ax"),
        i(
            &cat(&[&[0x66, 0x0F, 0x01, 0x16], &gdtr.to_le_bytes()]),
            format!("lgdt [{gdtr:#x}]"),
        ),
        i(&[0x0F, 0x20, 0xC0], "mov eax, cr0"),
        i(&[0x0C, 0x01], "or al, 0x1"),
        i(&[0x0F, 0x22, 0xC0], "mov cr0, eax"),
        i(
            &cat(&[
                &[0x66, 0xEA],
                &at.code32.to_le_bytes(),
                &CODE32_SEL.to_le_bytes(),
            ]),
            format!("jmp {CODE32_SEL:#x}:{:#x}", at.code32),
        ),
    ]
}

fn mov_dword(at: u32, value: u32) -> Insn {
    i(
        &cat(&[&[0xC7, 0x05], &at.to_le_bytes(), &value.to_le_bytes()]),
        format!("mov dword [{at:#x}], {value:#x}"),
    )
}

fn mov_imm(reg: u8, name: &str, value: u32) -> Insn {
    i(
        &cat(&[&[0xB8 + reg], &value.to_le_bytes()]),
        format!("mov {name}, {value:#x}"),
    )
}

fn code32(at: &At) -> Vec<Insn> {
    vec![
        i(
            &cat(&[&[0x66, 0xB8], &DATA_SEL.to_le_bytes()]),
            format!("mov ax, {DATA_SEL:#x}"),
        ),
        i(&[0x8E, 0xD8], "mov ds, ax"),
        i(&[0x8E, 0xC0], "mov es, ax"),
        i(&[0xFC], "cld"),
        // Zero the three tables, then fill their first entries.
        mov_imm(7, "edi", PML4),
        i(&[0x31, 0xC0], "xor eax, eax"),
        mov_imm(1, "ecx", 0xC00),
        i(&[0xF3, 0xAB], "rep stosd"),
        mov_dword(PML4, PDPT | 0x3),
        mov_dword(PDPT, PD | 0x3),
        mov_dword(PD, 0x83),
        mov_imm(0, "eax", 0x20),
        i(&[0x0F, 0x22, 0xE0], "mov cr4, eax"),
        mov_imm(0, "eax", PML4),
        i(&[0x0F, 0x22, 0xD8], "mov cr3, eax"),
        mov_imm(1, "ecx", EFER),
        i(&[0x0F, 0x32], "rdmsr"),
        i(&cat(&[&[0x0D], &0x100u32.to_le_bytes()]), "or eax, 0x100"),
        i(&[0x0F, 0x30], "wrmsr"),
        i(&[0x0F, 0x20, 0xC0], "mov eax, cr0"),
        i(
            &cat(&[&[0x0D], &0x8000_0000u32.to_le_bytes()]),
            "or eax, 0x80000000",
        ),
        i(&[0x0F, 0x22, 0xC0], "mov cr0, eax"),
        i(
            &cat(&[&[0xEA], &at.code64.to_le_bytes(), &CODE64_SEL.to_le_bytes()]),
            format!("jmp {CODE64_SEL:#x}:{:#x}", at.code64),
        ),
    ]
}

fn code64(at: &At, start: u32) -> Vec<Insn> {
    // `mov esi` and `mov dx` come first; `lodsb` heads the loop, and `hlt`
    // follows its 8 bytes.
    let next = start + 5 + 4;
    let halt = next + 8;
    vec![
        mov_imm(6, "esi", at.msg),
        i(
            &cat(&[&[0x66, 0xBA], &COM1.to_le_bytes()]),
            format!("mov dx, {COM1:#x}"),
        ),
        i(&[0xAC], "lodsb"),
        i(&[0x84, 0xC0], "test al, al"),
        i(&[0x74, 0x03], format!("jz {halt:#x}")),
        i(&[0xEE], "out dx, al"),
        i(&[0xEB, 0xF8], format!("jmp {next:#x}")),
        i(&[0xF4], "hlt"),
        i(&[0xEB, 0xFD], format!("jmp {halt:#x}")),
    ]
}

fn size(code: &[Insn]) -> u32 {
    code.iter().map(|insn| insn.bytes.len() as u32).sum()
}

fn align8(n: u32) -> u32 {
    n.next_multiple_of(8)
}

fn layout() -> At {
    let mut at = At::default();
    // Sizes do not move with the addresses: the first pass places the parts.
    for _ in 0..2 {
        let gdt = align8(LOAD + size(&code16(&at)));
        let gdtr = gdt + 8 * GDT.len() as u32;
        let code32_at = gdtr + 6;
        let code64_at = code32_at + size(&code32(&at));
        let msg = code64_at + size(&code64(&at, code64_at));
        at = At {
            gdt,
            gdtr,
            code32: code32_at,
            code64: code64_at,
            msg,
        };
    }
    at
}

impl Guest {
    pub fn build() -> Self {
        let at = layout();
        let parts = [
            (Mode::Real, LOAD, code16(&at)),
            (Mode::Protected, at.code32, code32(&at)),
            (Mode::Long, at.code64, code64(&at, at.code64)),
        ];
        let mut image = vec![0u8; (at.msg - LOAD) as usize];
        let mut put = |addr: u32, bytes: &[u8]| {
            let from = (addr - LOAD) as usize;
            image[from..from + bytes.len()].copy_from_slice(bytes);
        };
        for (_, addr, insns) in &parts {
            let bytes: Vec<u8> = insns.iter().flat_map(|insn| insn.bytes.clone()).collect();
            put(*addr, &bytes);
        }
        let gdt: Vec<u8> = GDT.iter().flat_map(|d| d.to_le_bytes()).collect();
        put(at.gdt, &gdt);
        let limit = (gdt.len() - 1) as u16;
        put(
            at.gdtr,
            &cat(&[&limit.to_le_bytes(), &at.gdt.to_le_bytes()]),
        );
        image.extend_from_slice(EXPECTED_SERIAL.as_bytes());
        image.extend_from_slice(b"\n\0");
        assert!(image.len() < 0x1000, "the image must end before the tables");
        let code = parts
            .into_iter()
            .map(|(mode, addr, insns)| Code { mode, addr, insns })
            .collect();
        Self { image, code }
    }
}
