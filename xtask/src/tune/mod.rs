//! `cargo xtask tune --sim [--mix A|B] [--seed N] (--local ID | --answer FILE)
//! [--store DIR] [--intent TEXT] [--yes] [--eval]`: the host rehearsal of M10a.
//! In the OS (part 2): `tune demo` (the model at the terminal, [`demo`]), `tune
//! check` (guest equals host, [`parity`]), `tune record` and `tune ab` (the
//! exit test under icount, [`record`], [`abrun`], [`ab`]).
//!
//! The mix runs in the simulator on the real `sched` code under Interactive; code
//! builds the summary (`llmproto::tune`); the model (or a canned answer) chooses;
//! the proposal card is shown and the trial waits for a typed yes; the simulated
//! trial runs the real judge and prints a line per window and a verdict card.
//! Next to it: what the burst rule, the designers' pick and the best of every
//! candidate do on the same seed. It is a rehearsal: the model controls the
//! scheduler code on the host, not the running OS. `--eval` asks for each of five
//! seeds of both mixes and counts how often the model's choice is kept.

mod ab;
mod abrun;
mod ask;
mod boot;
mod demo;
mod observe;
pub mod parity;
mod record;
#[cfg(test)]
mod tests;

use ask::{Ask, Canned, Local};
use llmproto::text;
use llmproto::tune::{self, Decision, Summary};
use sched::judge::{self, Choice, TrialPlan, Unloaded, Verdict};
use sched::spec::{PolicyKind, PolicySpec, Preset};
use sched::workload::{MIXES, Mix};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const USAGE: &str = "usage: cargo xtask tune --sim [--mix A|B] [--seed N] \
                     (--local ID | --answer FILE) [--store DIR] [--intent TEXT] [--yes] [--eval]";
const EVAL_SEEDS: [u64; 5] = [1, 2, 3, 4, 5];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local { id: String, store: Option<String> },
    Answer(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub mix: Mix,
    pub seed: u64,
    pub source: Source,
    pub intent: Option<String>,
    pub yes: bool,
    pub eval: bool,
}

pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut rest = args.iter();
    let (mut sim, mut mix, mut seed) = (false, sched::workload::MIX_A, 1);
    let (mut local, mut answer, mut store, mut intent) = (None, None, None, None);
    let (mut yes, mut eval) = (false, false);
    while let Some(arg) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .cloned()
                .ok_or(format!("{name} expects a value"))
        };
        match arg.as_str() {
            "--sim" => sim = true,
            "--mix" => {
                let name = value("--mix")?;
                mix = Mix::by_name(&name).ok_or(format!("--mix expects A or B, got {name:?}"))?;
            }
            "--seed" => {
                let text = value("--seed")?;
                seed = text
                    .parse()
                    .map_err(|_| format!("--seed expects a number, got {text:?}"))?;
            }
            "--local" => local = Some(value("--local")?),
            "--answer" => answer = Some(PathBuf::from(value("--answer")?)),
            "--store" => store = Some(value("--store")?),
            "--intent" => intent = Some(value("--intent")?),
            "--yes" => yes = true,
            "--eval" => eval = true,
            "--model" => return Err("--model names a server's model; use --local ID".into()),
            other => return Err(format!("unknown option {other}; {USAGE}")),
        }
    }
    if !sim {
        return Err(format!(
            "only --sim exists until the kernel runs trials itself (M10a-3); {USAGE}"
        ));
    }
    if eval && !yes {
        return Err("--eval runs every trial it is given without asking; add --yes".into());
    }
    let source = match (local, answer) {
        (Some(id), None) => Source::Local { id, store },
        (None, Some(path)) if store.is_none() => Source::Answer(path),
        (None, Some(_)) => return Err("--store goes with --local".into()),
        _ => return Err(format!("give one of --local ID or --answer FILE; {USAGE}")),
    };
    Ok(Args {
        mix,
        seed,
        source,
        intent,
        yes,
        eval,
    })
}

pub fn main(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("check") => return parity::main(&args[1..]),
        Some("demo") => return demo::main(&args[1..]),
        Some("ab") => return abrun::main(&args[1..]),
        Some("record") => return record::main(&args[1..]),
        _ => {}
    }
    let args = parse(args)?;
    let mut asker: Box<dyn Ask> = match &args.source {
        Source::Answer(path) => Box::new(Canned::from_file(path)?),
        Source::Local { id, store } => {
            let model = crate::models::ModelArgs {
                local: Some(id.clone()),
                store: store.clone(),
                ..Default::default()
            };
            let loaded = crate::models::Loaded::open(&model)?;
            let label = loaded.label.clone();
            Box::new(Local::new(label, loaded.into_bytes(), quant::Tier::best())?)
        }
    };
    let mut out = std::io::stdout();
    if args.eval {
        return evaluate(&args, asker.as_mut(), &mut out);
    }
    let stdin = std::io::stdin();
    let mut confirm = || {
        print!("Type yes to start the trial: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = stdin.lock().read_line(&mut line);
        line.trim() == "yes"
    };
    rehearse(&args, asker.as_mut(), &mut out, &mut confirm).map(|_| ())
}

