//! Plan lines the kernel gives the supervisor, kept here so the supervisor's
//! host tests parse exactly what the kernel sends.

use alloc::format;
use alloc::string::String;

pub mod system;

/// The shell's routes in its lines' `uses=` order. The shell finds each by its
/// index ([`shell_route`]), since a route carries no name, so every shell line
/// lists them in this order, `desktop` on a desktop only.
pub const SHELL_ROUTES: [&str; 5] = ["llm", "supervisor", "cc", "updated", "desktop"];

/// Each of [`SHELL_ROUTES`]'s index, as `user::route` takes it.
pub mod shell_route {
    pub const LLM: usize = 0;
    pub const SUPERVISOR: usize = 1;
    /// The `c>` prompt's compiler.
    pub const CC: usize = 2;
    /// The model channel (P4 task 10).
    pub const UPDATED: usize = 3;
    /// The desktop, for the AI panel (P3 task 7), on a desktop only.
    pub const DESKTOP: usize = 4;
}

/// The Rhai shell: it serves the terminal, calls the llm service, the
/// supervisor, `cc` and `updated` ([`SHELL_ROUTES`]), may switch the scheduling
/// policy, and needs a large stack for Rhai's recursion.
pub const SHELL: &str = "shell caps=serve+call+policy+stop+script uses=llm+supervisor+cc+updated pages=4096 stack_pages=512 burst_ms=5000";
/// [`SHELL`] once `fs` runs: with `files`, its `Dir` for `/home/user`, for the
/// file rows (P2 task 11), and `state`, its `Dir` for `/system/state`, for the
/// action log (P2 task 12).
pub const SHELL_FILES: &str = "shell caps=serve+call+policy+stop+script+files+state uses=llm+supervisor+cc+updated pages=4096 stack_pages=512 burst_ms=5000";
/// [`SHELL`] on a desktop: with a route to `desktop`, fifth, after `updated`,
/// which it takes the AI panel's requests from and reports their states to (P3
/// task 7).
pub const SHELL_DESK: &str = "shell caps=serve+call+policy+stop+script uses=llm+supervisor+cc+updated+desktop pages=4096 stack_pages=512 burst_ms=5000";
/// [`SHELL_DESK`] with the file system too, for a case that has the desktop and
/// a state directory in one shell (`app-settings` saves a theme and shows it in
/// Settings): the same routes, so `desktop` stays fifth, and `files` and `state`
/// added to the caps (a test holds the two lines together).
pub const SHELL_DESK_FILES: &str = "shell caps=serve+call+policy+stop+script+files+state uses=llm+supervisor+cc+updated+desktop pages=4096 stack_pages=512 burst_ms=5000";

/// `updated`'s words when it starts only on the shell's first model request,
/// as `cc` does: it holds no memory before then (K's condition that it does no
/// models work until the shell asks).
pub const ON_DEMAND: &str = " start=demand";

/// `shell`, a shell line, with `updated`'s line just before it, `words` after
/// that: [`ON_DEMAND`], a step (` step=check`), or nothing, for one started at
/// boot. Every plan with a shell line takes it from here: the shell's routes
/// must name services listed before it, and `updated`'s own, `llm`, comes
/// earlier still. With no step, `updated` does nothing at start but settle a
/// model trial left pending, and then answers the shell's model requests.
pub fn with_updated(shell: &str, words: &str) -> String {
    format!("{UPDATED}{words}\n{shell}")
}
/// The `c>` prompt's compiler (ADR 0012 D2; P4 task 12): xcc's cc.wasm and the
/// sessions, on wasmi, holding no authority. Planned before the shell, which
/// calls it, and started on demand: the first `c>` entry's call starts it, so
/// it holds no memory before then. Its pages cover `ccserve::budget`: the
/// program, its stack, the compiler and one compile, one session and the
/// modules it keeps (P4 plan's 3,680 counted the heap alone).
pub const CC: &str = "cc start=demand caps=serve pages=5120 stack_pages=512 burst_ms=5000";
/// The file system service (ADR 0004): the only holder of the `kai-data`
/// partition. Planned before the shell, and only when the disk has `kai-data`.
/// Its pages hold a 16 MiB block cache and kaifs's working state. `kai-fs 1`'s
/// minicbor decoder and the request paths outgrew the default 16-page stack,
/// as filecheck's and `pkg`'s did.
pub const FS: &str = "fs caps=serve+partition pages=8192 stack_pages=64 burst_ms=5000";
/// The update service (ADR 0008 §10): the only holder of the slots and the boot
/// record. `kaituf`'s JSON and `kai-fs 1`'s CBOR decoders recurse, hence the
/// larger stack, which counts within its pages with a 512 KiB root buffer and
/// two 128 KiB slot buffers. It is also the model channel (P4 task 10): it
/// serves the shell's model requests, asks `llm` for its health to judge a
/// model on trial, writes the model store (`dir=models`), and reads the signed
/// `models` role from the image's repository (`bootfiles=pkgrepo`).
pub const UPDATED: &str = "updated caps=update+serve+call uses=llm \
    dir=trust+updates+restart+models bootfiles=pkgrepo pages=2048 stack_pages=128 burst_ms=5000";
