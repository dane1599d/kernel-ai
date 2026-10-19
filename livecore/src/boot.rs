//! The boot manifest's kernel arguments (M9d). xtask builds the versions of
//! `notes` committed in git, ships them as boot modules and names them:
//! `live_good=notes:<label>`, the last one a boot swapped to and committed, and
//! `live_pending=notes:<label>`, the newest commit not proven yet. The kernel
//! starts the plan's version and, once notes serves, swaps it to the manifest's.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The component the manifest is for.
pub const COMPONENT: &str = "notes";
/// The version on every boot drive, built from `components/notes`.
pub const BUILT_IN: &str = "2";
/// What labels of versions built from a commit start with; 12 hex digits of the
/// commit follow.
pub const LABEL_PREFIX: &str = "c-";

/// Whether `label` names a version built from a commit, `c-<sha12>`: only those
/// are swapped to without a yes.
pub fn is_commit_label(label: &str) -> bool {
    label.strip_prefix(LABEL_PREFIX).is_some_and(|hex| {
        hex.len() == 12
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub good: Option<String>,
    pub pending: Option<String>,
}

impl Manifest {
    /// Reads `live_good=` and `live_pending=` from a kernel command line; also
    /// why any such argument was left out.
    pub fn parse(cmdline: &str) -> (Self, Vec<String>) {
        let mut manifest = Self::default();
        let mut ignored = Vec::new();
        for arg in cmdline.split_whitespace() {
            let (slot, value) = if let Some(value) = arg.strip_prefix("live_good=") {
                (&mut manifest.good, value)
            } else if let Some(value) = arg.strip_prefix("live_pending=") {
                (&mut manifest.pending, value)
            } else {
                continue;
            };
            match value.split_once(':') {
                Some((COMPONENT, label)) if is_commit_label(label) => {
                    *slot = Some(label.to_string());
                }
                _ => ignored.push(format!("ignoring {arg}: expected {COMPONENT}:c-<sha12>")),
            }
        }
        if manifest.pending.is_some() && manifest.pending == manifest.good {
            manifest.pending = None;
        }
        (manifest, ignored)
    }

    pub fn is_empty(&self) -> bool {
        self.good.is_none() && self.pending.is_none()
    }

    /// The version notes starts on, and the one it falls back to when that one
    /// cannot start.
    pub fn start(&self) -> (&str, Option<&str>) {
        (BUILT_IN, None)
    }

    /// The version to swap to once notes serves on `active`: the pending one.
    /// None when it already runs it or `failed` says it failed.
    pub fn swap_to(&self, active: &str, failed: impl Fn(&str) -> bool) -> Option<&str> {
        let label = self.pending.as_deref()?;
        (label != active && !failed(label)).then_some(label)
    }

    /// The line the kernel logs at boot.
    pub fn line(&self) -> String {
        format!(
            "boot: manifest good {} pending {}",
            self.good.as_deref().unwrap_or("none"),
            self.pending.as_deref().unwrap_or("none")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "c-0123456789ab";
    const TIP: &str = "c-ba9876543210";

    #[test]
    fn the_arguments_parse_and_bad_ones_are_left_out() {
        let (manifest, ignored) = Manifest::parse(&format!(
            "test live_good=notes:{GOOD} live_pending=notes:{TIP}"
        ));
        assert_eq!(manifest.good.as_deref(), Some(GOOD));
        assert_eq!(manifest.pending.as_deref(), Some(TIP));
        assert!(ignored.is_empty());
        assert_eq!(
            manifest.line(),
            format!("boot: manifest good {GOOD} pending {TIP}")
        );
        // Only versions built from a commit: never a test version on the drive.
        for bad in [
            "live_good=shell:c-0123456789ab",
            "live_good=notes:../x",
            "live_pending=notes:e3",
            "live_pending=notes:3-trap",
            "live_pending=notes:c-0123456789AB",
            "live_pending=notes:c-0123456789a",
            "live_pending=notes",
        ] {
            let (manifest, ignored) = Manifest::parse(bad);
            assert!(manifest.is_empty(), "{bad}");
            assert_eq!(ignored.len(), 1, "{bad}");
        }
        assert_eq!(Manifest::parse("llm=10.0.2.2:1").0, Manifest::default());
    }

    #[test]
    fn a_pending_version_that_is_the_good_one_is_only_good() {
        let (manifest, _) =
            Manifest::parse(&format!("live_good=notes:{GOOD} live_pending=notes:{GOOD}"));
        assert_eq!(
            (manifest.good.as_deref(), manifest.pending),
            (Some(GOOD), None)
        );
    }

    #[test]
    fn notes_starts_on_2_and_swaps_to_the_pending_version_unless_it_failed() {
        let (manifest, _) =
            Manifest::parse(&format!("live_good=notes:{GOOD} live_pending=notes:{TIP}"));
        assert_eq!(manifest.start(), ("2", None));
        assert_eq!(manifest.swap_to("2", |_| false), Some(TIP));
        assert_eq!(manifest.swap_to(TIP, |_| false), None);
        assert_eq!(manifest.swap_to("2", |label| label == TIP), None);
        let (good, _) = Manifest::parse(&format!("live_good=notes:{GOOD}"));
        assert_eq!(good.swap_to("2", |_| false), None);
        assert_eq!(Manifest::default().swap_to("2", |_| false), None);
    }
}
