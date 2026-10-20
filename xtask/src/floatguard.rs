//! The kernel stays soft-float. Ring 3's x87, SSE and AVX registers stay live
//! while the kernel runs (it saves them only at thread switches), so a single
//! x87, MMX, SSE or AVX instruction in the kernel would corrupt a process's state
//! without a trace. Every build disassembles the kernel ELF with the toolchain's
//! llvm-objdump and fails if it finds one. XSAVE, XRSTOR, XSETBV, XGETBV and the
//! fences pass: they move or order state but never compute with vector registers.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Most offending instructions quoted in the error.
const QUOTED: usize = 8;

/// Fails if the kernel ELF at `kernel` holds any x87, MMX, SSE or AVX instruction.
pub fn check(kernel: &Path) -> Result<(), String> {
    let objdump = objdump()?;
    let output = Command::new(&objdump)
        .args(["-d", "-M", "intel", "--no-show-raw-insn"])
        .arg(kernel)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", objdump.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} failed on {}: {}",
            objdump.display(),
            kernel.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    if !listing.contains("Disassembly of section") {
        return Err(format!(
            "{} printed no disassembly for {}",
            objdump.display(),
            kernel.display()
        ));
    }
    let found = float_instructions(&listing);
    if found.is_empty() {
        return Ok(());
    }
    let quoted: Vec<&str> = found.iter().take(QUOTED).map(String::as_str).collect();
    Err(format!(
        "the kernel must stay soft-float, but {} has {} x87/MMX/SSE/AVX instruction(s): {}",
        kernel.display(),
        found.len(),
        quoted.join("; ")
    ))
}

/// The toolchain's llvm-objdump, from the `llvm-tools` component that
/// rust-toolchain.toml lists.
fn objdump() -> Result<PathBuf, String> {
    let rustc = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("rustc")
            .args(args)
            .output()
            .map_err(|e| format!("cannot run rustc: {e}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let sysroot = rustc(&["--print", "sysroot"])?;
    let version = rustc(&["-vV"])?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc -vV printed no host triple")?;
    let name = if cfg!(windows) {
        "llvm-objdump.exe"
    } else {
        "llvm-objdump"
    };
    let path = Path::new(sysroot.trim())
        .join("lib")
        .join("rustlib")
        .join(host.trim())
        .join("bin")
        .join(name);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "{} not found: add the llvm-tools component (rustup component add llvm-tools)",
            path.display()
        ))
    }
}

/// The instructions in an `llvm-objdump -d -M intel --no-show-raw-insn` listing
/// that use x87, MMX, SSE or AVX state, each as `address: instruction`.
pub fn float_instructions(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(instruction)
        .filter(|(_, text)| uses_float_state(text))
        .map(|(address, text)| {
            let words: Vec<&str> = text.split_whitespace().collect();
            format!("{address}: {}", words.join(" "))
        })
        .collect()
}

/// `(address, instruction)` from an instruction line; None for headers, labels and
/// blank lines.
fn instruction(line: &str) -> Option<(&str, &str)> {
    let (address, text) = line.split_once(':')?;
    let address = address.trim();
    let is_address = !address.is_empty() && address.chars().all(|c| c.is_ascii_hexdigit());
    let text = text.trim();
    (is_address && !text.is_empty()).then_some((address, text))
}

/// Prefixes llvm-objdump prints before a mnemonic.
const PREFIXES: &[&str] = &[
    "lock", "rep", "repe", "repz", "repne", "repnz", "notrack", "data16", "addr32", "cs", "ds",
    "es", "fs", "gs", "ss",
];

