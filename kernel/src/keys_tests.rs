//! `test=ui-keys` (key input for `uikit` apps): the keyboard reaches an app's
//! widgets. Notes' window opens and takes the focus; the harness types Tab,
//! Tab and Enter, which `desktop` posts to the window's event ring, comphost
//! hands to `uikit`, and `uikit` routes to "New note"; two more Tabs reach the
//! editor and "hi" goes into the new note. The case reads the note back.

use crate::display::claim;
use crate::{console, ipc, log, process, threads};
use alloc::string::String;

const NAME: &str = "notes-window";
/// The plan lines this case adds.
pub const PLAN: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1";
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// The harness's seven keys, a key each 150 ms after 200 ms, with room.
const KEYS_MS: u64 = 6_000;

pub fn check() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && !process::named(NAME).is_empty() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        return Err("ui-keys: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    log!("ui-keys: ready for keys");
    threads::sleep_ms(KEYS_MS);
    let get = "kai-call 1\nfunc app:notes/actions@1.0.0#get\nargs {id: 1}\n";
    let reply = ipc::call(NAME, get.as_bytes(), CALL_MS)
        .map(|r| String::from_utf8_lossy(&r).into_owned())
        .map_err(|_| "ui-keys: notes-window did not reply")?;
    log!("ui-keys: note 1 is {reply}");
    if !reply.contains("some(\"hi\")") {
        return Err("ui-keys: the typed keys did not reach Notes' widgets");
    }
    log!("ui-keys: Tab, Enter and typed text reached Notes' widgets through desktop and comphost");
    Ok(())
}
