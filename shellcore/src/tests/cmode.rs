//! The `c>` prompt with the real `cc` service on the host: modes, entries
//! that define and call, `kai` calls through the one door with undo, asks
//! answered at the prompt, `:dis` and `:defs`, and a restart that replays.

use super::*;
use crate::PanelResult;
use ccserve::rows::Rows;
use ccserve::{Service, XCC_HEADERS};

/// cc's clock and Ctrl-C on the host.
pub(super) struct CcClock;

impl ccserve::Host for CcClock {
    fn now_ns(&self) -> u64 {
        let start = *START.get_or_init(std::time::Instant::now);
        start.elapsed().as_nanos() as u64
    }

    fn interrupted(&self) -> bool {
        false
    }
}

static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn start_cc(host: &FakeHost) {
    let service =
        Service::new(ccserve::CC_WASM, &XCC_HEADERS, Rows::os(), &CcClock).expect("cc starts");
    *host.cc.borrow_mut() = Some(service);
}

fn c_shell(host: &Rc<FakeHost>) -> Shell {
    start_cc(host);
    let mut shell = shell(host);
    let entered = shell.eval(":c");
    assert!(entered.ok && shell.at_c(), "{entered:?}");
    shell
}

#[test]
fn modes_switch_and_kai_lines_stay_rhai() {
    let host = FakeHost::new();
    start_cc(&host);
    let mut shell = shell(&host);
    assert!(!shell.at_c());
    assert_eq!(shell.eval("1 + 2").text, "3");
    // One entry from kai>, without changing mode.
    assert_eq!(shell.eval(":c 6 * 7").text, "= 42");
    assert!(!shell.at_c());
    assert!(shell.eval(":c").ok);
    assert!(shell.at_c());
    assert_eq!(shell.eval("6 * 7").text, "= 42");
    assert_eq!(shell.eval(":rhai").text, "back at kai>");
    assert!(!shell.at_c());
    assert_eq!(shell.eval("1 + 2").text, "3");
    assert!(!shell.eval(":nope").text.contains("c>"));
}

#[test]
fn definitions_calls_and_a_redefinition_that_switches_callers() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Sq(I64 x) { return x * x; }").ok);
    assert!(shell.eval("I64 Twice(I64 x) { return Sq(x) * 2; }").ok);
    assert_eq!(shell.eval("Twice(3)").text, "= 18");
    assert!(shell.eval("I64 Sq(I64 x) { return x + 1; }").ok);
    assert_eq!(shell.eval("Twice(3)").text, "= 8");
    assert_eq!(shell.eval("Print(\"hi %d\\n\", 5);").text, "hi 5");
    assert_eq!(shell.eval("\"x=%lld\\n\", Twice(1);").text, "x=4");
    let dis = shell.eval(":dis Twice");
    assert!(dis.ok && dis.text.contains("call Sq (slot "), "{dis:?}");
    let defs = shell.eval(":defs");
    assert!(defs.text.contains("I64 Twice(I64 x);"), "{defs:?}");
    // undo goes back past the calls to the redefinition.
    let undone = shell.eval("undo");
    assert!(
        undone.text.contains("its definitions are gone"),
        "{undone:?}"
    );
    assert_eq!(shell.eval("Twice(3)").text, "= 18");
}

#[test]
fn an_entry_left_open_continues_on_the_next_line() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    let open = shell.eval("I64 F(I64 x) {");
    assert!(open.more, "{open:?}");
    assert!(shell.eval("  return x * 10;").more);
    assert!(shell.eval("}").ok);
    assert_eq!(shell.eval("F(4)").text, "= 40");
    // Ctrl-C drops an open entry.
    assert!(shell.eval("I64 G() {").more);
    assert_eq!(shell.eval(CANCEL_ENTRY).text, "");
    assert!(!shell.eval("G()").ok);
}

#[test]
fn errors_name_the_entrys_line_and_traps_say_why() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    let error = shell.eval("I64 a = 1;\nI64 b = nope + 1;");
    assert!(!error.ok && error.text.starts_with("line 2: "), "{error:?}");
    let trap = shell.eval("I64 z = 0;\nPrint(\"a\");\nz = 1 / z;");
    assert!(!trap.ok, "{trap:?}");
    assert_eq!(trap.text, "a\nstopped: div-zero");
    assert!(
        shell
            .eval(":nope")
            .text
            .starts_with("at c> the commands are")
    );
}