/// The live `notes` component, run by the shared `comphost` program, without its
/// arguments.
const NOTES_BASE: &str =
    "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000";
/// The live `notes` component as an app of the current generation (P4 task 1):
/// the supervisor starts it when `pkg` names its package version.
pub fn notes_from_pkg() -> String {
    format!("{NOTES_BASE} start=pkg")
}

/// The live `notes` component starting on `version`, falling back to `fallback`
/// when that cannot start. Its arguments go to the host. Version 2 is built from
/// `components/notes`, which `:edit notes` edits; the boot manifest may name a
/// version committed in git instead (M9d).
pub fn notes(version: &str, fallback: Option<&str>) -> String {
    match fallback {
        Some(fallback) => format!("{NOTES_BASE} version={version} fallback={fallback}"),
        None => format!("{NOTES_BASE} version={version}"),
    }
}
/// The System monitor (P4 task 9), `comphost` serving `monitor-window`: a route
/// to `desktop` for its window, and after it one to `llm`, which comphost asks
/// for `health` and for nothing else. comphost takes the second route to be llm's,
/// so the order in `uses=` is part of the line (a test in `supervise` pins it);
/// the name is the grant of `kai:system`, which no other line may carry. Planned
/// after `desktop` and `llm`.
pub const MONITOR_WINDOW: &str = "monitor-window program=comphost caps=serve+component+call uses=desktop+llm pages=8192 stack_pages=512 burst_ms=20000 version=1";
/// Probation ended by calls alone, so a test runs the same under KVM and TCG.
pub const CALLS_ONLY: &str = "probation_calls=3 probation_ms=0";
/// For the M9a test: version 1, with probation ended by calls alone, so the test
/// runs the same under KVM and TCG.
pub const NOTES_TEST: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=1 probation_calls=3 probation_ms=0";
/// For the M9b test: version 2, with probation ended by calls alone.
pub const NOTES_EDIT_TEST: &str = "notes program=comphost caps=serve+component pages=8192 stack_pages=512 burst_ms=20000 version=2 probation_calls=3 probation_ms=0";
/// The edit service, planned only when the kernel has a build service. The
/// kernel appends `host=IP:PORT token=HEX`.
pub const EDIT_BASE: &str = "edit caps=serve+publish pages=2048 burst_ms=10000";
/// The editor app (P4 task 11), `comphost` serving the `editor` component with a
/// route to `desktop` for its window. The kernel gives `cap::EDIT` to the service
/// of this name and no other, so the name is the grant of `kai:edit`, which sends
/// the app's messages to the edit service (`EDIT_CALL`). Planned after `desktop`.
pub const EDITOR: &str = "editor program=comphost caps=serve+component+call+edit uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1";

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn uses(line: &str) -> Vec<&str> {
        line.split_whitespace()
            .find_map(|w| w.strip_prefix("uses="))
            .map_or_else(Vec::new, |u| u.split('+').collect())
    }

    #[test]
    fn each_shell_line_lists_its_routes_in_the_shells_order() {
        let routes = SHELL_ROUTES;
        let named = [
            (shell_route::LLM, "llm"),
            (shell_route::SUPERVISOR, "supervisor"),
            (shell_route::CC, "cc"),
            (shell_route::UPDATED, "updated"),
            (shell_route::DESKTOP, "desktop"),
        ];
        for (index, name) in named {
            assert_eq!(routes[index], name);
        }
        assert_eq!(uses(SHELL), routes[..shell_route::DESKTOP]);
        assert_eq!(uses(SHELL_FILES), routes[..shell_route::DESKTOP]);
        assert_eq!(uses(SHELL_DESK), routes);
        assert_eq!(uses(SHELL_DESK_FILES), routes);
        assert_eq!(uses(system::SHELL_DESK_FILES), routes);
    }

    #[test]
    fn updated_is_planned_just_before_the_shell() {
        let plan = with_updated(SHELL, " step=check");
        let lines: Vec<&str> = plan.lines().collect();
        assert_eq!(lines, [format!("{UPDATED} step=check").as_str(), SHELL]);
        assert_eq!(uses(UPDATED), ["llm"]);
        assert_eq!(with_updated(SHELL_DESK, "").lines().next(), Some(UPDATED));
        let demand = with_updated(SHELL_DESK, ON_DEMAND);
        assert!(demand.lines().next().unwrap().ends_with(" start=demand"));
    }

    #[test]
    fn the_desk_shell_with_files_is_the_desk_shell_with_two_caps_more() {
        assert_eq!(
            SHELL_DESK_FILES,
            SHELL_DESK
                .replacen("+script ", "+script+files+state ", 1)
                .as_str()
        );
    }
}
