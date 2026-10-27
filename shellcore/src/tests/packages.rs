//! Installs where P1 touches them (ADR 0011 §9-§10): typed package lines, the
//! `pkg_*` rows the AI calls, and `undo` of a generation switch.

use super::*;
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

const NOTES_DONE: &str = "Done: installed notes 2.0.0 (74 KB), with spell-en 1.2.3. undo";

#[test]
fn a_named_install_runs_and_undo_puts_the_old_set_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(shell.eval("install notes"), ok(NOTES_DONE));
    assert_eq!(
        shell.eval("pkg list"),
        ok("notes 2.0.0\nspell-en 1.2.3 (needed by another)")
    );
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: packages back to generation 1")
    );
    assert_eq!(
        shell.eval("pkg list"),
        ok("nothing installed beyond the system")
    );
    // The undo is an entry: undo again puts the install back.
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: packages back to generation 2")
    );
    assert!(
        shell
            .eval("log actlog")
            .text
            .contains("res=pkg-generation before=1 after=2")
    );
    // No model was asked about any of it.
    assert!(host.act_lines.borrow().is_empty());
}

#[test]
fn other_systems_spellings_work_and_sudo_is_dropped_once() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(
        shell.eval("sudo apt install -y notes"),
        ok(&format!(
            "no sudo needed: the question is the permission\n{NOTES_DONE}"
        ))
    );
    assert_eq!(
        shell.eval("sudo pacman -R notes"),
        ok("Done: removed notes 2.0.0, and spell-en 1.2.3. undo")
    );
    assert_eq!(
        shell.eval("apt install python3"),
        Reply::err(
            "No Kernel AI package named python3. Linux packages need the Linux layer (v0.2)."
        )
    );
    // A variable of that name is Rhai's.
    shell.eval("let apt = 2");
    assert!(!shell.eval("apt install notes").text.starts_with("Done"));
}

#[test]
fn an_install_that_needs_a_new_permission_asks_once() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let reply = shell.eval("install weather");
    let wire = reply.to_wire();
    assert!(wire.starts_with("menu 1 asked\n"), "{wire}");
    assert!(wire.ends_with("\npkg_install(\"weather\")"), "{wire}");
    assert_eq!(
        shell.eval("pkg list"),
        ok("nothing installed beyond the system")
    );
    shell.eval("install weather");
    let once = host.mint(2, "pkg_install(\"weather\")");
    assert_eq!(
        shell.eval(&format!("answer 2 allow once={once}")),
        ok("Done: installed weather 0.1.0 (38 KB). undo")
    );
    assert_eq!(shell.eval("pkg list"), ok("weather 0.1.0"));
}

#[test]
fn an_install_the_ai_picked_asks_and_one_the_line_names_runs() {
    let host = FakeHost::new();
    host.calls.borrow_mut().extend(vec![
        call(r#"{"name":"pkg_install","arguments":{"name":"notes"}}"#),
        call(r#"{"name":"pkg_install","arguments":{"name":"notes"}}"#),
    ]);
    let mut shell = shell(&host);
    // The line names no package: the AI picked notes, so it asks.
    let reply = shell.eval("i need somewhere to keep my ideas");
    assert!(reply.to_wire().starts_with("menu 1 added\n"), "{reply:?}");
    shell.eval("n");
    assert_eq!(
        shell.eval("pkg list"),
        ok("nothing installed beyond the system")
    );
    // The line names it: it runs.
    assert_eq!(shell.eval("could you install notes for me"), ok(NOTES_DONE));
}
