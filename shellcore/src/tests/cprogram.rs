//! Kai's C programs at `c>` (P4 task 14) with the real `cc` service: the
//! request goes out as C, the program is checked and tried on stub imports,
//! a refused one goes back with why at most three times, the run's calls go
//! through the one door into one log entry, the user's session survives it,
//! and `:keep` copies its definitions in.

use super::cmode::start_cc;
use super::*;
use llmproto::script::Lang;

const CPUS: &str = "I64 n_cpus; cpus(&n_cpus);\nPrint(\"cpus: %lld\\n\", n_cpus);";

fn c_shell(host: &Rc<FakeHost>) -> Shell {
    start_cc(host);
    let mut shell = shell(host);
    assert!(shell.eval(":c").ok);
    shell
}

fn answers(host: &FakeHost, programs: &[&str]) {
    for program in programs {
        host.answers.borrow_mut().push_back(Ok(Answer {
            summary: String::from("Does what was asked."),
            script: program.to_string(),
        }));
    }
}

#[test]
fn a_program_runs_after_its_trial_and_the_users_session_survives() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 kept = 7;").ok);
    answers(&host, &[CPUS]);
    let ran = shell.eval("? how many cpus are there");
    assert!(ran.ok, "{ran:?}");
    assert_eq!(ran.text, "kai: Does what was asked.\ncpus: 4");
    let asked = host.script_requests.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].lang, Lang::C);
    assert_eq!(asked[0].request, "how many cpus are there");
    drop(asked);
    // The program's session was its own: the user's globals are as they were,
    // and the program's names are not in it.
    assert_eq!(shell.eval("kept").text, "= 7");
    assert!(!shell.eval("n_cpus").ok);
}

#[test]
fn a_refused_program_goes_back_with_exactly_why() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    answers(
        &host,
        &[
            "I64 cpus;\ncpus(&cpus);",
            "I64 n;\ncpus(n, n, n);",
            "I64 z = 0;\nz = 1 / z;",
            CPUS,
        ],
    );
    let ran = shell.eval("? how many cpus");
    assert!(ran.ok, "{ran:?}");
    let asked = host.script_requests.borrow();
    assert_eq!(asked.len(), 4);
    let why = |i: usize| asked[i].repair.clone().expect("a repair").1;
    assert!(why(1).contains("refused before compiling"), "{}", why(1));
    assert!(
        why(1).contains("line 1: `cpus` is a function"),
        "{}",
        why(1)
    );
    assert!(
        why(2).starts_with("wcc refused that program:\nline 2"),
        "{}",
        why(2)
    );
    assert!(
        why(3).contains("stopped in its trial run: div-zero"),
        "{}",
        why(3)
    );
    assert_eq!(asked[3].repair.clone().unwrap().0, "I64 z = 0;\nz = 1 / z;");
}

/// Lane T's review of #588: with `<string.h>` added, the repair's program is
/// the one the line numbers count.
#[test]
fn a_repair_sends_the_program_its_line_numbers_count() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    answers(&host, &["I64 cpus;\nI64 n = strlen(\"ab\");", CPUS]);
    assert!(shell.eval("? how many cpus").ok);
    let asked = host.script_requests.borrow();
    let (program, why) = asked[1].repair.clone().expect("a repair");
    assert!(why.contains("line 2: `cpus` is a function"), "{why}");
    assert_eq!(program.lines().nth(1), Some("I64 cpus;"), "{program}");
}

#[test]
fn three_repairs_are_the_most() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    answers(&host, &["nope;"; 5]);
    let ran = shell.eval("? something");
    assert!(!ran.ok);
    assert!(
        ran.text
            .starts_with("Kai's program did not work after 3 repairs:"),
        "{ran:?}"
    );
    assert_eq!(host.script_requests.borrow().len(), 4);
}

#[test]
fn the_trial_changes_nothing_and_the_run_is_one_logged_entry() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    let program = "set_policy(\"round-robin\");\nPrint(\"done\\n\");";
    answers(&host, &[program]);
    let before = host.switches.get();
    let ran = shell.eval("? use round robin");
    assert!(ran.ok, "{ran:?}");
    // The trial's call was a stand-in: only the run switched the policy.
    assert_eq!(host.switches.get(), before + 1);
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    let id = shell.log.last_id();
    let entry = shell.log.get(id).expect("the run's entry");
    assert_eq!(entry.consent, "c-program");
    assert_eq!(entry.request, "? use round robin");
    assert_eq!(entry.script, program);
    // undo at c> puts the run back and leaves the user's session alone.
    assert!(shell.eval("I64 mine = 3;").ok);
    assert!(shell.eval("undo").ok);
    assert!(
        !shell.eval("mine").ok,
        "undo went back past the user's entry"
    );
    let undone = shell.eval("undo");
    assert!(undone.ok, "{undone:?}");
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn keep_shows_and_copies_the_programs_definitions() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(!shell.eval(":keep").ok);
    let program = "I64 Sq(I64 x) { return x * x; }\nPrint(\"%lld\\n\", Sq(4));";
    answers(&host, &[program]);
    assert_eq!(
        shell.eval("? square 4").text,
        "kai: Does what was asked.\n16"
    );
    assert!(!shell.eval("Sq(3)").ok, "Sq is the program's until :keep");
    let kept = shell.eval(":keep");
    assert!(kept.ok, "{kept:?}");
    assert!(kept.text.starts_with("kept Sq:\nI64 Sq(I64 x)"), "{kept:?}");
    // Definitions only: the program's Print did not run again.
    assert!(!kept.text.contains("\n16"));
    assert_eq!(shell.eval("Sq(3)").text, "= 9");
    assert!(!shell.eval(":keep").ok, "kept once");
    assert!(shell.eval("undo").text.contains("its definitions are gone"));
    assert!(!shell.eval("Sq(3)").ok);
}

#[test]
fn no_program_needed_is_the_summary() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    host.answers.borrow_mut().push_back(Ok(Answer {
        summary: String::from("Hello! Ask me for a program."),
        script: String::new(),
    }));
    assert_eq!(shell.eval("? hello").text, "Hello! Ask me for a program.");
    host.answers
        .borrow_mut()
        .push_back(Err(AnswerError::NoScript(String::from("I cannot reboot."))));
    assert_eq!(shell.eval("? reboot").text, "I cannot reboot.");
    assert!(!shell.eval("?").ok);
}
