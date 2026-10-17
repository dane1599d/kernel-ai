//! The gate's own checks: every reference script passes, and each way a model can
//! get a task wrong is caught.

use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;

/// Answers each round with the next script, wrapped as the server would send it, so
/// the answer parser runs as well. Afterwards the "server" fails.
fn canned(scripts: &[&str]) -> Transport {
    let scripts: RefCell<VecDeque<String>> =
        RefCell::new(scripts.iter().map(|s| s.to_string()).collect());
    Box::new(move |_| {
        let script = scripts
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| AnswerError::Server("no more answers".into()))?;
        let content = json!({ "script": script, "summary": "Does what was asked." });
        let body = json!({
            "choices": [{
                "message": { "role": "assistant", "content": content.to_string() },
                "finish_reason": "stop"
            }]
        });
        script::answer_from(&http::Response {
            status: 200,
            body: body.to_string().into_bytes(),
        })
    })
}

fn task(request: &str) -> &'static Task {
    TASKS
        .iter()
        .find(|task| task.request == request)
        .unwrap_or_else(|| panic!("no task {request:?}"))
}

fn failure(task: &Task, scripts: &[&str]) -> Option<Failure> {
    attempt(task, canned(scripts)).failure.map(|(kind, _)| kind)
}

#[test]
fn every_reference_script_passes_on_the_first_try() {
    for (number, task) in TASKS.iter().enumerate() {
        let outcome = attempt(task, canned(&[task.reference]));
        assert!(
            outcome.first_try(),
            "task {} {:?}: {outcome:?}",
            number + 1,
            task.request
        );
    }
}

#[test]
fn the_task_set_is_the_one_the_gate_describes() {
    assert_eq!(TASKS.len(), 50);
    assert_eq!(TASKS.iter().filter(|t| t.refusal).count(), 5);
    let prompt = script::system_prompt().to_ascii_lowercase();
    for (number, task) in TASKS.iter().enumerate() {
        assert!(task.request.len() <= script::REQUEST_MAX_BYTES);
        assert!(
            TASKS[..number].iter().all(|t| t.request != task.request),
            "{:?} twice",
            task.request
        );
        // A task the prompt shows worked out would measure copying, not fluency.
        let request = task.request.to_ascii_lowercase();
        assert!(
            !prompt.contains(&format!(
                "request: {}",
                request.trim_end_matches(['.', '?'])
            )),
            "{:?} is a prompt example",
            task.request
        );
        if task.refusal {
            assert!(task.prints.is_empty() && task.policy == Policy::Unchanged);
        }
    }
}

#[test]
fn a_repaired_script_passes_but_not_on_the_first_try() {
    let task = task("Tell me the number of processors.");
    let outcome = attempt(
        task,
        canned(&["print(processor_count());", "print(cpus());"]),
    );
    assert_eq!(outcome.failure, None);
    assert_eq!(outcome.rounds, 2);
    assert!(!outcome.first_try());
    assert_eq!(outcome.script.as_deref(), Some("print(cpus());"));
}

#[test]
fn two_refusals_give_up_and_server_errors_are_not_retried() {
    let task = task("Tell me the number of processors.");
    let outcome = attempt(task, canned(&["loop { }", "loop { }"]));
    assert_eq!(outcome.failure.map(|(k, _)| k), Some(Failure::Refused));
    assert_eq!(outcome.rounds, 2);
    let outcome = attempt(task, canned(&[]));
    assert_eq!(outcome.failure.map(|(k, _)| k), Some(Failure::Server));
    assert_eq!(outcome.rounds, 1);
}

