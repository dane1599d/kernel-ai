//! What each app's effects may no longer call (P4 task 7, ADR 0001): the user
//! withholds a row an app's package declares, and gives it back. The pairs are
//! kept as `kai-withheld 1` in `/system/state/withheld`, beside the rules, and
//! the file is the one copy: each check reads it through the host, so a trial
//! run (whose host throws writes away) changes nothing, and a reboot loses
//! nothing. `run_effects` refuses a withheld row as it refuses an undeclared
//! one, from the next effect on, with no restart. A file that is there but out
//! of shape fails closed: every effect row is refused, and nothing overwrites
//! it, until it is put right; no file at all means none are withheld.
//!
//! ```text
//! kai-withheld 1
//! notes fs_write
//! ```

use crate::Host;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The file's first line.
pub const HEADER: &str = "kai-withheld 1";
/// At most this many pairs: every app's every declarable row, with room.
const MAX_PAIRS: usize = 256;
/// A name's longest, as the shell's `Name` holds it.
const MAX_NAME: usize = kabi::MAX_NAME;

/// The withheld (app, row) pairs, in the order they were withheld.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Withheld {
    pairs: Vec<(String, String)>,
}

impl Withheld {
    /// Whether `app`'s effects may not call `row`.
    pub fn contains(&self, app: &str, row: &str) -> bool {
        self.pairs.iter().any(|(a, r)| a == app && r == row)
    }

    /// `kai-withheld 1` text.
    pub fn encode(&self) -> String {
        let mut out = String::from(HEADER);
        for (app, row) in &self.pairs {
            out.push_str(&format!("\n{app} {row}"));
        }
        out.push('\n');
        out
    }

    /// `kai-withheld 1` read back. Every line is untrusted: one out of shape
    /// refuses the whole file.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return Err(format!("not {HEADER}"));
        }
        let mut withheld = Self::default();
        for line in lines.filter(|line| !line.is_empty()) {
            let (app, row) = line
                .split_once(' ')
                .filter(|(app, row)| valid(app, '-') && valid(row, '_'))
                .ok_or_else(|| format!("a line out of shape: {line:?}"))?;
            withheld = withheld.with(app, row, true)?;
        }
        Ok(withheld)
    }

    /// A copy with `app`'s `row` withheld (`true`) or given back.
    fn with(&self, app: &str, row: &str, withheld: bool) -> Result<Self, String> {
        let mut pairs: Vec<(String, String)> = self
            .pairs
            .iter()
            .filter(|(a, r)| !(a == app && r == row))
            .cloned()
            .collect();
        if withheld {
            if pairs.len() >= MAX_PAIRS {
                return Err(format!("more than {MAX_PAIRS} rows withheld"));
            }
            pairs.push((app.to_string(), row.to_string()));
        }
        Ok(Self { pairs })
    }
}

/// A package or row name: lower-case letters, digits and `sep`, starting with a
/// letter, and short enough for an undo to hold.
fn valid(name: &str, sep: char) -> bool {
    name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == sep)
}

/// The pairs withheld now: none with no file; why not, for a file out of
/// shape, which must not be read as none.
fn current(host: &dyn Host) -> Result<Withheld, String> {
    match host.withheld_read() {
        None => Ok(Withheld::default()),
        Some(text) => Withheld::parse(&text)
            .map_err(|why| format!("the withheld grants on disk are out of shape ({why})")),
    }
}

/// Whether `app`'s effects may not call `row` now: every row, while the file
/// is out of shape (fail closed).
pub fn is_withheld(host: &dyn Host, app: &str, row: &str) -> bool {
    current(host).map_or(true, |withheld| withheld.contains(app, row))
}

/// Checks `/system/state/withheld` at start: a file out of shape is audited,
/// and every effect row is refused until it is put right.
pub fn check_saved(host: &dyn Host) {
    if let Err(why) = current(host) {
        host.audit(&format!(
            "{why}; every app effect that calls a row is refused until it is put right"
        ));
    }
}

/// Withholds `row` from `app`'s effects: only a row its package declares, since
/// built-in imports and its own storage are not revocable in v0.1. Whether it
/// changed anything.
pub fn revoke(host: &dyn Host, app: &str, row: &str) -> Result<bool, String> {
    crate::apps::load_app(host, app);
    let apps = host
        .apps()
        .ok_or_else(|| String::from("there are no apps here"))?;
    if !apps.borrow().rows.may_effect(app, row) {
        return Err(format!(
            "{app} does not declare effects that call {row}, so there is nothing to revoke"
        ));
    }
    set(host, app, row, true)
}

/// Sets `app`'s `row` withheld or given back; it takes effect once written,
/// so a write that fails changes nothing. Whether it changed anything.
pub fn set(host: &dyn Host, app: &str, row: &str, withheld: bool) -> Result<bool, String> {
    if !valid(app, '-') || !valid(row, '_') {
        return Err(format!("{app} {row} is not an app and a row"));
    }
    // A file out of shape is left for the user to see: never overwritten.
    let now = current(host)?;
    if now.contains(app, row) == withheld {
        return Ok(false);
    }
    host.withheld_write(&now.with(app, row, withheld)?.encode())?;
    Ok(true)
}

/// The rows `app` declares for its effects that it may still call, for
/// Settings' "what each app can do": none while the file is out of shape.
pub fn allowed(host: &dyn Host, app: &str) -> Vec<String> {
    let (Some(apps), Ok(withheld)) = (host.apps(), current(host)) else {
        return Vec::new();
    };
    let apps = apps.borrow();
    apps.rows
        .effects_of(app)
        .iter()
        .filter(|row| !withheld.contains(app, row))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_reads_back_what_it_wrote() {
        let withheld = Withheld::default()
            .with("notes", "fs_write", true)
            .and_then(|w| w.with("files-ui", "fs_trash", true))
            .unwrap();
        let text = withheld.encode();
        assert_eq!(text, "kai-withheld 1\nnotes fs_write\nfiles-ui fs_trash\n");
        assert_eq!(Withheld::parse(&text), Ok(withheld));
    }

    #[test]
    fn a_line_out_of_shape_refuses_the_file() {
        for bad in [
            "kai-withheld 2\nnotes fs_write\n",
            "kai-withheld 1\nnotes\n",
            "kai-withheld 1\nNotes fs_write\n",
            "kai-withheld 1\nnotes fs-write\n",
            "kai-withheld 1\nnotes fs_write extra\n",
            "kai-withheld 1\nnotes ../fs\n",
        ] {
            assert!(Withheld::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn giving_back_removes_only_that_pair() {
        let withheld = Withheld::default()
            .with("notes", "fs_write", true)
            .and_then(|w| w.with("notes", "fs_trash", true))
            .and_then(|w| w.with("notes", "fs_write", false))
            .unwrap();
        assert!(!withheld.contains("notes", "fs_write"));
        assert!(withheld.contains("notes", "fs_trash"));
    }
}
