//! `keep <name> on|off`: the keep-running switch (P3 task 12b; Settings' apps
//! area, P4 task 7). An app kept running never naps. The shell asks the
//! supervisor, which takes this only from the shell, only for an app
//! (`program=comphost`) and never for a protected name. The supervisor keeps
//! the flag in the service's [`crate::Record`], in the kernel-held state
//! region, so it holds across restarts of the app and of the supervisor; it
//! passes `keep=1` at each start, and tells a running app at once through the
//! kernel's `ctl keep-running on|off` (APP_CTL).

use crate::{PROTECTED, Record, ServiceSpec};
use alloc::format;
use alloc::string::String;

const WORD: &str = "keep";

/// The program that hosts apps: only its lines nap, so only they may be kept.
pub const APP_HOST: &str = "comphost";

/// The argument a kept app gets at start.
pub const ARG: &str = "keep=1";

/// `keep <name> on|off`, or `None` for any other request.
pub fn parse(text: &str) -> Option<(&str, bool)> {
    let mut words = text.split_whitespace();
    if words.next()? != WORD {
        return None;
    }
    let name = words.next()?;
    let on = match words.next()? {
        "on" => true,
        "off" => false,
        _ => return None,
    };
    if words.next().is_some() || !crate::valid_name(name) {
        return None;
    }
    Some((name, on))
}

pub fn encode(name: &str, on: bool) -> String {
    format!("{WORD} {name} {}", if on { "on" } else { "off" })
}

/// Whether `caller` may switch `name`, and its index: `shell` is the pid
/// serving the shell, if it runs.
pub fn check(
    name: &str,
    specs: &[&ServiceSpec],
    records: &[Record],
    caller: u64,
    shell: Option<u64>,
) -> Result<usize, &'static str> {
    if shell.is_none() || shell != Some(caller) {
        return Err("err only the shell keeps apps running");
    }
    if PROTECTED.contains(&name) {
        return Err("err protected");
    }
    let index = specs
        .iter()
        .position(|s| s.name == name)
        .ok_or("err unknown service")?;
    if specs[index].program != APP_HOST {
        return Err("err not an app");
    }
    match records.get(index) {
        Some(record) if record.name == name => Ok(index),
        _ => Err("err unknown service"),
    }
}