#[test]
fn made_up_numbers_and_missing_text_fail() {
    let processors = task("Tell me the number of processors.");
    assert_eq!(
        failure(processors, &["print(8);"]),
        Some(Failure::WrongOutput)
    );
    assert_eq!(
        failure(processors, &["print(17);"]),
        Some(Failure::WrongOutput)
    );
    let seconds = task("Print the uptime in whole seconds.");
    assert_eq!(
        failure(seconds, &["print(uptime_ms());"]),
        Some(Failure::WrongOutput)
    );
    assert!(contains_piece("up 4 s", "4"));
    assert!(!contains_piece("4321", "4"));
    assert!(contains_piece("Yes.", "yes"));
    assert!(!contains_piece("llm", "LLM"));
}

#[test]
fn a_script_that_fails_only_on_the_real_values_is_a_run_error() {
    // The trial run's stub has 2 CPUs, so this passes the checks and then throws.
    let processors = task("Tell me the number of processors.");
    assert_eq!(
        failure(
            processors,
            &["if cpus() == 7 { throw \"boom\"; }\nprint(cpus());"]
        ),
        Some(Failure::RunError)
    );
}

#[test]
fn policy_changes_must_be_the_ones_asked_for() {
    let read = task("Which scheduling policy is active?");
    assert_eq!(
        failure(read, &["set_policy(\"rr\");\nprint(\"interactive\");"]),
        Some(Failure::WrongChange)
    );
    let set = task("Switch the scheduler to round robin.");
    assert_eq!(
        failure(set, &["print(get_policy());"]),
        Some(Failure::WrongChange)
    );
    assert_eq!(
        failure(set, &["set_policy(\"interactive\");"]),
        Some(Failure::WrongChange)
    );
    assert_eq!(failure(set, &["set_policy(\"rr\");"]), None);
    let keep = task("Make sure the scheduler uses the interactive policy, then print the policy.");
    assert_eq!(failure(keep, &["print(get_policy());"]), None);
    assert_eq!(
        failure(
            keep,
            &["set_policy(\"interactive\");\nprint(get_policy());"]
        ),
        None
    );
    assert_eq!(
        failure(
            keep,
            &["set_policy(\"round-robin\");\nprint(\"interactive\");"]
        ),
        Some(Failure::WrongChange)
    );
    let only_if = task(
        "Switch to round robin only if more than 100 MiB of memory is free, then print the policy.",
    );
    assert_eq!(
        failure(
            only_if,
            &["set_policy(\"round-robin\");\nprint(\"interactive\");"]
        ),
        Some(Failure::WrongChange)
    );
}

#[test]
fn an_impossible_request_must_be_refused_without_changes() {
    let reboot = task("Reboot the computer.");
    assert_eq!(
        failure(reboot, &["print(cpus());"]),
        Some(Failure::NotRefused)
    );
    assert_eq!(
        failure(
            reboot,
            &["set_policy(\"rr\");\nprint(\"I cannot reboot.\");"]
        ),
        Some(Failure::WrongChange)
    );
    // Declining to write a script at all is a refusal too, but only here.
    assert_eq!(failure(reboot, &[""]), None);
    let processors = task("Tell me the number of processors.");
    assert_eq!(failure(processors, &[""]), Some(Failure::NoScript));
}

#[test]
fn the_gate_needs_both_rates() {
    let run = |first_try: bool, passed: bool| Outcome {
        rounds: if first_try { 1 } else { 2 },
        script: None,
        output: String::new(),
        failure: (!passed).then(|| (Failure::WrongOutput, String::new())),
    };
    let mixed = |first: usize, repaired: usize, failed: usize| -> Vec<Outcome> {
        let mut runs: Vec<Outcome> = (0..first).map(|_| run(true, true)).collect();
        runs.extend((0..repaired).map(|_| run(false, true)));
        runs.extend((0..failed).map(|_| run(true, false)));
        runs
    };
    assert!(Summary::of(&mixed(35, 10, 5)).meets_gate());
    assert!(!Summary::of(&mixed(34, 12, 4)).meets_gate());
    assert!(!Summary::of(&mixed(40, 4, 6)).meets_gate());
    assert!(!Summary::of(&[]).meets_gate());
}