/// What a rehearsal decided and measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub decision: Decision,
    /// The choice tried and the judge's verdict, if a trial ran.
    pub trial: Option<(Choice, Verdict)>,
}

/// The labels a decision names, as a mask over the mix.
fn choice_of(decision: &Decision, summary: &Summary) -> Option<Choice> {
    let Decision::Try {
        spec, background, ..
    } = decision
    else {
        return None;
    };
    let mask = background
        .iter()
        .filter_map(|name| summary.index_of(name))
        .fold(0u32, |mask, index| mask | 1 << index);
    Some(Choice::new(*spec, mask))
}

fn names(mix: &Mix) -> Vec<&'static str> {
    mix.procs.iter().map(|p| p.name).collect()
}

fn describe(mix: &Mix, choice: &Choice) -> String {
    let labelled: Vec<&str> = (0..mix.procs.len())
        .filter(|&i| choice.labelled(i))
        .map(|i| mix.procs[i].name)
        .collect();
    match labelled.is_empty() {
        true => format!("{}", choice.spec.label()),
        false => format!(
            "{} with {} background",
            choice.spec.label(),
            labelled.join(", ")
        ),
    }
}

fn io(error: std::io::Error) -> String {
    format!("cannot write the report: {error}")
}

/// One rehearsal: observe, ask, show the card, confirm, run the trial, judge.
pub fn rehearse(
    args: &Args,
    asker: &mut dyn Ask,
    out: &mut dyn Write,
    confirm: &mut dyn FnMut() -> bool,
) -> Result<Outcome, String> {
    let mix = &args.mix;
    let observation = observe::observe(mix, args.seed, args.intent.clone());
    let summary = asker
        .summarize(&observation)
        .ok_or("the target is not one of the observed processes")?;
    let request = summary.wire();
    writeln!(
        out,
        "xtask: mix {} seed {}, observed for 3 s under interactive:",
        mix.name, args.seed
    )
    .map_err(io)?;
    for line in summary.text.lines() {
        writeln!(out, "  > {line}").map_err(io)?;
    }
    let (completion, how) = asker.ask(&request)?;
    writeln!(out, "xtask: asked {how}").map_err(io)?;
    writeln!(out, "xtask: the answer, as written:").map_err(io)?;
    for line in text::printable(completion.text.trim(), 1000, 12).lines() {
        let line: String = line
            .chars()
            .map(|ch| if (' '..='~').contains(&ch) { ch } else { '?' })
            .collect();
        writeln!(out, "  | {line}").map_err(io)?;
    }
    let decision = tune::decide(&completion, &summary, observation.current, &[]);
    let measured = tune::measured(&observation, &summary);
    writeln!(out).map_err(io)?;
    for line in tune::card(&summary, &measured, &decision, 1) {
        writeln!(out, "{line}").map_err(io)?;
    }
    let Some(choice) = choice_of(&decision, &summary) else {
        writeln!(out, "xtask: nothing to try; the policy stays as it is.").map_err(io)?;
        return Ok(Outcome {
            decision,
            trial: None,
        });
    };
    if !args.yes && !confirm() {
        writeln!(out, "xtask: cancelled; nothing ran.").map_err(io)?;
        return Ok(Outcome {
            decision,
            trial: None,
        });
    }
    let plan = TrialPlan::STANDARD;
    let alone = Unloaded::run(mix, args.seed, &plan);
    let trial = judge::run_trial(mix, args.seed, &Choice::DEFAULT, &choice, &plan, &alone)
        .map_err(|e| e.describe().to_string())?;
    let target = mix.procs[mix.target].name;
    for (i, pair) in trial.pairs.iter().enumerate() {
        for (side, window) in [("baseline", &pair.baseline), ("candidate", &pair.candidate)] {
            let background: u64 = (0..mix.procs.len())
                .filter(|&p| p != mix.target)
                .map(|p| window.cpu_ns[p])
                .sum();
            writeln!(
                out,
                "window {}/{} {side}: \"{target}\" p99 latency {} ms over its time alone, background CPU {}%",
                2 * i + usize::from(side == "candidate") + 1,
                2 * trial.pairs.len(),
                tune::two_sig(window.p99() / 1000, 3),
                tune::two_sig(background * 1000 / (window.to - window.from), 1),
            )
            .map_err(io)?;
        }
    }
    for line in tune::verdict_card(choice.spec, &names(mix), target, &trial.verdict) {
        writeln!(out, "{line}").map_err(io)?;
    }
    baselines(mix, args.seed, &alone, out)?;
    Ok(Outcome {
        decision,
        trial: Some((choice, trial.verdict)),
    })
}

