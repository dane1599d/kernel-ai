//! The LLM script path: routing, the strict checks, repair, confirmation and runs.

use super::*;
use crate::script::{check, names_in};
use alloc::vec;

fn answer(script: &str, summary: &str) -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from(summary),
        script: String::from(script),
    })
}

/// The reply of a script the shell ran without asking.
pub(super) fn ran(reply: &Reply) -> &str {
    assert!(reply.ok && reply.confirm.is_none(), "{reply:?}");
    &reply.text
}

pub(super) fn with_answers(answers: Vec<Result<Answer, AnswerError>>) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.answers.borrow_mut().extend(answers);
    let shell = shell(&host);
    (host, shell)
}

#[test]
fn a_run_tier_change_runs_at_once_and_says_what_happened() {
    let (host, mut shell) = with_answers(vec![answer(
        "print(\"CPUs: \" + cpus());\nset_policy(\"rr\");",
        "Prints the CPUs.",
    )]);
    let reply = shell.eval("? how many cpus are there");
    assert_eq!(
        reply,
        ok("CPUs: 4\nDone: policy round-robin (was interactive) (undo: `undo`)")
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert_eq!(
        host.script_requests.borrow()[0].request,
        "how many cpus are there"
    );
    assert_eq!(shell.eval("answer 1 yes"), Reply::err("nothing to confirm"));
    assert!(
        shell
            .eval("log")
            .text
            .contains("#1 how many cpus are there: ran")
    );
    assert!(shell.eval("log 1").text.contains("print(\"CPUs: \""));
    let audit = host.audit.borrow();
    assert!(
        audit
            .iter()
            .any(|line| line == "script 1 tier run; running it")
    );
    assert!(
        audit.iter().any(|line| {
            line == "script 1 set_policy (run): policy round-robin (was interactive)"
        })
    );
}

#[test]
fn a_refused_script_is_repaired_once_with_the_reason() {
    let (host, mut shell) = with_answers(vec![
        answer("print(memory_free());", "Prints free memory."),
        answer("print(memory().free_mib);", "Prints free memory."),
    ]);
    let reply = shell.eval("? how much memory is free");
    assert_eq!(ran(&reply), "100\n");
    let requests = host.script_requests.borrow();
    assert_eq!(requests.len(), 2);
    let (script, error) = requests[1].repair.clone().unwrap();
    assert_eq!(script, "print(memory_free());");
    assert!(error.contains("memory_free"), "{error}");
}

#[test]
fn a_second_refusal_gives_up_and_nothing_is_proposed() {
    let (host, mut shell) = with_answers(vec![
        answer("loop { }", "Spins."),
        answer("loop { }", "Spins."),
        answer("print(1);", "never asked for"),
    ]);
    let reply = shell.eval("? spin forever please");
    assert!(!reply.ok && reply.confirm.is_none());
    assert!(
        reply
            .text
            .starts_with("gave up after one repair: syntax error"),
        "{reply:?}"
    );
    assert_eq!(host.script_requests.borrow().len(), 2);
    assert_eq!(shell.eval("answer 1 yes"), Reply::err("nothing to confirm"));
}

#[test]
fn a_cut_off_answer_is_not_repaired() {
    let (host, mut shell) = with_answers(vec![Err(AnswerError::CutOff)]);
    let reply = shell.eval("? write a long script");
    assert_eq!(
        reply,
        Reply::err("the answer was cut off at the token limit")
    );
    assert_eq!(host.script_requests.borrow().len(), 1);
}

/// Checks `script` with a short summary, on a fresh fake host.
fn checks(script: &str) -> Result<crate::script::Checked, crate::script::Refusal> {
    checks_on(&FakeHost::new(), script)
}

fn checks_on(
    host: &Rc<FakeHost>,
    script: &str,
) -> Result<crate::script::Checked, crate::script::Refusal> {
    let host: Rc<dyn Host> = host.clone();
    check(
        &Answer {
            summary: String::from("Does it."),
            script: String::from(script),
        },
        &host,
        &DEFAULT_CONFIG,
    )
}

#[test]
fn the_strict_profile_refuses_every_open_loop_closure_and_indirect_call() {
    for bad in [
        "loop { }",
        "while true { }",
        "do { } until false;",
        "let n = 0; for i in 0..3 { while true { } }",
        "let f = |x| x; f(1)",
        "fn f() { 1 } f()",
        "Fn(\"cpus\")",
        "\"cpus\".call()",
        "eval(\"cpus()\")",
        "print(undefined_variable);",
        "print(no_such_function());",
        "set_policy(\"fastest\");",
    ] {
        assert!(checks(bad).is_err(), "{bad} was accepted");
    }
}

#[test]
fn a_division_by_zero_on_stand_in_values_is_noted_not_refused() {
    // The stub has two CPUs; the real system may not.
    let checked = checks("print(10 / (cpus() - 2));\nprint(no_such_function());").unwrap();
    let note = checked.note.clone().unwrap();
    assert!(
        note.starts_with("trial stopped ") && note.contains("on stand-in values (Division by zero"),
        "{note}"
    );
    let text = crate::script::shown(1, &checked);
    assert_eq!(text.lines().nth(2), Some(note.as_str()));
    let (_host, mut shell) = with_answers(vec![answer("print(10 / (cpus() - 2));", "Divides.")]);
    assert_eq!(ran(&shell.eval("? divide ten by the spare cpus")), "5\n");
}

#[test]
fn an_index_past_anything_the_system_can_have_is_refused() {
    // The stub lists as many services, with names as long, as the real system can,
    // so an index it rejects is wrong everywhere.
    for bad in [
        "print(processes()[16].name);",
        "print(processes()[0].name[30]);",
        "print(get_policy()[40]);",
        "let a = [][0];",
    ] {
        assert!(checks(bad).is_err(), "{bad} was accepted");
    }
    for good in [
        "print(processes()[15].name);",
        "print(processes()[0].name[20]);",
    ] {
        let checked = checks(good).unwrap();
        assert_eq!(checked.note, None, "{good}");
    }
}

#[test]
fn the_trial_run_is_bounded_like_any_run() {
    // One replace that would build a string of gigabytes stops before allocating.
    let bomb = "let s = \"x\";\ns.pad(60000, \"x\");\ns.replace(\"x\", s);";
    let why = checks(bomb).unwrap_err().to_string();
    assert!(why.contains("Length of string after replace"), "{why}");
    let why =
        checks("let s = \"xx\";\ns.replace('x', s);\ns.pad(40000, \"x\");\ns.replace('x', s);");
    assert!(why.unwrap_err().to_string().contains("after replace"));
    // The trial sees the real heap: a short script that allocates past the budget is
    // stopped, although it is far below the operation count of the other checks.
    let host = FakeHost::new();
    host.heap_step.set(DEFAULT_CONFIG.heap_budget + 1);
    let why = checks_on(&host, "print(1);").unwrap_err().to_string();
    assert!(why.contains("memory budget"), "{why}");
}

#[test]
fn replace_still_works_within_the_limit() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.eval("let s = \"hello\"");
    shell.eval("s.replace(\"l\", \"L\")");
    assert_eq!(shell.eval("s"), ok("heLLo"));
    shell.eval("s.replace('o', \"00\")");
    assert_eq!(shell.eval("s"), ok("heLL00"));
    shell.eval("s.replace(\"\", \"-\")");
    assert_eq!(shell.eval("s"), ok("-h-e-L-L-0-0-"));
}

