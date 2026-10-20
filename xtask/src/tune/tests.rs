//! The whole `tune --sim` pipeline with canned answers: a good pick is kept,
//! round-robin is rolled back, and an invalid answer changes nothing.

use super::*;
use sched::judge::Reason;

fn args(source: &str) -> Args {
    Args {
        mix: sched::workload::MIX_A,
        seed: 2,
        source: Source::Answer(PathBuf::from(source)),
        intent: Some(String::from("make typing snappier while the build runs")),
        yes: true,
        eval: false,
    }
}

fn canned(text: &str) -> Canned {
    Canned {
        text: String::from(text),
        from: String::from("the test"),
    }
}

fn answer(policy: &str, preset: &str, background: &[&str]) -> String {
    serde_json::json!({
        "reason": "The build jobs wake often and delay the editor.",
        "action": "try",
        "policy": policy,
        "preset": preset,
        "background": background,
    })
    .to_string()
}

fn run(text: &str) -> (Outcome, String) {
    let mut out = Vec::new();
    let outcome = rehearse(&args("x"), &mut canned(text), &mut out, &mut || true).unwrap();
    (outcome, String::from_utf8(out).unwrap())
}

#[test]
fn a_good_pick_is_kept() {
    let builds = ["build-1", "build-2", "build-3", "build-4"];
    let (outcome, report) = run(&answer("classes", "firm", &builds));
    println!("{report}");
    let (choice, verdict) = outcome.trial.expect("a trial ran");
    assert_eq!(choice, sched::workload::MIX_A.oracle_choice());
    assert!(verdict.kept, "{verdict:?}");
    assert!(report.contains("The model says (its claim, not checked): The build jobs"));
    assert!(report.contains("  | {\"action\":\"try\""));
    assert!(report.contains("Kept classes/firm: \"editor\" p99 latency"));
    assert!(report.contains("window 6/6 candidate"));
    assert!(report.contains("the burst rule (responsive): Rolled back"));
    assert!(report.contains("the best of 67 candidates: classes/"));
}

#[test]
fn round_robin_is_rolled_back() {
    let (outcome, report) = run(&answer("round-robin", "gentle", &[]));
    let (_, verdict) = outcome.trial.expect("a trial ran");
    assert_eq!(verdict.reason, Some(Reason::NotFaster));
    assert!(report.contains("Rolled back round-robin: \"editor\" p99 went from"));
}

#[test]
fn an_invalid_answer_changes_nothing() {
    for text in [
        "Sure! I would label the builds as background.",
        &answer("classes", "firm", &["editor"]),
        &answer("classes", "turbo", &["build-1"]),
        &answer("interactive", "gentle", &["build-1"]),
    ] {
        let (outcome, report) = run(text);
        assert!(outcome.trial.is_none(), "{text}");
        assert!(matches!(
            outcome.decision,
            Decision::Keep {
                invalid: Some(_),
                ..
            }
        ));
        assert!(report.contains("Kept the current policy:"), "{report}");
        assert!(report.contains("nothing to try"));
    }
}

#[test]
fn keep_and_a_refused_yes_run_nothing() {
    let keep = r#"{"reason":"Nothing is flagged.","action":"keep","policy":"interactive","preset":"gentle","background":[]}"#;
    assert!(run(keep).0.trial.is_none());
    let mut out = Vec::new();
    let mut unconfirmed = args("x");
    unconfirmed.yes = false;
    let outcome = rehearse(
        &unconfirmed,
        &mut canned(&answer("classes", "firm", &["build-1"])),
        &mut out,
        &mut || false,
    )
    .unwrap();
    assert!(outcome.trial.is_none());
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("cancelled; nothing ran")
    );
}

