//! The package mirror, `pkgrepo=IP:PORT` (ADR 0011 §8; P4 task 2): the third
//! fixed destination beside `llm=` and `build=`. Ring 3 never chooses where a
//! request goes; PKG_FETCH sends `pkg`'s requests here and nowhere else.

use crate::log;
use core::net::Ipv4Addr;
use spin::Once;

static TARGET: Once<(Ipv4Addr, u16)> = Once::new();

/// Reads `pkgrepo=` from the kernel command line.
pub fn configure(cmdline: &str) {
    for arg in cmdline.split_whitespace() {
        let Some(value) = arg.strip_prefix("pkgrepo=") else {
            continue;
        };
        match crate::net::parse_endpoint(value) {
            Some(target) => {
                TARGET.call_once(|| target);
                log!("pkgrepo: packages come from the mirror at {value}");
            }
            None => log!("pkgrepo: ignoring pkgrepo={value}: expected IPv4:PORT"),
        }
    }
}

/// The mirror, if the command line names one.
pub fn target() -> Option<(Ipv4Addr, u16)> {
    TARGET.get().copied()
}