/// What the rule-based baseline, the designers' pick and the best of every
/// candidate do on the same seed.
fn baselines(mix: &Mix, seed: u64, alone: &Unloaded, out: &mut dyn Write) -> Result<(), String> {
    let plan = TrialPlan::STANDARD;
    let target = mix.procs[mix.target].name;
    let burst = Choice::new(PolicySpec::new(PolicyKind::Responsive, Preset::Gentle), 0);
    let oracle = mix.oracle_choice();
    writeln!(out, "xtask: next to it, on the same seed:").map_err(io)?;
    for (label, choice) in [("the burst rule", burst), ("the designers' pick", oracle)] {
        if choice == Choice::DEFAULT {
            writeln!(out, "  {label}: keep interactive").map_err(io)?;
            continue;
        }
        let trial = judge::run_trial(mix, seed, &Choice::DEFAULT, &choice, &plan, alone)
            .map_err(|e| e.describe().to_string())?;
        let card = tune::verdict_card(choice.spec, &names(mix), target, &trial.verdict);
        let card: Vec<&str> = card.iter().map(|line| line.trim()).collect();
        writeln!(
            out,
            "  {label} ({}): {}",
            describe(mix, &choice),
            card.join(" ")
        )
        .map_err(io)?;
    }
    let candidates = judge::candidates(mix);
    let scores =
        judge::evaluate(mix, &[seed], &candidates, &plan).map_err(|e| e.describe().to_string())?;
    match judge::best(&scores) {
        Some(best) => writeln!(
            out,
            "  the best of {} candidates: {}, p99 {}x of interactive's",
            candidates.len(),
            describe(mix, &best.choice),
            tune::two_sig(best.mean_ratio_permille(), 3)
        ),
        None => writeln!(
            out,
            "  the best of {} candidates: keep interactive; nothing passes the judge",
            candidates.len()
        ),
    }
    .map_err(io)
}

/// `--eval`: five seeds of each mix, each answered and tried; how often the
/// chosen change is kept, next to the designers' pick and the burst rule. It
/// runs every trial without a card or a question, which is why it needs --yes.
fn evaluate(args: &Args, asker: &mut dyn Ask, out: &mut dyn Write) -> Result<(), String> {
    let plan = TrialPlan::STANDARD;
    let who = match args.source {
        Source::Answer(_) => "the canned answer",
        Source::Local { .. } => "the model",
    };
    let (mut runs, mut tried, mut kept, mut invalid) = (0, 0, 0, 0);
    let mut provenance = None;
    for mix in MIXES {
        let burst = Choice::new(PolicySpec::new(PolicyKind::Responsive, Preset::Gentle), 0);
        let oracle = mix.oracle_choice();
        let fixed = judge::evaluate(&mix, &EVAL_SEEDS, &[oracle, burst], &plan)
            .map_err(|e| e.describe().to_string())?;
        for (i, &seed) in EVAL_SEEDS.iter().enumerate() {
            runs += 1;
            let observation = observe::observe(&mix, seed, args.intent.clone());
            let summary = asker
                .summarize(&observation)
                .ok_or("the target is not one of the observed processes")?;
            let (completion, how) = asker.ask(&summary.wire())?;
            if provenance.is_none() {
                writeln!(out, "xtask: answers from {how}").map_err(io)?;
                provenance = Some(how);
            }
            let decision = tune::decide(&completion, &summary, observation.current, &[]);
            let verdict = match choice_of(&decision, &summary) {
                Some(choice) => {
                    tried += 1;
                    let alone = Unloaded::run(&mix, seed, &plan);
                    let trial =
                        judge::run_trial(&mix, seed, &Choice::DEFAULT, &choice, &plan, &alone)
                            .map_err(|e| e.describe().to_string())?;
                    kept += usize::from(trial.verdict.kept);
                    let result = if trial.verdict.kept {
                        "kept"
                    } else {
                        "rolled back"
                    };
                    format!("{}: {result}", describe(&mix, &choice))
                }
                None => match &decision {
                    Decision::Keep {
                        invalid: Some(why), ..
                    } => {
                        invalid += usize::from(why != tune::UNCHANGED);
                        format!("keep ({why})")
                    }
                    _ => String::from("keep"),
                },
            };
            let fixed_kept = |score: &judge::Score, choice: &Choice| match *choice {
                Choice::DEFAULT => "keep",
                _ if score.verdicts[i].kept => "kept",
                _ => "rolled back",
            };
            writeln!(
                out,
                "mix {} seed {seed}: {who} {verdict}; designers' pick {}; burst rule {}",
                mix.name,
                fixed_kept(&fixed[0], &oracle),
                fixed_kept(&fixed[1], &burst)
            )
            .map_err(io)?;
        }
    }
    writeln!(
        out,
        "xtask: {runs} rehearsals: {who} tried {tried} changes and the judge kept {kept}; \
         {invalid} answers were invalid and changed nothing"
    )
    .map_err(io)
}
