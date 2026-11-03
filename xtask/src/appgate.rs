//! `cargo xtask app-gate --local ID [--store DIR]` (plan "Kai builds apps",
//! piece 5): 20 app requests through the shell's own `app_build`, each written
//! by the model with the `c-app-1` card on Kernel AI's engine at the OS's
//! context, checked, built by the real `cc` and repaired as the shell does,
//! within the terminal's wait. A built app is then started as comphost starts
//! a version and given a scripted click test: every button pressed and every
//! field typed into and submitted, each within its fuel. The gate passes when
//! [`GATE`]% of requests build, open and pass the click test. Each round's
//! time, answer tokens and any full context are recorded, since a full answer
//! takes minutes on the runner, in `target/appgate/<unix time>.jsonl`.

use crate::cc::app_run;
use crate::fluency::local::{self as engine_local, Shared};
use infer::Next;
use infer::chat::Step;
use llmproto::act::{ActError, ActRequest, Call};
use llmproto::script::{Answer, AnswerError, ScriptRequest};
use serde_json::{Value as Json, json};
use shellcore::ccbuild::CcBuilt;
use shellcore::{DEFAULT_CONFIG, Shell};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uikit::a11y::Role;
use uikit::widget::{UiEvent, Value};

/// Percent of requests that must build, open and pass the click test.
const GATE: usize = 80;

/// The requests, as (the app's name, what the person asks for).
pub const REQUESTS: [(&str, &str); 20] = [
    ("timer", "a countdown timer that starts at 60 seconds"),
    ("counter", "a counter with plus and minus buttons"),
    ("notes", "a notes list: type a note and add it"),
    ("units", "a unit converter from kilometres to miles"),
    ("tally", "a tally counter for three people"),
    ("todo", "a to-do list where I can tick items off"),
    ("dice", "a dice roller that shows a number from 1 to 6"),
    ("tip", "a tip calculator for a bill and a tip percent"),
    ("temps", "a Celsius to Fahrenheit converter"),
    ("score", "a scoreboard for two teams"),
    ("habits", "a habit tracker with three habits I can toggle"),
    ("shop", "a shopping list I can clear"),
    (
        "clicks",
        "a button that counts how many times it was pressed",
    ),
    ("words", "a word counter for text I type"),
    ("budget", "a budget that adds up expenses I type in"),
    ("quiz", "a yes or no quiz with three questions and a score"),
    ("water", "a water tracker: add a glass, eight a day"),
    ("lap", "a lap counter with a reset button"),
    ("guess", "a number guessing game from 1 to 100"),
    ("hello", "an app that says hello to the name I type"),
];

/// One round of the model's: its answer's length in tokens, its time, and
/// what went wrong, if anything.
#[derive(Clone, Debug)]
pub struct Round {
    pub ms: u128,
    pub tokens: u32,
    pub error: Option<String>,
    pub context_full: bool,
}

/// Script requests answered by the model, and the tokens each answer took.
pub type Transport = Box<dyn Fn(&ScriptRequest) -> (Result<Answer, AnswerError>, u32)>;

/// The stand-in OS for one request: the model, `pkg`'s next version, the
/// app's folder and `cc build`, each recorded.
pub struct AppHost {
    started: Instant,
    transport: Transport,
    call: RefCell<Option<Call>>,
    sources: RefCell<HashMap<String, String>>,
    pub rounds: RefCell<Vec<Round>>,
    pub artifact: RefCell<Option<Vec<u8>>>,
}

impl AppHost {
    /// A host whose act model calls `app_build(name, words)` once.
    pub fn new(name: &str, words: &str, transport: Transport) -> Self {
        let call = json!({"name": "app_build", "arguments": {"name": name, "request": words}});
        Self {
            started: Instant::now(),
            transport,
            call: RefCell::new(llmproto::act::parse(&call.to_string()).ok()),
            sources: RefCell::new(HashMap::new()),
            rounds: RefCell::new(Vec::new()),
            artifact: RefCell::new(None),
        }
    }
}

