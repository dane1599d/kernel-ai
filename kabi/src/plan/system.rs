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
//! - `updated`, the model channel, started at boot, so a model trial the last
//!   boot left is judged then ([`super::with_updated`]; P4 task 10);
//! - the shell, with its routes to `updated`, `desktop` and, with `kai-data`,
//!   `pkg`, where the editor's builds of a C component install;
//! - with `kai-data`, the model manager's window, on demand, after `updated`,
//!   its second route (P4 task 10).
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
/// [`super::SHELL_DESK`]'s routes, the one line [`super::SHELL_DESK_FILES`].
pub const SHELL_DESK_FILES: &str = super::SHELL_DESK_FILES;
/// The shell with `fs` and `pkg`, which run together: [`SHELL_DESK_FILES`] and a
/// route to `pkg`, the one line [`super::SHELL_DESK_BUILD`].
pub const SHELL_DESK_BUILD: &str = super::SHELL_DESK_BUILD;

/// What every app's line shares: the shared host, its window on `desktop`, and
/// any caps beyond an app's own.
macro_rules! app {
    ($name:literal, $uses:literal) => {
        app!($name, $uses, "")
    };
    ($name:literal, $uses:literal, $caps:literal) => {
        app!($name, $uses, $caps, "")
    };
    ($name:literal, $uses:literal, $caps:literal, $more:literal) => {
        concat!(
            $name,
            " program=comphost caps=serve+component+call",
            $caps,
            " uses=",
            $uses,
            $more,
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

/// Notes with `kai-data`: [`APPS`]' first line with its folder of
/// `/system/apps` (`dir=apps`), where comphost keeps its saved state, so a note
/// survives a reboot. `fs` opens only `notes-window`'s own folder through it
/// (`fsserve`'s `own_folder_only`). Without `kai-data` there is no `fs`, and a
/// `dir=` line could not start, so Notes then keeps what the kernel keeps. It
/// is the one image app with `kai:app/storage`.
pub const NOTES_SAVED: &str = app!("notes-window", "desktop", "", " dir=apps");

/// The editor ([`super::EDITOR`], started on demand), with `edit`, which the
/// kernel grants comphost under this name alone. It is planned only with
/// `kai-data`: it opens and saves the person's files through the shell's
/// `files`, and a boot with no build service has no source for it to edit.
pub const EDITOR_APP: &str = app!("editor", "desktop", "+edit");

/// The model manager's window ([`super::MODELS_WINDOW`], started on demand). Its
/// second route is `updated`, so it is planned after `updated` and the shell,
/// not with [`APPS`]; and only with `kai-data`, since without it `updated` has
/// no model store to list.
pub const MODELS_APP: &str = app!("models-window", "desktop+updated");

/// The system's lines after `llm`'s and before `updated`'s and the shell's, in
/// plan order; `files` when the disk has a `kai-data` partition, so `fs` and
/// `pkg` run.
pub fn lines(files: bool) -> Vec<&'static str> {
    let mut lines = Vec::new();
    if files {
        lines.extend([FS, PKG]);
    }
    lines.push(CC);
    lines.push(if files { DESKTOP_PKG } else { DESKTOP });
    lines.extend(APPS.map(|app| match files && app == APPS[0] {
        true => NOTES_SAVED,
        false => app,
    }));
    if files {
        lines.push(EDITOR_APP);
    }
    lines
}

/// The system's shell line.
pub fn shell(files: bool) -> &'static str {
    match files {
        true => SHELL_DESK_BUILD,
        false => super::SHELL_DESK,
    }
}

/// The whole system plan: `llm`'s line, [`lines`], then the shell with
/// `updated` just before it, started at boot with the model download's
/// `NetDest` ([`super::UPDATED_NET`]), then with `kai-data` [`MODELS_APP`].
pub fn plan(llm: &str, files: bool) -> String {
    let mut plan = String::from(llm);
    for line in lines(files) {
        plan.push('\n');
        plan.push_str(line);
    }
    plan.push('\n');
    plan.push_str(&super::with_updated(shell(files), super::UPDATED_NET));
    if files {
        plan.push('\n');
        plan.push_str(MODELS_APP);
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

    /// With `kai-data` the system's shell has the route to `pkg` that the editor's
    /// builds need, and without it, which plans no `pkg`, none.
    #[test]
    fn the_system_shell_has_a_route_to_pkg_with_kai_data_and_only_then() {
        assert_eq!(shell(true), SHELL_DESK_BUILD);
        assert_eq!(shell(false), super::super::SHELL_DESK);
        assert!(shell(true).contains("routes=desktop+pkg+fetch "));
        assert!(lines(true).iter().any(|line| line.starts_with("pkg ")));
        assert!(!shell(false).contains("pkg"));
        // `pkg` is planned before the shell, as its route needs.
        let plan = plan("llm caps=serve", true);
        let order: Vec<&str> = plan
            .lines()
            .map(|l| l.split(' ').next().unwrap_or(""))
            .collect();
        let at = |name| order.iter().position(|n| *n == name).unwrap();
        assert!(at("pkg") < at("shell") && at("desktop") < at("shell"));
    }

    /// With `kai-data`, Notes keeps its saved state on disk: its line is
    /// [`APPS`]' first plus ` dir=apps` and nothing else, and no other line of
    /// the plan has a `dir=apps`. Without `kai-data`, no line has a `dir=`
    /// that `fs` would have to serve.
    #[test]
    fn notes_alone_has_its_apps_folder_and_only_with_kai_data() {
        assert!(APPS[0].starts_with("notes-window "));
        assert_eq!(
            NOTES_SAVED.replacen(" dir=apps", "", 1),
            APPS[0],
            "{NOTES_SAVED}"
        );
        let with = plan("llm caps=serve", true);
        let with: Vec<&str> = with.lines().collect();
        let saved: Vec<&str> = with
            .iter()
            .copied()
            .filter(|line| line.split_whitespace().any(|w| w == "dir=apps"))
            .collect();
        assert_eq!(saved, [NOTES_SAVED]);
        assert!(!with.contains(&APPS[0]));
        let without = plan("llm caps=serve", false);
        assert!(without.lines().any(|line| line == APPS[0]));
        assert!(!without.contains("dir=apps"));
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

    /// [`MODELS_APP`] is [`super::super::MODELS_WINDOW`] started on demand,
    /// planned last and only with `kai-data`; and no other line of the plan but
    /// the shell's has a route to `updated`.
    #[test]
    fn the_models_window_is_last_with_kai_data_and_the_only_app_routed_to_updated() {
        let on_demand = alloc::format!("{} start=demand", super::super::MODELS_WINDOW);
        assert_eq!(MODELS_APP, on_demand);
        let llm = "llm caps=serve+net";
        assert_eq!(plan(llm, true).lines().last(), Some(MODELS_APP));
        assert!(!plan(llm, false).contains("models-window"));
        for line in plan(llm, true).lines() {
            let name = line.split(' ').next().unwrap_or_default();
            let routes = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix("uses="))
                .unwrap_or_default();
            if routes.split('+').any(|r| r == "updated") {
                assert!(["shell", "models-window"].contains(&name), "{line}");
            }
        }
    }
}
