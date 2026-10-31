//! `cargo xtask fluency --local ID`: the gate run through Kernel AI's own engine
//! on this PC, on a catalog model from the store. Each script request goes the
//! way the llm service takes it in the OS: through the wire format both ways (so
//! it is clipped the same), into `llmproto::local::script` on an engine of the same
//! context cap, memory budget and tier choice, within the same time limit (but
//! without the 1 ms pause the service takes after every step, so a request near
//! the limit passes here sooner than in the OS). One engine serves every task, so
//! the system prompt is computed once, before the first task and outside its
//! time limit as the service computes it after loading, and reused from the
//! cache, as in the OS.
//! The draws are seeded, so a second run gives the same answers: `--repeat` is
//! for servers.

use super::host::Transport;
use infer::Next;
use infer::chat::Engine;
use llmproto::script::{self, ScriptRequest};
use quant::Tier;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub type Shared = Rc<RefCell<Engine<'static>>>;

/// The engine for catalog model `id` in the store (`store`, else `KAI_MODELS`,
/// else the default), and a line saying what it runs.
pub fn load(id: &str, store: Option<&str>) -> Result<(Shared, String), String> {
    let path = crate::models::stored(id, store)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    // The engine borrows the file for as long as the gate runs.
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let started = Instant::now();
    let engine = crate::local_script::engine(bytes, Tier::best())
        .map_err(|e| format!("{} cannot run: {e}", path.display()))?;
    let about = engine.about();
    let line = format!(
        "{} ({} tier, context {}), loaded and warmed in {} ms from {}",
        about.name.as_deref().unwrap_or(id),
        about.tier.name(),
        about.context,
        started.elapsed().as_millis(),
        path.display()
    );
    Ok((Rc::new(RefCell::new(engine)), line))
}

/// Script requests answered by `engine`, as the llm service answers them, within
/// the same time limit.
pub fn transport(engine: Shared) -> Transport {
    Box::new(move |request: &ScriptRequest| {
        let limit = Duration::from_millis(request.timeout_ms.min(script::SERVICE_TIMEOUT_MS));
        let started = Instant::now();
        crate::local_script::served(&mut engine.borrow_mut(), request, |_| {
            match started.elapsed() >= limit {
                true => Next::Stop,
                false => Next::Continue,
            }
        })
    })
}

/// Program requests (spike S12-2) answered by `engine` with the `c-1` card
/// (`llmproto::local::program`), within the same time limit. There is no
/// wire for them yet, so they go to the engine directly.
pub fn program_transport(engine: Shared) -> Transport {
    Box::new(move |request: &ScriptRequest| {
        let limit = Duration::from_millis(request.timeout_ms.min(script::SERVICE_TIMEOUT_MS));
        let started = Instant::now();
        llmproto::local::program(&mut engine.borrow_mut(), request, |_| {
            match started.elapsed() >= limit {
                true => Next::Stop,
                false => Next::Continue,
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::super::tasks::TASKS;
    use super::super::{Failure, Summary, attempt};
    use super::*;

    /// The gate on the local-script model: it runs every task through the local
    /// path end to end, and a model of random weights passes none of them.
    #[test]
    fn the_gate_runs_on_the_local_engine() {
        let bytes = infer::fixture::local_script(&crate::local_script::prompts());
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let engine = Rc::new(RefCell::new(
            crate::local_script::engine(bytes, Tier::best()).unwrap(),
        ));
        let outcomes: Vec<_> = TASKS
            .iter()
            .map(|task| attempt(task, transport(engine.clone())))
            .collect();
        let summary = Summary::of(&outcomes);
        let failures = |kind| {
            outcomes
                .iter()
                .filter(|o| o.failure.as_ref().is_some_and(|(k, _)| *k == kind))
                .count()
        };
        println!(
            "fixture: {}/{} first try, {}/{} after one repair; refused {}, run errors {}, no script {}",
            summary.first_try,
            summary.runs,
            summary.passed,
            summary.runs,
            failures(Failure::Refused),
            failures(Failure::RunError),
            failures(Failure::NoScript),
        );
        assert_eq!(summary.runs, TASKS.len());
        // Every answer came from the engine: none failed as a server would.
        assert_eq!(failures(Failure::Server), 0);
        assert!(outcomes.iter().all(|o| o.rounds >= 1));
        assert!(!summary.meets_gate());
    }
}
