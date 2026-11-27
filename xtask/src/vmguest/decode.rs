//! A decoder for the few instructions the guest uses, so its bytes are checked
//! against the text written beside them. It knows nothing else: an opcode it
//! does not know fails the check.

use super::listing::Mode;

const R32: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
const R16: [&str; 8] = ["ax", "cx", "dx", "bx", "sp", "bp", "si", "di"];
const R8: [&str; 8] = ["al", "cl", "dl", "bl", "ah", "ch", "dh", "bh"];
const SEG: [&str; 6] = ["es", "cs", "ss", "ds", "fs", "gs"];

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    fn le(&mut self, n: usize) -> Option<u32> {
        let mut value = 0u32;
        for k in 0..n {
            value |= u32::from(self.u8()?) << (8 * k);
        }
        Some(value)
    }
}

fn gpr(n: u8, wide: bool) -> &'static str {
    match wide {
        true => R32[usize::from(n & 7)],
        false => R16[usize::from(n & 7)],
    }
}

/// The instruction at the start of `bytes`, loaded at `at`: its length and text.
pub fn decode(mode: Mode, at: u32, bytes: &[u8]) -> Option<(usize, String)> {
    let mut r = Reader { bytes, at: 0 };
    let mut wide = mode != Mode::Real;
    let (mut rep, mut prefixed) = (false, false);
    let mut op = r.u8()?;
    loop {
        match op {
            // The operand-size prefix flips the mode's default size, once per
            // instruction: a CPU takes a repeated 0x66 as one, this loop flips
            // per byte. No instruction of the guest carries two.
            0x66 => {
                wide = !wide;
                prefixed = true;
            }
            0xF3 => rep = true,
            _ => break,
        }
        op = r.u8()?;
    }
    let size = if wide { 4 } else { 2 };
    let text = match op {
        0xFA => String::from("cli"),
        0xF4 => String::from("hlt"),
        0xFC => String::from("cld"),
        0xAC => String::from("lodsb"),
        0xEE => String::from("out dx, al"),
        0xAB if rep => String::from("rep stosd"),
        0x31 | 0x84 => {
            let modrm = r.u8()?;
            if modrm >> 6 != 3 {
                return None;
            }
            let (reg, rm) = ((modrm >> 3) & 7, modrm & 7);
            match op {
                0x31 => format!("xor {}, {}", gpr(rm, wide), gpr(reg, wide)),
                _ => format!("test {}, {}", R8[usize::from(rm)], R8[usize::from(reg)]),
            }
        }
        0x8E => {
            let modrm = r.u8()?;
            let seg = SEG.get(usize::from((modrm >> 3) & 7))?;
            match modrm >> 6 == 3 && !prefixed {
                true => format!("mov {seg}, {}", R16[usize::from(modrm & 7)]),
                false => return None,
            }
        }
        0x0C => format!("or al, {:#x}", r.u8()?),
        0x0D => format!("or {}, {:#x}", gpr(0, wide), r.le(size)?),
        0xB8..=0xBF => format!("mov {}, {:#x}", gpr(op - 0xB8, wide), r.le(size)?),
        0xC7 => {
            // `mov dword [disp32], imm32`, 32-bit addressing.
            if r.u8()? != 0x05 || !wide || mode == Mode::Real {
                return None;
            }
            let (disp, imm) = (r.le(4)?, r.le(4)?);
            format!("mov dword [{disp:#x}], {imm:#x}")
        }
        0xEA => {
            let offset = r.le(size)?;
            format!("jmp {:#x}:{offset:#x}", r.le(2)?)
        }
        0x74 | 0xEB => {
            let rel = r.u8()? as i8;
            let target = (at as i64 + r.at as i64 + i64::from(rel)) as u32;
            format!("{} {target:#x}", if op == 0x74 { "jz" } else { "jmp" })
        }
        0x0F => match r.u8()? {
            0x32 => String::from("rdmsr"),
            0x30 => String::from("wrmsr"),
            two @ (0x20 | 0x22) => {
                let modrm = r.u8()?;
                if modrm >> 6 != 3 {
                    return None;
                }
                let (cr, rm) = (modrm >> 3 & 7, R32[usize::from(modrm & 7)]);
                match two {
                    0x20 => format!("mov {rm}, cr{cr}"),
                    _ => format!("mov cr{cr}, {rm}"),
                }
            }
            0x01 => {
                // `lgdt [disp16]`: /2, mod 00 and r/m 110 in real mode.
                if r.u8()? != 0x16 || mode != Mode::Real || !wide {
                    return None;
                }
                format!("lgdt [{:#x}]", r.le(2)?)
            }
            _ => return None,
        },
        _ => return None,
    };
    Some((r.at, text))
}