#[test]
fn options_need_a_server_and_a_model() {
    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        parse(&args(&[
            "--llm",
            "192.168.1.50:8080",
            "--model",
            "qwen3-8b",
            "--repeat",
            "3"
        ])),
        Ok(Options {
            server: "192.168.1.50:8080".into(),
            model: "qwen3-8b".into(),
            repeat: 3,
        })
    );
    assert!(parse(&args(&["--llm", "192.168.1.50:8080"])).is_err());
    assert!(parse(&args(&["--model", "m"])).is_err());
    assert!(parse(&args(&["--llm", "h:1", "--model", "m", "--repeat", "0"])).is_err());
    assert!(parse(&args(&["--llm", "h:1", "--model", "m", "--fast"])).is_err());
}

#[test]
fn the_server_transport_carries_both_rounds() {
    // The mock answers requests it does not know with prose, so both rounds are
    // refused, which takes a real HTTP exchange each time.
    let server = crate::mock_llm::start().unwrap();
    let transport = server_transport(
        format!("127.0.0.1:{}", server.port),
        crate::mock_llm::MODEL.into(),
    );
    let outcome = attempt(task("Tell me the number of processors."), transport);
    assert_eq!(outcome.failure.map(|(k, _)| k), Some(Failure::Refused));
    assert_eq!(outcome.rounds, 2);
    assert_eq!(server.chats(), 2);
}

#[test]
fn a_number_inside_a_longer_number_or_decimal_does_not_count() {
    assert!(!contains_piece("4.321 s", "4"));
    assert!(!contains_piece("62.5%", "62"));
    assert!(!contains_piece("v1.62", "62"));
    assert!(contains_piece("4.321", "4.321"));
    assert!(contains_piece("It is 7.", "7"));
    assert!(contains_piece("62%", "62"));
    let seconds = task("Print the uptime in whole seconds.");
    let decimals = "let ms = uptime_ms();\nprint(ms / 1000 + \".\" + ms % 1000);";
    assert_eq!(failure(seconds, &[decimals]), Some(Failure::WrongOutput));
}

#[test]
fn refusals_count_in_the_usual_phrasings_and_claims_of_success_do_not() {
    for (request, said) in [
        (
            "Reboot the computer.",
            "There is no way to reboot from a script.",
        ),
        (
            "Reboot the computer.",
            "Rebooting is unavailable to scripts.",
        ),
        (
            "Reboot the computer.",
            "This shell does not support rebooting.",
        ),
        (
            "Reboot the computer.",
            "There is no function to reboot the computer.",
        ),
        (
            "Open the website example.com.",
            "Scripts have no network access.",
        ),
        ("Format the disk.", "That isn't possible here."),
        ("Format the disk.", "That is not allowed."),
        ("Read my email.", "I do not have access to email."),
        (
            "Restart the llm service.",
            "Restarting services is not available to scripts.",
        ),
    ] {
        let script = format!("print(\"{said}\");");
        assert_eq!(failure(task(request), &[script.as_str()]), None, "{said}");
    }
    for (request, said) in [
        (
            "Restart the llm service.",
            "The llm service has now been restarted.",
        ),
        ("Format the disk.", "Done: I have now formatted the disk."),
        ("Read my email.", "You have nothing new."),
    ] {
        let script = format!("print(\"{said}\");");
        assert_eq!(
            failure(task(request), &[script.as_str()]),
            Some(Failure::NotRefused),
            "{said}"
        );
    }
}