impl shellcore::Host for AppHost {
    fn now_ns(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    fn interrupted(&self) -> bool {
        false
    }

    fn heap_bytes(&self) -> u64 {
        0
    }

    fn metrics(&self) -> Result<kabi::Metrics, kabi::Error> {
        Ok(kabi::Metrics::empty())
    }

    fn set_policy(&self, _: u64) -> Result<(), kabi::Error> {
        Ok(())
    }

    fn ask(&self, _: &str) -> Result<String, String> {
        Err("not used by the gate".into())
    }

    fn script(&self, request: &ScriptRequest) -> Result<Answer, AnswerError> {
        let clock = Instant::now();
        let (answer, tokens) = (self.transport)(request);
        let error = answer.as_ref().err().map(AnswerError::describe);
        let context_full = error.as_deref().is_some_and(|e| e.contains("context"));
        self.rounds.borrow_mut().push(Round {
            ms: clock.elapsed().as_millis(),
            tokens,
            error,
            context_full,
        });
        answer
    }

    fn act(&self, _: &ActRequest) -> Result<Call, ActError> {
        self.call
            .borrow_mut()
            .take()
            .ok_or_else(|| ActError::Unavailable(String::from("one call a request")))
    }

    fn pkg(&self, request: &str) -> Result<String, String> {
        match request.starts_with("local-next ") {
            true => Ok(String::from("0.0.1")),
            false => Err(String::from("not used by the gate")),
        }
    }

    fn app_source(&self, name: &str, text: &str) -> Result<(), String> {
        self.sources
            .borrow_mut()
            .insert(name.to_string(), text.to_string());
        Ok(())
    }

    fn app_source_read(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.sources.borrow().get(name).cloned())
    }

    fn cc_build(&self, dir: &str, name: &str, version: &str) -> Result<CcBuilt, String> {
        let source = self.sources.borrow().get(name).cloned();
        let source = source.ok_or_else(|| format!("{dir}/{name}/main.c: no such file"))?;
        let (kind, built) = app_run::build_main(name, version, &source).map_err(|why| {
            why.text()
                .unwrap_or_else(|_| String::from("cc was stopped"))
        })?;
        let bytes = built.artifact.len();
        *self.artifact.borrow_mut() = Some(built.artifact);
        Ok(CcBuilt {
            app: matches!(
                kind,
                ccserve::build::Kind::App | ccserve::build::Kind::Window
            ),
            bytes,
            provenance: format!("built-by cc gate source {}", built.source),
            path: format!("{dir}/{name}.kai"),
        })
    }

    fn audit(&self, _: &str) {}
}

/// What became of one request.
#[derive(Debug)]
pub struct Outcome {
    pub built: bool,
    pub opened: bool,
    /// Why it did not pass: `None` when it built, opened and took every click.
    pub failure: Option<String>,
    pub rounds: Vec<Round>,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.failure.is_none()
    }
}

/// Asks for app `name` as a person would, then opens and clicks it.
pub fn attempt(name: &str, words: &str, transport: Transport) -> Outcome {
    let host = Rc::new(AppHost::new(name, words, transport));
    let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
    let reply = shell.eval(&format!("make me an app: {words}"));
    let rounds = host.rounds.borrow().clone();
    let artifact = host.artifact.borrow_mut().take();
    let outcome = |built, opened, failure: Option<String>| Outcome {
        built,
        opened,
        failure,
        rounds: rounds.clone(),
    };
    let Some(artifact) = artifact.filter(|_| reply.ok) else {
        return outcome(false, false, Some(format!("not built: {}", reply.text)));
    };
    match app_run::start(&artifact) {
        Err(why) => outcome(true, false, Some(format!("does not open: {why}"))),
        Ok((mut app, window)) => outcome(true, true, clicked(&mut app, window).err()),
    }
}

/// The click test: every button pressed and every field typed into and
/// submitted, in the first frame's order, each answered without a fault.
fn clicked(app: &mut wasmhost::CoreInstance, window: u32) -> Result<(), String> {
    let nodes = app_run::last_frame(app).ok_or("it shows nothing")?;
    for node in nodes {
        let events = match node.role {
            Role::Button => vec![UiEvent::Pressed(node.id)],
            Role::TextInput => vec![
                UiEvent::Changed(node.id, Value::Text(String::from("12"))),
                UiEvent::Submitted(node.id),
            ],
            Role::Switch => vec![UiEvent::Changed(node.id, Value::On(true))],
            _ => continue,
        };
        app_run::in_call(app, |a| a.ui_events(window, &events))
            .map_err(|fault| format!("{:?} {} stopped it: {fault:?}", node.role, node.name))?;
    }
    Ok(())
}

/// App requests answered by `engine` as the llm service answers them, within
/// the request's time limit, with the tokens each answer took.
fn transport(engine: Shared) -> Transport {
    Box::new(move |request: &ScriptRequest| {
        let limit =
            Duration::from_millis(request.timeout_ms.min(llmproto::script::SERVICE_TIMEOUT_MS));
        let started = Instant::now();
        let mut tokens = 0u32;
        let answer = llmproto::local::app(&mut engine.borrow_mut(), request, |step| {
            if let Step::Token(_) = step {
                tokens += 1;
            }
            match started.elapsed() >= limit {
                true => Next::Stop,
                false => Next::Continue,
            }
        });
        (answer, tokens)
    })
}