fn uses_float_state(text: &str) -> bool {
    // Drop the comment and any `<symbol>`, whose names could look like registers.
    let code = text.split('#').next().unwrap_or("");
    let mut cleaned = String::with_capacity(code.len());
    let mut depth = 0usize;
    for c in code.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => cleaned.push(c),
            _ => {}
        }
    }
    let mut words = cleaned.split_whitespace();
    let Some(mnemonic) = words.find(|word| !PREFIXES.contains(word)) else {
        return false;
    };
    let mnemonic = mnemonic.to_ascii_lowercase();
    let x87_or_mmx = mnemonic.starts_with('f') || matches!(mnemonic.as_str(), "wait" | "emms");
    let control = mnemonic.contains("mxcsr") || mnemonic.starts_with("vzero");
    let operands: String = words.collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    x87_or_mmx || control || operands_use_float_state(&operands)
}

fn operands_use_float_state(operands: &str) -> bool {
    let sized = ["xmmword", "ymmword", "zmmword", "tbyte"];
    if sized.iter().any(|size| operands.contains(size)) {
        return true;
    }
    operands
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(is_float_register)
}

/// xmm, ymm, zmm and tmm registers, MMX's mm0-7, x87's st and st0-7, and AVX-512's
/// mask registers k0-7.
fn is_float_register(word: &str) -> bool {
    let numbered = |prefix: &str, max: u32| {
        word.strip_prefix(prefix)
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|n| n <= max && word.len() > prefix.len())
    };
    word == "st"
        || ["xmm", "ymm", "zmm"].iter().any(|p| numbered(p, 31))
        || numbered("tmm", 7)
        || (numbered("mm", 7) && word.len() == 3)
        || (numbered("st", 7) && word.len() == 3)
        || (numbered("k", 7) && word.len() == 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "
kernel:\tfile format elf64-x86-64

Disassembly of section .text:

ffffffff80000000 <kmain>:
ffffffff80000000:      \tpush\trbp
ffffffff80000001:      \tmov\trax, qword ptr gs:[0x8]
ffffffff80000009:      \tlfence
ffffffff8000000c:      \txsave64\t[rdi]
ffffffff80000010:      \txrstor64\t[rsi]
ffffffff80000014:      \txsetbv
ffffffff80000017:      \tcall\t0xffffffff80001000 <_ZN6kernel3fpu5stack1k17hxmm0e>
ffffffff8000001c:      \tlea\trdi, [rip + 0x10]    # 0xffffffff80000030 <st_k1_xmm3>
ffffffff80000023:      \tnop\tword ptr cs:[rax + rax]
ffffffff8000002d:      \tlock\tcmpxchg\tqword ptr [rdi], rcx
";

    #[test]
    fn the_soft_float_kernel_passes() {
        assert_eq!(float_instructions(LISTING), Vec::<String>::new());
    }

    #[test]
    fn every_kind_of_float_instruction_is_found() {
        let lines = [
            "movaps\txmmword ptr [rsp], xmm0",
            "movq\trax, xmm1",
            "vmovdqu\tymm15, ymmword ptr [r12]",
            "vpaddd\tzmm0, zmm1, zmm2",
            "kmovw\tk1, eax",
            "fld\ttbyte ptr [rax]",
            "fstp\tst(1)",
            "fnstcw\tword ptr [rsp]",
            "wait",
            "emms",
            "movq\tmm0, rax",
            "ldmxcsr\tdword ptr [rsp]",
            "vzeroupper",
            "tileloadd\ttmm0, [rax + rcx]",
            "lock\tfxsave64\t[rdi]",
        ];
        for line in lines {
            let listing = format!("ffffffff80000000:      \t{line}\n");
            assert_eq!(
                float_instructions(&listing),
                [format!("ffffffff80000000: {}", line.replace('\t', " "))],
                "{line}"
            );
        }
    }

    #[test]
    fn register_names_are_matched_exactly() {
        for word in [
            "xmm0", "ymm31", "zmm7", "mm7", "st", "st3", "k0", "k7", "tmm2",
        ] {
            assert!(is_float_register(word), "{word}");
        }
        for word in [
            "xmm", "xmm32", "mm8", "mmx", "st8", "start", "k8", "k", "rsp", "kmain",
        ] {
            assert!(!is_float_register(word), "{word}");
        }
    }
}