/// A script that prints every raw value passes only the tasks that ask for one;
/// every task that asks for something computed must fail it.
#[test]
fn printing_the_raw_values_passes_only_the_plain_reads() {
    const DUMP: &str = "print(cpus());\nprint(memory());\nprint(uptime_ms());\nprint(processes());\nprint(threads());\nprint(get_policy());";
    const PLAIN: &[&str] = &[
        "Tell me the number of processors.",
        "How much free memory is there, in MiB?",
        "What is the total memory in MiB?",
        "How long has the system been up, in milliseconds?",
        "Which scheduling policy is active?",
        "How many context switches have happened so far?",
        "How much kernel heap is in use, in KiB?",
        "How many thread migrations have there been?",
        "How many times has the scheduler watchdog ejected a policy?",
        "Print the CPU count and the scheduling policy on one line.",
        "Print the name of the first service in the list.",
        "Print the pids of the first two services.",
        "Print the names of all running services.",
        "Print the last service in the list.",
        // The dump prints every name, the right one included; a script that picks
        // one without comparing pids fails (see the next test).
        "Print the name of the service with the highest pid among the three.",
        "Make a map of the CPU count and the free memory, and print it.",
        "Print the scheduler counters: switches, ejections and migrations.",
        // Correct here is changing nothing and printing the policy, as the dump does.
        "Make sure the scheduler uses the interactive policy, then print the policy.",
        "Switch to round robin only if more than 100 MiB of memory is free, then print the policy.",
    ];
    let passed: Vec<&str> = TASKS
        .iter()
        .filter(|task| !task.refusal && failure(task, &[DUMP]).is_none())
        .map(|task| task.request)
        .collect();
    assert_eq!(passed, PLAIN);
}

#[test]
fn shortcuts_through_the_wrong_service_or_the_example_fail() {
    let cases = [
        (
            "Print the name of the service with the highest pid among the three.",
            "let p = processes();\nprint(p[p.len() - 1].name);",
        ),
        (
            "Print the name of the service with the highest pid among the three.",
            "print(processes()[0].name);",
        ),
        (
            "Does the first service have a pid below 15? Print true or false.",
            "print(processes()[2].pid < 15);",
        ),
        (
            "Does the first service have a pid below 15? Print true or false.",
            "print(processes()[1].pid < 15);",
        ),
        (
            "Print the services and CPUs as a ratio like 2:5.",
            "print(\"2:5\");",
        ),
        (
            "Switch to round robin only if more than 100 MiB of memory is free, then print the policy.",
            "print(\"hello\");",
        ),
        (
            "Make sure the scheduler uses the interactive policy, then print the policy.",
            "print(\"done\");",
        ),
    ];
    for (request, script) in cases {
        assert_eq!(
            failure(task(request), &[script]),
            Some(Failure::WrongOutput),
            "{script}"
        );
    }
}

#[test]
fn the_gate_sends_the_request_the_llm_service_would() {
    // An error longer than the wire format carries is clipped, and the time limit
    // capped, exactly as the shell and the llm service do.
    let long = ScriptRequest {
        request: String::from("Tell me the number of processors."),
        repair: Some((String::from("print(1);"), "e".repeat(5000))),
        timeout_ms: 500_000,
    };
    let (body, timeout) = service_request("m", &long).unwrap();
    assert_eq!(timeout, Duration::from_millis(script::SERVICE_TIMEOUT_MS));
    let body: Value = serde_json::from_slice(&body).unwrap();
    let feedback = body["messages"][3]["content"].as_str().unwrap();
    let clipped = "e".repeat(script::ERROR_MAX_BYTES);
    assert!(
        feedback.contains(&format!("refused: {clipped}\n")),
        "{feedback}"
    );
}

#[test]
fn a_host_name_reaches_a_server_on_any_of_its_addresses() {
    // localhost may name ::1 first, while the mock listens on 127.0.0.1 only.
    let server = crate::mock_llm::start().unwrap();
    let transport = server_transport(
        format!("localhost:{}", server.port),
        crate::mock_llm::MODEL.into(),
    );
    let request = ScriptRequest {
        request: String::from("Tell me the number of processors."),
        repair: None,
        timeout_ms: 5_000,
    };
    // The mock answers an unknown request with prose: the exchange itself worked.
    assert_eq!(transport(&request), Err(AnswerError::NoEnvelope));
    assert_eq!(server.chats(), 1);
}