#[test]
fn a_call_goes_through_the_one_door_and_undo_puts_it_back() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    let made = shell.eval("I64 st = fs_create(\"a.txt\", \"one\");\nst");
    assert_eq!(made.text, "= 0", "{made:?}");
    assert_eq!(
        host.files.borrow().get("a.txt").map(String::as_str),
        Some("one")
    );
    let mut buf =
        shell.eval("char buf[16];\nI64 n = fs_read(\"a.txt\", buf, 16);\n(const char *)buf");
    assert_eq!(buf.text, "= \"one\"", "{buf:?}");
    // The read entry defined buf and n: undo drops them, and the file stays.
    let undone = shell.eval("undo");
    assert_eq!(
        undone.text, "undid entry 2: its definitions are gone",
        "{undone:?}"
    );
    assert!(host.files.borrow().contains_key("a.txt"));
    // The next undo reverts the create, and st with it.
    let undone = shell.eval("undo");
    assert!(
        undone.ok && undone.text.starts_with("undid entry 1: "),
        "{undone:?}"
    );
    assert!(host.files.borrow().is_empty(), "{:?}", host.files.borrow());
    buf = shell.eval("st");
    assert!(!buf.ok, "st's definition is undone too: {buf:?}");
}

#[test]
fn a_call_that_asks_waits_for_the_answer() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    let mut shell = c_shell(&host);
    let asked = shell.eval(
        "Print(\"before\\n\");\nI64 st = process_stop(\"notes\");\nPrint(\"after %lld\\n\", st);",
    );
    let id = asked.confirm.expect("a stop asks");
    // Declined at its prompt: the program gets KAI_EREFUSED and goes on.
    let declined = shell.eval(&format!("answer {id} n"));
    assert!(declined.ok, "{declined:?}");
    assert!(declined.text.starts_with("before\n"), "{declined:?}");
    assert!(declined.text.ends_with("after -1"), "{declined:?}");
    assert!(host.procs.borrow().contains(&String::from("notes")));
    // Allowed: the stop runs and the program gets 0.
    let asked = shell.eval("process_stop(\"notes\")");
    let id2 = asked.confirm.expect("a stop asks");
    assert_ne!(id, id2);
    let once = host.mint(id2, "process_stop(\"notes\")");
    let allowed = shell.eval(&format!("answer {id2} allow once={once}"));
    assert!(allowed.text.ends_with("= 0"), "{allowed:?}");
    assert!(host.procs.borrow().is_empty());
}

#[test]
fn another_line_drops_an_entry_that_waits() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    let mut shell = c_shell(&host);
    assert!(shell.eval("process_stop(\"notes\");").confirm.is_some());
    assert_eq!(shell.eval("1 + 1").text, "= 2");
    assert!(host.procs.borrow().contains(&String::from("notes")));
}

#[test]
fn a_restarted_cc_gets_the_definitions_back() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Sq(I64 x) { return x * x; }").ok);
    assert!(shell.eval("I64 g = 5;").ok);
    assert_eq!(shell.eval("Sq(g)").text, "= 25");
    // The supervisor restarts cc: a new process with no session.
    start_cc(&host);
    let after = shell.eval("Sq(g)");
    assert_eq!(
        after.text, "cc restarted; restored 2 definitions; globals reset\n= 0",
        "{after:?}"
    );
    assert_eq!(shell.eval("Sq(3)").text, "= 9");
    // A cc that does not answer is said so, and the next line tries again.
    *host.cc.borrow_mut() = None;
    assert!(shell.eval("Sq(2)").text.contains("cc is not running"));
    start_cc(&host);
    assert!(shell.eval("Sq(2)").text.ends_with("= 4"));
}

#[test]
fn reset_empties_the_session() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Sq(I64 x) { return x * x; }").ok);
    assert_eq!(shell.eval(":reset").text, "the C session is empty again");
    assert!(!shell.eval("Sq(2)").ok);
    assert_eq!(shell.eval(":defs").text, "nothing defined yet");
    assert_eq!(shell.eval("undo").text, "nothing to undo at c>");
}

