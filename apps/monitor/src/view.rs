//! How the monitor's numbers read: whole units, one decimal for percentages, and
//! "-" for what the OS has not told it.

use crate::{Memory, Proc};
use alloc::format;
use alloc::string::String;

/// A CPU share in thousandths: `12.3%`.
pub fn cpu(permille: Option<u32>) -> String {
    match permille {
        Some(p) => format!("{}.{}%", p / 10, p % 10),
        None => String::from("-"),
    }
}

/// A size in KiB: `340 KiB`, or `1.5 MiB`, or `2.0 GiB`.
pub fn size(kib: Option<u64>) -> String {
    let Some(kib) = kib else {
        return String::from("-");
    };
    match kib {
        k if k < 1024 => format!("{k} KiB"),
        k if k < 1024 * 1024 => format!("{}.{} MiB", k / 1024, k % 1024 * 10 / 1024),
        k => format!(
            "{}.{} GiB",
            k / (1024 * 1024),
            k % (1024 * 1024) * 10 / (1024 * 1024)
        ),
    }
}

/// Uptime as `3d 4h`, `2h 5m`, `7m 12s` or `9s`.
pub fn uptime(ms: Option<u64>) -> String {
    let Some(ms) = ms else {
        return String::from("-");
    };
    let s = ms / 1000;
    let (d, h, m, s) = (s / 86_400, s / 3600 % 24, s / 60 % 60, s % 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m {s}s"),
        (0, _, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

/// Memory in use out of the total, in thousandths; none until `memory` came.
pub fn used_permille(memory: Option<Memory>) -> Option<u16> {
    let m = memory?;
    if m.total_mib == 0 {
        return None;
    }
    let used = m.total_mib - m.free_mib;
    Some((used * 1000 / m.total_mib).min(1000) as u16)
}

/// `used / total MiB` for the memory line.
pub fn memory_line(memory: Option<Memory>) -> String {
    match memory {
        Some(m) => format!("{} of {} MiB used", m.total_mib - m.free_mib, m.total_mib),
        None => String::from("-"),
    }
}

/// Tokens a second, in tenths: `12.5 tokens/s`.
pub fn tokens(x10: Option<u32>) -> String {
    match x10 {
        Some(t) => format!("{}.{} tokens/s", t / 10, t % 10),
        None => String::from("-"),
    }
}

/// A process in one phrase, for the agent's answers: `firefox (pid 7), 512.0 MiB, 3.2% CPU`.
pub fn describe(p: &Proc) -> String {
    let mut out = format!("{} (pid {})", p.name, p.pid);
    if p.mem_kib.is_some() {
        out.push_str(&format!(", {}", size(p.mem_kib)));
    }
    if p.cpu_permille.is_some() {
        out.push_str(&format!(", {} CPU", cpu(p.cpu_permille)));
    }
    out
}
