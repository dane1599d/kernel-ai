//! `test=desk-many` (H3-8's review follow-up): `desktop` serves an app that
//! runs past the [`kabi::MAX_METRIC_PROCESSES`] processes METRICS lists.
//! [`FILLERS`] idle spinners start before the Settings stand-in, so its pid
//! comes after all of them. `desktop` names its caller with PROC_NAME, so the
//! stand-in's `settings put` is taken (`ok`), not refused as `err not the
//! settings app`, and the shell answers it. The stand-in is `keep-running`'s
//! (`keep_tests`), with its shell.

use crate::display::claim;
use crate::{ipc, keep_tests, log, process, threads};
use alloc::string::String;

/// Idle processes before the stand-in: with the image's own, more than
/// METRICS lists.
pub const FILLERS: usize = kabi::MAX_METRIC_PROCESSES;
/// The shell `keep-running` boots with: its route to `desktop` takes the
/// Settings app's requests.
pub const SHELL: &str = keep_tests::SHELL;
const WAIT_MS: u64 = 60_000;

/// `desktop`, [`FILLERS`] spinners, then the stand-in, before the shell.
pub fn plan() -> String {
    let mut plan = String::from("desktop caps=display+input+serve pages=8192 stack_pages=512\n");
    for n in 0..FILLERS {
        plan.push_str(&alloc::format!(
            "fill-{n} program=spinner pages=64 idle=1\n"
        ));
    }
    plan.push_str("settings-window program=spinner pages=64 idle=1");
    plan
}

pub fn check() -> Result<(), &'static str> {
    let mut up = false;
    for _ in 0..WAIT_MS / 50 {
        // `desktop` binds on its first receive, after it shows the desktop.
        up = claim::showing()
            && ipc::server_of("desktop").is_some()
            && ["settings-window", "shell"]
                .iter()
                .all(|name| !process::named(name).is_empty())
            && (0..FILLERS).all(|n| !process::named(&alloc::format!("fill-{n}")).is_empty());
        if up {
            break;
        }
        threads::sleep_ms(50);
    }
    if !up {
        return Err("desk-many: desktop, the fillers, the stand-in or the shell never came up");
    }
    let (pid, _) = process::named("settings-window")[0];
    let past = (0..FILLERS).all(|n| {
        process::named(&alloc::format!("fill-{n}"))
            .first()
            .is_some_and(|&(filler, _)| filler < pid)
    });
    if !past {
        return Err("desk-many: the stand-in did not start after the fillers");
    }
    log!("desk-many: desktop bound");
    log!("desk-many: the Settings stand-in runs after {FILLERS} other processes");
    // `settings` fails if desktop refuses the put; with user=0 the shell
    // answers with its refusal of an unclicked change.
    let answer = keep_tests::settings(1, false, "keep-running on notes-window")?;
    log!("desk-many: user=0: {answer}");
    if !answer.starts_with("err ") {
        return Err("desk-many: the shell took a change without a key or a click");
    }
    log!("desk-many: desktop named an app past METRICS' processes and served it");
    Ok(())
}