/// A reply for the scripted cc: one `need` for `call`.
fn need(call: &str) -> Vec<u8> {
    let mut reply = promptcore::wire::Reply::new(8192);
    reply
        .push(&promptcore::wire::Record::Need {
            id: 1,
            call: call.as_bytes(),
        })
        .unwrap();
    reply.into_bytes()
}

fn done() -> Vec<u8> {
    b"done 0 0 0\n".to_vec()
}

/// cc runs untrusted code: a `need` naming the shell's own words, not a `kai`
/// import, is refused and runs nothing.
#[test]
fn a_need_that_is_no_kai_call_is_refused() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("fs_create(\"a.txt\", \"one\");").ok);
    for name in ["undo", "script", "say", "clarify", "rules"] {
        host.cc_script.borrow_mut().extend([
            need(&format!(r#"{{"name":"{name}","arguments":{{}}}}"#)),
            done(),
        ]);
        assert!(shell.eval("1;").ok);
        let sent = host.cc_sent.borrow().last().cloned().unwrap();
        assert_eq!(
            sent,
            format!("resume 1 err\nrefused: {name} is not a kai call")
        );
    }
    assert!(host.files.borrow().contains_key("a.txt"));
}

#[test]
fn a_bare_yes_does_not_answer_a_waiting_call() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    let mut shell = c_shell(&host);
    let id = shell
        .eval("process_stop(\"notes\");")
        .confirm
        .expect("asks");
    // Not the prompt's answer: the entry is dropped and `y` runs as C.
    assert!(!shell.eval("y").ok);
    assert!(
        !shell
            .eval(&format!("answer {id} yes"))
            .text
            .contains("Done")
    );
    assert!(host.procs.borrow().contains(&String::from("notes")));
}

#[test]
fn an_entry_stops_after_its_call_limit_and_at_ctrl_c() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 n;").ok);
    let many = shell.eval("for (I64 i = 0; i < 300; i++) cpus(&n);");
    assert_eq!(
        many.text, "stopped: more than 256 kai calls in one entry",
        "{many:?}"
    );
    host.interrupt_in.set(Some(3));
    let stopped = shell.eval("for (I64 i = 0; i < 100; i++) cpus(&n);");
    assert_eq!(stopped.text, "stopped: interrupted", "{stopped:?}");
    assert_eq!(shell.eval("2 + 2").text, "= 4");
}

#[test]
fn colon_c_inside_an_open_rhai_entry_stays_rhai() {
    let host = FakeHost::new();
    start_cc(&host);
    let mut shell = shell(&host);
    assert!(shell.eval("fn f() {").more);
    shell.eval(":c");
    assert!(!shell.at_c());
}

/// A line from the desktop's AI panel runs at kai> while the terminal is at
/// c>, never reaches cc, and leaves the terminal's open entry and session.
#[test]
fn a_panel_line_runs_at_kai_and_leaves_c_alone() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Sq(I64 x) { return x * x; }").ok);
    assert!(shell.eval("I64 F(I64 x) {").more);
    let sent = host.cc_sent.borrow().len();
    assert_eq!(shell.eval_panel("1 + 2"), PanelResult::Done("3".into()));
    for line in [":c 6 * 7", ":c", ":rhai"] {
        assert_eq!(
            shell.eval_panel(line),
            PanelResult::Refused("C goes in the terminal".into()),
            "{line}"
        );
    }
    assert_eq!(host.cc_sent.borrow().len(), sent);
    assert!(shell.at_c());
    assert!(shell.eval("  return Sq(x); }").ok);
    assert_eq!(shell.eval("F(3)").text, "= 9");
}

/// While a C entry waits on its question a panel line is refused, since the
/// entry's log entry takes in what is logged after it began; the terminal's
/// answer still finishes the entry.
#[test]
fn the_terminals_answer_finishes_a_c_entry_after_a_panel_line() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    let mut shell = c_shell(&host);
    let id = shell
        .eval("process_stop(\"notes\")")
        .confirm
        .expect("a stop asks");
    assert!(matches!(
        shell.eval_panel("1 + 2"),
        PanelResult::Refused(why) if why.contains("C entry waits")
    ));
    let once = host.mint(id, "process_stop(\"notes\")");
    let allowed = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(allowed.text.ends_with("= 0"), "{allowed:?}");
    assert!(host.procs.borrow().is_empty());
}