#[test]
fn eval_runs_both_mixes_with_a_canned_answer() {
    let mut out = Vec::new();
    let builds = answer(
        "classes",
        "firm",
        &["build-1", "build-2", "build-3", "build-4"],
    );
    let mut eval = args("x");
    eval.eval = true;
    evaluate(&eval, &mut canned(&builds), &mut out).unwrap();
    let report = String::from_utf8(out).unwrap();
    println!("{report}");
    // Mix A keeps the pick on every seed; on mix B the names are not in the
    // table, so the answer is refused and nothing changes.
    assert!(
        report.contains("10 rehearsals: the canned answer tried 5 changes and the judge kept 5")
    );
    assert!(report.contains("answers from a canned answer from the test"));
    assert!(report.contains("mix B seed 1: the canned answer keep"));
    assert!(report.contains("designers' pick keep; burst rule"));
    assert!(report.contains("5 answers were invalid"));
}

#[test]
fn options_parse_and_need_a_source() {
    let words = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let parsed = parse(&words(&[
        "--sim",
        "--mix",
        "b",
        "--seed",
        "4",
        "--local",
        "qwen3-0.6b",
    ]))
    .unwrap();
    assert_eq!(parsed.mix.name, "B");
    assert_eq!(parsed.seed, 4);
    assert_eq!(
        parsed.source,
        Source::Local {
            id: String::from("qwen3-0.6b"),
            store: None
        }
    );
    assert!(
        parse(&words(&["--mix", "A", "--answer", "a.json"])).is_err(),
        "--sim is required"
    );
    assert!(parse(&words(&["--sim"])).is_err());
    assert!(parse(&words(&["--sim", "--local", "x", "--answer", "a.json"])).is_err());
    assert!(parse(&words(&["--sim", "--answer", "a", "--store", "D:/m"])).is_err());
    assert!(parse(&words(&["--sim", "--mix", "C", "--answer", "a"])).is_err());
    assert!(parse(&words(&["--sim", "--model", "q", "--answer", "a"])).is_err());
    let eval = words(&["--sim", "--eval", "--answer", "a"]);
    assert!(parse(&eval).unwrap_err().contains("--yes"));
    assert!(parse(&words(&["--sim", "--eval", "--yes", "--answer", "a"])).is_ok());
}

#[test]
fn the_summary_budgets_for_the_guests_context() {
    assert_eq!(tune::CONTEXT_TOKENS, kabi::model::CONTEXT_CAP);
}

/// Opt-in, as bpe's real-vocabulary test: with the real Qwen3 tokenizer, mix A's
/// prompt fits the guest's context with the estimate alone, and with real
/// counting even hostile 16-letter names fit. Set KAI_QWEN3_GGUF to the model
/// file (or its first 8 MB) and run with `--ignored`.
#[test]
#[ignore = "needs KAI_QWEN3_GGUF"]
fn the_real_tokenizer_fits_every_summary_in_the_guests_context() {
    let path = std::env::var("KAI_QWEN3_GGUF").expect("KAI_QWEN3_GGUF");
    let bytes = std::fs::read(path).unwrap();
    let header = gguf::Header::parse(&bytes).unwrap();
    let tok = bpe::Tokenizer::from_gguf(&header).unwrap();
    let chat = bpe::chat::Chat::new(&tok).unwrap();
    let count = |text: &str| {
        let mut ids = Vec::new();
        tok.encode(text, &mut ids);
        ids.len()
    };
    let budget = kabi::model::CONTEXT_CAP - tune::ANSWER_TOKENS as usize - tune::TEMPLATE_TOKENS;
    let prompt_tokens = |summary: &tune::Summary| {
        let request = tune::request(summary);
        let (system, user) = request.single_turn().unwrap();
        chat.prompt(&tok, system, user).len()
    };
    let fits = |real: usize| real + tune::ANSWER_TOKENS as usize <= kabi::model::CONTEXT_CAP;
    for mix in sched::workload::MIXES {
        let observation = observe::observe(&mix, 1, Some(String::from("make typing snappier")));
        let estimated = tune::summary(&observation).unwrap();
        let real = prompt_tokens(&estimated);
        println!(
            "mix {}: {real} real prompt tokens by the estimate's rows",
            mix.name
        );
        assert!(fits(real), "mix {}: {real}", mix.name);
        assert_eq!(estimated.omitted, 0);
        let counted = tune::summary_with(&observation, &count, budget).unwrap();
        assert_eq!(counted.omitted, 0);
    }
    // Hostile names: rare letters, all sixteen characters.
    let mut hostile = observe::observe(&sched::workload::MIX_A, 1, Some("xkqzjvwm".repeat(10)));
    for (i, row) in hostile.processes.iter_mut().enumerate() {
        row.name = format!("xkqzjvwmfgbtpd{i:02}");
    }
    let by_estimate = tune::summary(&hostile).unwrap();
    let counted = tune::summary_with(&hostile, &count, budget).unwrap();
    let (estimate_real, counted_real) = (prompt_tokens(&by_estimate), prompt_tokens(&counted));
    println!(
        "hostile names: {estimate_real} real tokens by the estimate's {} rows, \
         {counted_real} by real counting's {} rows",
        by_estimate.shown.len(),
        counted.shown.len()
    );
    assert!(fits(counted_real), "{counted_real}");
}