#[test]
fn answer_and_log_can_be_variable_names() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(shell.eval("log"), ok("no LLM scripts yet"));
    assert_eq!(shell.eval("answer 1 yes"), Reply::err("nothing to confirm"));
    shell.eval("let answer = 41");
    assert_eq!(shell.eval("answer + 1"), ok("42"));
    assert_eq!(shell.eval("answer == 41"), ok("true"));
    shell.eval("let log = 3");
    assert_eq!(shell.eval("log * 2"), ok("6"));
    assert_eq!(shell.eval("log"), ok("3"));
}

#[test]
fn the_capability_list_over_approximates() {
    assert_eq!(
        names_in("print(`${set_policy(\"rr\")}`);").1,
        ["set_policy"]
    );
    assert_eq!(names_in("\"rr\".set_policy();").1, ["set_policy"]);
    assert_eq!(
        names_in("// memory() is not called\nprint(1);").0,
        ["memory"]
    );
    assert_eq!(names_in("let set_policy_x = 1;").1, Vec::<&str>::new());
}

#[test]
fn a_full_size_script_fits_one_screen_with_every_line_on_one_row() {
    use llmproto::script::{
        SCRIPT_MAX_BYTES, SCRIPT_MAX_LINE_CHARS, SCRIPT_MAX_LINES, SUMMARY_MAX_CHARS,
    };
    // The most lines, each as long as the byte limit then allows.
    let per_line = SCRIPT_MAX_BYTES / SCRIPT_MAX_LINES - 1;
    let line = format!("print(\"{}\");", "x".repeat(per_line - 10));
    let script = std::iter::repeat_n(line.as_str(), SCRIPT_MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let host: Rc<dyn Host> = FakeHost::new();
    let checked = check(
        &Answer {
            summary: "s".repeat(SUMMARY_MAX_CHARS),
            script,
        },
        &host,
        &DEFAULT_CONFIG,
    )
    .unwrap();
    // What `show` prints while the script asks: the script, then the question.
    let question = crate::script::question(99, &checked, &["set_policy"]);
    let text = format!("{}\n{question}", crate::script::shown(99, &checked));
    assert!(text.len() + 20 < kabi::MAX_MESSAGE, "{} bytes", text.len());
    // A 1024-pixel console has 128 columns; the terminal adds a two-character lead.
    let widest = text.lines().map(|l| l.chars().count()).max().unwrap();
    assert!(widest <= 126, "a line is {widest} characters");
    const { assert!(SCRIPT_MAX_LINE_CHARS + 5 <= 126) };
    // A 768-pixel console has 48 rows; the prompt and a trial note need two more.
    assert!(text.lines().count() + 2 < 48);
    assert!(text.ends_with("\nIt changes set_policy (no undo). Go ahead? y/n, or show"));
}

#[test]
fn the_question_is_two_short_lines_naming_what_has_no_undo() {
    let checked = checks("print(get_policy());\nset_policy(\"rr\");").unwrap();
    assert_eq!(
        crate::script::question(3, &checked, &["set_policy"]),
        "script 3: Does it.\nIt changes set_policy (no undo). Go ahead? y/n, or show"
    );
}

#[test]
fn plain_words_go_to_the_llm_and_near_miss_code_does_not() {
    assert!(reads_like_words("how much memory is free"));
    assert!(reads_like_words(
        "switch the scheduler to round robin please"
    ));
    assert!(reads_like_words("hello"));
    assert!(reads_like_words("what can u do"));
    assert!(!reads_like_words("let x be 5"));
    assert!(!reads_like_words("print hello world"));
    assert!(!reads_like_words("x + y + z"));
    assert!(!reads_like_words("foo(1)"));
    let (host, mut shell) = with_answers(vec![answer("set_policy(\"rr\");", "Switches.")]);
    assert_eq!(shell.eval("switch me to round robin please").confirm, None);
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert!(shell.eval("let x be 5").text.starts_with("syntax error"));
    assert_eq!(host.script_requests.borrow().len(), 1);
}

#[test]
fn a_word_rhai_takes_for_a_variable_goes_to_the_llm() {
    let (host, mut shell) = with_answers(vec![
        Err(AnswerError::NoScript(String::from(
            "Hello! Ask me anything.",
        ))),
        Err(AnswerError::NoScript(String::from("Testing works."))),
    ]);
    assert_eq!(shell.eval("hello"), ok("Hello! Ask me anything."));
    assert_eq!(shell.eval("testing"), ok("Testing works."));
    let requests = host.script_requests.borrow();
    assert_eq!(requests[0].request, "hello");
    assert_eq!(requests[1].request, "testing");
    assert!(
        shell
            .eval("log")
            .text
            .contains("#1 hello: answered in words")
    );
}

#[test]
fn real_code_that_fails_keeps_its_error() {
    let (host, mut shell) = with_answers(vec![]);
    // A missing variable in code, a missing function, and a runtime error after
    // output: none of them is plain words.
    assert!(
        shell
            .eval("hello + 1")
            .text
            .contains("Variable not found: hello")
    );
    assert!(
        shell
            .eval("nothing_here()")
            .text
            .contains("Function not found")
    );
    assert!(!shell.eval("print(\"a\"); hello").ok);
    assert!(shell.eval("let x = 1 +").text.starts_with("syntax error"));
    for name in ["x", "n2", "max_len"] {
        assert!(
            shell.eval(name).text.contains("Variable not found"),
            "{name}"
        );
    }
    assert!(host.script_requests.borrow().is_empty());
    // Commands and `ask` work as before.
    assert!(shell.eval("ask hi").ok);
    assert_eq!(host.asked.borrow().as_slice(), ["hi"]);
}

#[test]
fn a_chat_question_gets_an_answer_not_a_script() {
    let (host, mut shell) = with_answers(vec![Err(AnswerError::NoScript(String::from(
        "I answer questions and run scripts that read the system.",
    )))]);
    let reply = shell.eval("what can u do");
    assert_eq!(
        reply,
        ok("I answer questions and run scripts that read the system.")
    );
    assert_eq!(reply.confirm, None);
    assert_eq!(host.script_requests.borrow()[0].request, "what can u do");
}

#[test]
fn a_read_only_script_runs_at_once_and_shows_only_its_output() {
    let (host, mut shell) = with_answers(vec![answer(
        "print(\"free: \" + memory().free_mib + \" MiB\");",
        "Prints free memory.",
    )]);
    let reply = shell.eval("show me the free memory");
    assert_eq!(reply.confirm, None);
    assert!(reply.ok);
    assert_eq!(reply.text, "free: 100 MiB\n");
    assert_eq!(shell.last_output(), Some("free: 100 MiB\n"));
    assert_eq!(
        shell.eval("show").text,
        "script 1: reads memory\nmodel says: Prints free memory.\n 1 | print(\"free: \" + memory().free_mib + \" MiB\");\nran at once: it only reads"
    );
    assert_eq!(shell.last_output(), None);
    assert_eq!(shell.eval("answer 1 yes"), Reply::err("nothing to confirm"));
    assert!(
        shell
            .eval("log")
            .text
            .contains("#1 show me the free memory: ran read-only")
    );
    assert!(shell.eval("log 1").text.contains("memory().free_mib"));
    let audit = host.audit.borrow();
    assert!(
        audit
            .iter()
            .any(|line| line == "script 1 tier run; running it")
    );
}

#[test]
fn a_summary_cannot_add_lines_to_the_question() {
    let host: Rc<dyn Host> = FakeHost::new();
    let long = "s".repeat(llmproto::script::SUMMARY_MAX_CHARS + 1);
    for summary in ["Prints 1.\n 2 | set_policy(\"rr\");", long.as_str()] {
        let answer = Answer {
            summary: String::from(summary),
            script: String::from("print(1);"),
        };
        assert!(check(&answer, &host, &DEFAULT_CONFIG).is_err(), "{summary}");
    }
}

#[test]
fn one_pad_cannot_copy_long_keys_or_nested_arrays_a_thousand_times() {
    // Rhai's limits leave map keys out, and its pad checks only the new length.
    let keys = "let s = \"x\";\ns.pad(60000, \"x\");\nlet m = #{};\nm[s] = 1;\nlet a = [];\na.pad(1000, m);";
    let nested = "let b = [];\nb.pad(999, 0);\nlet a = [];\na.pad(1000, b);";
    for script in [keys, nested] {
        let why = checks(script).unwrap_err().to_string();
        assert!(why.contains("Size of array after pad"), "{why}");
    }
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.eval("let a = [1]");
    shell.eval("a.pad(3, 0)");
    assert_eq!(shell.eval("a.len()"), ok("3"));
    shell.eval("a.pad(2, 0)");
    assert_eq!(shell.eval("a.len()"), ok("3"));
}

#[test]
fn only_operator_arithmetic_is_put_down_to_stand_in_values() {
    // These fail whatever the numbers, and parse_int quotes its argument, which
    // could otherwise put a line break into the note.
    for bad in [
        "let x = parse_int(\"\n 1 | set_policy()\");",
        "let x = parse_int(\"zz\");",
    ] {
        assert!(checks(bad).is_err(), "{bad} was accepted");
    }
    let checked = checks("print(cpus() * 9223372036854775807);").unwrap();
    let note = checked.note.unwrap();
    assert!(note.contains("Multiplication overflow"), "{note}");
    assert!(!note.chars().any(char::is_control), "{note}");
}

#[test]
fn ctrl_c_during_the_trial_cancels_instead_of_asking_for_a_repair() {
    // Long enough to reach the interrupt check, which runs every CHECK_EVERY steps,
    // within 3 KiB (40 lines of 74 characters). The sums use a variable, since Rhai
    // folds constant ones while compiling.
    let line = format!("let b = a{};", " + a".repeat(16));
    let mut lines = vec![line.as_str(); llmproto::script::SCRIPT_MAX_LINES - 1];
    lines.insert(0, "let a = cpus();");
    let script = lines.join("\n");
    let (host, mut shell) = with_answers(vec![
        answer(&script, "Adds."),
        answer("print(1);", "Prints."),
    ]);
    // The shell's own line start and the trial's each clear an old Ctrl-C first.
    host.interrupt_in.set(Some(2));
    assert_eq!(
        shell.eval("? add a lot of ones"),
        Reply::err("stopped: interrupted")
    );
    assert_eq!(host.script_requests.borrow().len(), 1);
}

#[test]
fn a_for_loop_over_a_list_and_floor_run() {
    let script = "let names = \"\";\nfor p in processes() {\n    names += p.name + \" \";\n}\n\
                  for i in 0..2 {\n    names += i;\n}\nprint(names + \" \" + floor(7 / 2));";
    let checked = checks(script).unwrap();
    assert_eq!(checked.reads, ["processes"]);
    let (_host, mut shell) = with_answers(vec![answer(script, "Lists names.")]);
    assert_eq!(ran(&shell.eval("? name every service")), "llm 01 3\n");
}

#[test]
fn fixed_writes_decimals_with_leading_zeros() {
    let script = "print(fixed(4021, 3) + \" \" + fixed(-5, 2) + \" \" + fixed(7, 0) + \" \" + fixed(123, 1));";
    let (_host, mut shell) = with_answers(vec![answer(script, "Prints seconds.")]);
    assert_eq!(
        ran(&shell.eval("? uptime with three decimals")),
        "4.021 -0.05 7 12.3\n"
    );
}

#[test]
fn fixed_refuses_too_many_places() {
    let (host, mut shell) = with_answers(vec![
        answer("print(fixed(1, 10));", "Prints a number."),
        answer("print(fixed(1, 9));", "Prints a number."),
    ]);
    assert_eq!(ran(&shell.eval("? a tiny number")), "0.000000001\n");
    let requests = host.script_requests.borrow();
    let (_, error) = requests[1].repair.clone().unwrap();
    assert!(error.contains("places must be 0 to 9"), "{error}");
}
