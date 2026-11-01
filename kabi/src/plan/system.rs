//! The v0.1 system: what the default boot (no `test=`) plans, and what
//! `desk-default` boots, from this one table so the two cannot drift. In plan
//! order, as each `uses=` target must come before its user:
//!
//! - `llm` (the kernel's own line, with its host and model);
//! - with a `kai-data` partition, `fs`, then `pkg`, which seeds the image's
//!   signed repository and serves the catalogue;
//! - `cc`, started on demand;
//! - `desktop`, with a route to `pkg` when it runs (the Apps sheet);
//! - the apps, each `start=demand` (the founder's decision of 2026-10-28):
//!   started when first opened, from the panel, the Apps sheet or `ui_open`;
//!   with `kai-data`, the editor too, which opens and saves the person's files;
//! - the shell, with its route to `desktop`.
//!
//! With a build service (`build=`), the kernel adds the boot manifest's headless
//! `notes` and the edit service after these; a boot without one runs neither.

use super::{CC, FS};
use alloc::string::String;
use alloc::vec::Vec;

/// `desktop` with the ask card, and its route to `pkg` for the Apps sheet.
pub const DESKTOP_PKG: &str =
    "desktop caps=display+input+serve+ask+call uses=pkg pages=8192 stack_pages=512 burst_ms=10000";
/// `desktop` when there is no `pkg` (no `kai-data`): the sheet lists nothing.
pub const DESKTOP: &str =
    "desktop caps=display+input+serve+ask pages=8192 stack_pages=512 burst_ms=10000";
/// `pkg` with `/system/pkg` and the image's repository. With no `step=` it
/// seeds the repository as generation 1 if the store has none, then serves.
/// Its stack is `pkg-image`'s: fsproto's minicbor decoder recurses.
pub const PKG: &str =
    "pkg dir=pkg bootfiles=pkgrepo caps=call+serve uses=supervisor pages=2048 stack_pages=128";
/// The shell on a desktop with `fs`: [`super::SHELL_FILES`]'s caps and
/// [`super::SHELL_DESK`]'s routes.
pub const SHELL_DESK_FILES: &str = "shell caps=serve+call+policy+stop+script+files+state uses=llm+supervisor+cc+desktop pages=4096 stack_pages=512 burst_ms=5000";

/// What every app's line shares: the shared host, its window on `desktop`, and
/// any caps beyond an app's own.
macro_rules! app {
    ($name:literal, $uses:literal) => {
        app!($name, $uses, "")
    };
    ($name:literal, $uses:literal, $caps:literal) => {
        concat!(
            $name,
            " program=comphost caps=serve+component+call",
            $caps,
            " uses=",
            $uses,
            " pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand"
        )
    };
}

/// The image's apps (P4), each its component's name. The System monitor is
/// the one granted `kai:system`, and its second route is `llm`, for `health`
/// alone (`comphost/system.rs`).
pub const APPS: [&str; 5] = [
    app!("notes-window", "desktop"),
    app!("files-window", "desktop"),
    app!("terminal-window", "desktop"),
    app!("settings-window", "desktop"),
    app!("monitor-window", "desktop+llm"),
];

/// The editor ([`super::EDITOR`], started on demand), with `edit`, which the
/// kernel grants comphost under this name alone. It is planned only with
/// `kai-data`: it opens and saves the person's files through the shell's
/// `files`, and a boot with no build service has no source for it to edit.
pub const EDITOR_APP: &str = app!("editor", "desktop", "+edit");

/// The system's lines after `llm`'s, in plan order; `files` when the disk has
/// a `kai-data` partition, so `fs` and `pkg` run.
pub fn lines(files: bool) -> Vec<&'static str> {
    let mut lines = Vec::new();
    if files {
        lines.extend([FS, PKG]);
    }
    lines.push(CC);
    lines.push(if files { DESKTOP_PKG } else { DESKTOP });
    lines.extend(APPS);
    if files {
        lines.push(EDITOR_APP);
    }
    lines.push(if files {
        SHELL_DESK_FILES
    } else {
        super::SHELL_DESK
    });
    lines
}

/// The whole system plan: `llm`'s line, then [`lines`].
pub fn plan(llm: &str, files: bool) -> String {
    let mut plan = String::from(llm);
    for line in lines(files) {
        plan.push('\n');
        plan.push_str(line);
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// `line`'s words, its `caps=` word apart.
    fn split(line: &str) -> (Vec<&str>, Option<&str>) {
        let caps = line.split_whitespace().find(|w| w.starts_with("caps="));
        let rest = line
            .split_whitespace()
            .filter(|w| !w.starts_with("caps="))
            .collect();
        (rest, caps)
    }

    /// [`SHELL_DESK_FILES`] is [`super::super::SHELL_DESK`] with
    /// [`super::super::SHELL_FILES`]'s caps, word for word: a change to the
    /// shell's routes or limits in one fails here until the other follows.
    #[test]
    fn the_desktop_shell_with_files_is_shell_desk_with_shell_files_caps() {
        let (desk, _) = split(super::super::SHELL_DESK);
        let (files_desk, files_caps) = split(SHELL_DESK_FILES);
        let (_, files) = split(super::super::SHELL_FILES);
        assert_eq!(files_desk, desk);
        assert_eq!(files_caps, files);
    }

    /// [`EDITOR_APP`] is the line `app-editor` boots, [`super::super::EDITOR`],
    /// started on demand; and only a boot with `kai-data` plans it.
    #[test]
    fn the_editor_is_the_app_editor_line_on_demand_with_kai_data_only() {
        let on_demand = alloc::format!("{} start=demand", super::super::EDITOR);
        assert_eq!(EDITOR_APP, on_demand);
        assert!(lines(true).contains(&EDITOR_APP));
        assert!(!lines(false).contains(&EDITOR_APP));
    }
}