#[test]
fn the_local_engine_path_runs_and_an_overlong_prompt_is_inert() {
    // The local-ask fixture's context is 152 positions: the tune prompt cannot
    // fit, so the answer is cut off and the decision is a keep.
    let mut local = Local::new(
        String::from("the local-ask fixture"),
        infer::fixture::local_ask(),
        quant::Tier::BASELINE,
    )
    .unwrap();
    let mut out = Vec::new();
    let outcome = rehearse(&args("x"), &mut local, &mut out, &mut || true).unwrap();
    let report = String::from_utf8(out).unwrap();
    assert!(report.contains("the prompt fills the context"), "{report}");
    assert!(matches!(
        outcome.decision,
        Decision::Keep {
            invalid: Some(_),
            ..
        }
    ));
    assert!(outcome.trial.is_none());
    // The tiny fixture has 512 positions, too few for the prompt: it answers greedily with
    // random weights, which can only be refused.
    let mut tiny = Local::new(
        String::from("the tiny fixture"),
        infer::fixture::build(&infer::fixture::Spec::tiny()),
        quant::Tier::BASELINE,
    )
    .unwrap();
    let mut out = Vec::new();
    let outcome = rehearse(&args("x"), &mut tiny, &mut out, &mut || true).unwrap();
    assert!(
        outcome.trial.is_none(),
        "{}",
        String::from_utf8(out).unwrap()
    );
}

#[test]
fn asking_for_what_runs_now_is_not_counted_as_invalid() {
    let mut out = Vec::new();
    let unchanged = answer("interactive", "gentle", &[]);
    let mut eval = args("x");
    eval.eval = true;
    evaluate(&eval, &mut canned(&unchanged), &mut out).unwrap();
    let report = String::from_utf8(out).unwrap();
    assert!(report.contains("tried 0 changes"), "{report}");
    assert!(report.contains("0 answers were invalid"), "{report}");
}

#[test]
fn the_local_engine_decodes_when_the_prompt_fits() {
    let mut tiny = Local::new(
        String::from("the tiny fixture"),
        infer::fixture::build(&infer::fixture::Spec::tiny()),
        quant::Tier::BASELINE,
    )
    .unwrap();
    let request = llmproto::chat::Request::new(Some("s"), "hi", 8);
    let (completion, how) = tiny.ask(&request).unwrap();
    assert!(how.contains("it wrote"), "{how}");
    assert!(completion.cut_off || !completion.text.is_empty());
}

#[test]
fn an_overflowed_ring_falls_back_to_the_histogram_below_the_maximum() {
    let mut sim = sched::sim::Sim::new(
        sched::workload::MIX_A.procs,
        1,
        Box::new(sched::Interactive::new()),
    );
    sim.run_until(4_000 * sched::sim::MS);
    let window = sim
        .core()
        .snapshot(4_000 * sched::sim::MS)
        .since(&sched::stats::Snapshot::EMPTY);
    // The indexer woke far more than the ring's 256 samples.
    let tid = sim
        .tid(sched::workload::MIX_A.index("indexer").unwrap())
        .unwrap();
    let t = &window.threads[tid];
    assert!(t.sample_count() > sched::stats::SAMPLES as u64);
    let p99 = observe::wait_us(&sim, tid, t, 99);
    assert!(p99 > 0 && p99 <= t.max_latency_ns / 1000, "{p99}");
}
