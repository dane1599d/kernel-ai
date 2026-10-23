//! Part of spike S12-3, Rhai Grain's gate (ADR 0012 D9): every reference script,
//! checked and run as the shell runs the model's scripts, gives the same output and
//! the same policy changes on Grain's VM as on the AST interpreter.

use super::host::GateHost;
use super::tasks::TASKS;
use llmproto::script::{Answer, AnswerError};
use shellcore::{Config, DEFAULT_CONFIG, Exec, Host, script};
use std::rc::Rc;

/// What one reference script did: whether it was accepted, then its output and
/// whether it ran without an error, and the policy ids it set.
fn outcome(reference: &str, exec: Exec) -> (Result<(bool, String), String>, Vec<u64>) {
    let host = Rc::new(GateHost::new(Box::new(|_| {
        Err(AnswerError::Server("no model here".into()))
    })));
    let shared: Rc<dyn Host> = host.clone();
    let config = Config {
        exec,
        ..DEFAULT_CONFIG
    };
    let answer = Answer {
        script: reference.to_string(),
        summary: "Does what was asked.".to_string(),
    };
    let result = script::check(&answer, &shared, &config)
        .map(|checked| {
            let ran = script::run(&checked, &shared, &config);
            (ran.ok, ran.text)
        })
        .map_err(|refusal| refusal.to_string());
    let changes = host.changes.borrow().clone();
    (result, changes)
}

#[test]
fn grain_runs_every_reference_script_as_the_ast_does() {
    for (number, task) in TASKS.iter().enumerate() {
        assert_eq!(
            outcome(task.reference, Exec::Grain),
            outcome(task.reference, Exec::Ast),
            "task {} {:?}",
            number + 1,
            task.request
        );
    }
}
