//! The isolation probe: a test program that, on its Nth start, tries operation
//! `N % OPS.len()`. Ops 0-2 (x87, SSE and AVX) ring 3 may do since L2, and must
//! complete; every other op it must not be allowed to do, and the kernel's `probe`
//! case checks each attempt was stopped by one of the listed exception vectors.

/// (operation, vectors that may stop it); an empty list means it must complete.
pub const OPS: [(&str, &[u64]); 18] = [
    ("x87 instruction", &[]),
    ("SSE instruction", &[]),
    // Completes when the kernel turned AVX on; a CPU without AVX raises #UD.
    ("AVX instruction", &[]),
    ("cli", &[13]),
    ("hlt", &[13]),
    ("in from a port", &[13]),
    ("rdmsr", &[13]),
    ("write cr3", &[13]),
    // Stopped only when UMIP is on; otherwise the probe exits normally.
    ("sgdt", &[13]),
    ("swapgs", &[13]),
    ("int 0x80", &[11, 13]),
    ("int3", &[13]),
    // #GP on Intel (SYSENTER_CS is 0), #UD on AMD in long mode.
    ("sysenter", &[6, 13]),
    ("write own code", &[14]),
    ("run own data", &[14]),
    ("read kernel memory", &[14]),
    ("syscall with a kernel stack pointer", &[13]),
    // An x87 error kills only the process: CR0.NE makes it #MF, not an IRQ.
    ("unmasked x87 exception", &[16]),
];
pub const AVX: usize = 2;
pub const SGDT: usize = 8;
/// #UD, which AVX raises on a CPU without it.
const INVALID_OPCODE: u64 = 6;

/// How one run of the probe ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It exited with this code.
    Exited(u64),
    /// The exception with this vector stopped it.
    Stopped(u64),
    /// Anything else: killed, out of memory, not loaded.
    Other,
}

/// Whether op `op` may end with `outcome` on a CPU whose UMIP is on or off, and
/// on which the kernel turned AVX on or not.
pub fn allowed(op: usize, outcome: Outcome, umip: bool, avx: bool) -> bool {
    let Some((_, vectors)) = OPS.get(op) else {
        return false;
    };
    match (op, outcome) {
        (AVX, Outcome::Exited(0)) => avx,
        (AVX, Outcome::Stopped(vector)) => !avx && vector == INVALID_OPCODE,
        (SGDT, Outcome::Exited(0)) => !umip,
        (SGDT, Outcome::Stopped(vector)) => umip && vectors.contains(&vector),
        (_, Outcome::Exited(0)) => vectors.is_empty(),
        (_, Outcome::Stopped(vector)) => vectors.contains(&vector),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x87_sse_and_avx_must_complete() {
        for op in 0..3 {
            assert!(allowed(op, Outcome::Exited(0), true, true));
            assert!(!allowed(op, Outcome::Exited(2), true, true));
            assert!(!allowed(op, Outcome::Stopped(7), true, true));
        }
        assert!(!allowed(0, Outcome::Stopped(INVALID_OPCODE), true, false));
        assert!(!allowed(AVX, Outcome::Exited(0), true, false));
        assert!(allowed(AVX, Outcome::Stopped(INVALID_OPCODE), true, false));
        assert!(!allowed(AVX, Outcome::Stopped(INVALID_OPCODE), true, true));
    }

    #[test]
    fn every_other_op_must_be_stopped_by_a_listed_vector() {
        for (op, (_, vectors)) in OPS.iter().enumerate().skip(3) {
            if op == SGDT {
                continue;
            }
            for &vector in *vectors {
                assert!(allowed(op, Outcome::Stopped(vector), true, true), "{op}");
            }
            assert!(!allowed(op, Outcome::Stopped(3), true, true), "{op}");
            assert!(!allowed(op, Outcome::Exited(0), true, true), "{op}");
            assert!(!allowed(op, Outcome::Exited(1), true, true), "{op}");
            assert!(!allowed(op, Outcome::Other, true, true), "{op}");
        }
        assert!(allowed(17, Outcome::Stopped(16), true, true));
        assert!(!allowed(OPS.len(), Outcome::Exited(0), true, true));
    }

    #[test]
    fn sgdt_depends_on_umip() {
        assert!(allowed(SGDT, Outcome::Stopped(13), true, true));
        assert!(!allowed(SGDT, Outcome::Exited(0), true, true));
        assert!(allowed(SGDT, Outcome::Exited(0), false, true));
        assert!(!allowed(SGDT, Outcome::Stopped(13), false, true));
    }
}