fn record(name: &str, words: &str, outcome: &Outcome, ms: u128) -> Json {
    let rounds: Vec<Json> = outcome
        .rounds
        .iter()
        .map(|r| json!({"ms": r.ms, "tokens": r.tokens, "error": r.error, "context_full": r.context_full}))
        .collect();
    json!({
        "name": name,
        "request": words,
        "built": outcome.built,
        "opened": outcome.opened,
        "passed": outcome.passed(),
        "failure": outcome.failure,
        "ms": ms,
        "rounds": rounds,
    })
}

fn parse(args: &[String]) -> Result<(String, Option<String>), String> {
    match args {
        [flag, id] if flag == "--local" => Ok((id.clone(), None)),
        [flag, id, store_flag, store] if flag == "--local" && store_flag == "--store" => {
            Ok((id.clone(), Some(store.clone())))
        }
        _ => Err("usage: cargo xtask app-gate --local ID [--store DIR]".into()),
    }
}

pub fn main(args: &[String], target_dir: &Path) -> Result<(), String> {
    let (id, store) = parse(args)?;
    let (engine, line) = engine_local::load(&id, store.as_deref())?;
    println!(
        "xtask: app gate, {} requests against local: {line}",
        REQUESTS.len()
    );
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let mut lines = vec![json!({"model": id, "engine": line, "started_unix": started}).to_string()];
    let mut passed = 0;
    for (number, (name, words)) in REQUESTS.iter().enumerate() {
        let clock = Instant::now();
        let outcome = attempt(name, words, transport(engine.clone()));
        let ms = clock.elapsed().as_millis();
        passed += usize::from(outcome.passed());
        let tokens: Vec<u32> = outcome.rounds.iter().map(|r| r.tokens).collect();
        let full = outcome.rounds.iter().filter(|r| r.context_full).count();
        println!(
            "xtask: {:>2} {name}: {} in {} s, {} rounds, tokens {tokens:?}, context full {full}",
            number + 1,
            outcome.failure.as_deref().unwrap_or("passes"),
            ms / 1000,
            outcome.rounds.len(),
        );
        lines.push(record(name, words, &outcome, ms).to_string());
    }
    let dir = target_dir.join("appgate");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(format!("{started}.jsonl"));
    std::fs::write(&file, lines.join("\n") + "\n")
        .map_err(|e| format!("{}: {e}", file.display()))?;
    println!("xtask: runs written to {}", file.display());
    let percent = passed * 100 / REQUESTS.len();
    println!(
        "xtask: app gate: {passed}/{} build, open and take the clicks ({percent}%); gate {GATE}%",
        REQUESTS.len()
    );
    match percent >= GATE {
        true => Ok(()),
        false => Err(String::from("the app gate fails")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmproto::pack::capp1;

    fn canned(answers: Vec<Result<Answer, AnswerError>>) -> Transport {
        let answers = RefCell::new(answers.into_iter());
        Box::new(move |_: &ScriptRequest| {
            let next = answers.borrow_mut().next();
            (next.unwrap_or(Err(AnswerError::CutOff)), 7)
        })
    }

    fn app(script: &str) -> Result<Answer, AnswerError> {
        Ok(Answer {
            summary: String::from("An app."),
            script: String::from(script),
        })
    }

    #[test]
    fn the_cards_example_builds_opens_and_takes_the_clicks() {
        let (_, main, _) = capp1::EXAMPLES[0];
        let outcome = attempt("shop", "a shopping list", canned(vec![app(main)]));
        assert!(outcome.passed(), "{outcome:?}");
        assert_eq!(outcome.rounds.len(), 1);
        assert_eq!(outcome.rounds[0].tokens, 7);
    }

    #[test]
    fn a_program_that_does_not_compile_is_repaired_with_the_reason() {
        let broken = capp1::TEMPLATE.replace("s->count++;", "s->count++");
        let outcome = attempt(
            "counter",
            "a counter",
            canned(vec![app(&broken), app(capp1::TEMPLATE)]),
        );
        assert!(outcome.passed(), "{outcome:?}");
        assert_eq!(outcome.rounds.len(), 2);
    }

    #[test]
    fn an_app_that_traps_on_a_click_fails_the_click_test() {
        let trap = capp1::TEMPLATE.replace("s->count++;", "s->count = 1 / (s->count - s->count);");
        let outcome = attempt("counter", "a counter", canned(vec![app(&trap)]));
        assert!(outcome.built && outcome.opened, "{outcome:?}");
        assert!(outcome.failure.unwrap().contains("stopped it"));
    }

    #[test]
    fn a_full_context_is_recorded_and_fails_the_request() {
        let full = AnswerError::Local(String::from("the conversation filled the model's context"));
        let outcome = attempt("counter", "a counter", canned(vec![Err(full)]));
        assert!(!outcome.built);
        assert!(outcome.rounds.iter().any(|r| r.context_full), "{outcome:?}");
    }
}
