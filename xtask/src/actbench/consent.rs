//! What code, not the model, would do with a decoded call: the shipping
//! consent code, `agentabi::consent::decide` (ADR 0002 D2-D3, task 7), over the
//! mock registry's rows mapped to `agentabi::Action`s. So the gate counts false
//! runs and false asks as the OS would make them.
//!
//! - A row's call is one planned call with its arguments as text. A value the
//!   State line holds and the user's line does not came from data, so it is
//!   labelled `Content("state")` (ADR 0002 D6, ADR 0010 State).
//! - A script plans each change row its code calls, with no arguments known
//!   statically (today's `names_in`; B's analyser is task 11), and takes the
//!   strictest decision.

use super::registry::{Row, Ty, Undo};
use agentabi::consent::{self, Call as Planned, Label, Value as Consented, Why};
use agentabi::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// No effect: `say` or `clarify`.
    Talk,
    Run,
    /// The vague path: it runs, saying what it took the request as.
    TakenAs,
    Ask,
    Never,
}

impl Decision {
    pub fn runs(self) -> bool {
        matches!(self, Self::Run | Self::TakenAs)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Talk => "talk",
            Self::Run => "run",
            Self::TakenAs => "taken-as",
            Self::Ask => "ask",
            Self::Never => "never",
        }
    }

    fn of(decision: consent::Decision) -> Self {
        match (decision.tier, decision.why) {
            (Tier::Run, Why::TakenAs) => Self::TakenAs,
            (Tier::Run, _) => Self::Run,
            (Tier::Ask, _) => Self::Ask,
            (Tier::Never, _) => Self::Never,
        }
    }
}

fn leak<T>(value: T) -> &'static T {
    Box::leak(Box::new(value))
}

fn arg_type(ty: &Ty) -> ArgType {
    match ty {
        Ty::Enum(words) => ArgType::Enum(words),
        Ty::Int { min, max } => ArgType::Int {
            min: *min,
            max: *max,
        },
        Ty::Text(max) => ArgType::Text { max: *max },
        Ty::Host => ArgType::Host,
        Ty::Path => ArgType::Path,
        Ty::Code => ArgType::Text {
            max: llmproto::script::SCRIPT_MAX_BYTES as u16,
        },
        Ty::List(inner, n) => ArgType::List(leak(arg_type(inner)), *n),
        Ty::Opt(inner) => ArgType::Opt(leak(arg_type(inner))),
    }
}

/// A mock row as the registry would declare it.
fn action(row: &'static Row) -> Action {
    let facts = Facts {
        open_world: row.open_world,
        ..match row.undo {
            Undo::Read => Facts::LOCAL_READ,
            Undo::Restore => Facts::LOCAL_CHANGE,
            Undo::None => Facts {
                read_only: false,
                destructive: true,
                idempotent: false,
                open_world: false,
            },
        }
    };
    let inverse = match row.undo {
        Undo::Read => Inverse::Read,
        Undo::Restore => Inverse::Restore,
        Undo::None => Inverse::None("it cannot be undone"),
    };
    let args: Vec<Arg> = row
        .args
        .iter()
        .map(|a| Arg {
            name: a.name,
            ty: arg_type(&a.ty),
            role: if a.from_request {
                Role::Target
            } else {
                Role::Option
            },
            from_request: a.from_request,
            never: &[],
            aliases: &[],
        })
        .collect();
    Action {
        name: row.name,
        doc: row.doc,
        args: args.leak(),
        facts,
        inverse,
        cues: row.cues,
        returns: Returns::System,
        ret: RetType::Unit,
        owner: Owner::Kernel,
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

/// The registry's rows (not the pseudo-actions) as `agentabi` actions.
pub fn actions(rows: &[&'static Row]) -> Vec<Action> {
    rows.iter()
        .filter(|r| !matches!(r.name, "say" | "clarify" | "script"))
        .map(|r| action(r))
        .collect()
}

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Whether `haystack` holds `value` as words, case and hyphens folded.
fn holds(haystack: &str, value: &str) -> bool {
    let fold = |s: &str| s.to_lowercase().replace(['-', '_'], " ");
    let (haystack, value) = (fold(haystack), fold(value));
    !value.trim().is_empty() && haystack.contains(value.trim())
}

/// The decision for a decoded call `{"name":..,"arguments":{..}}`.
pub fn decide(rows: &[&'static Row], call: &Value, line: &str, state: &str) -> Decision {
    let name = call["name"].as_str().unwrap_or("");
    let args = &call["arguments"];
    let actions = actions(rows);
    match name {
        "say" | "clarify" => Decision::Talk,
        "script" => script(&actions, args["code"].as_str().unwrap_or(""), line),
        _ => {
            let values: Vec<(String, String)> = args
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, v)| (k.clone(), text(v)))
                        .collect()
                })
                .unwrap_or_default();
            let consented: Vec<(&str, Consented)> = values
                .iter()
                .map(|(k, v)| {
                    let from_state = holds(state, v) && !holds(line, v);
                    let label = match from_state {
                        true => Label::Content("state"),
                        false => Label::Literal,
                    };
                    (k.as_str(), Consented { text: v, label })
                })
                .collect();
            let planned = [Planned {
                name,
                args: &consented,
                control: Label::Literal,
            }];
            Decision::of(consent::decide(line, &planned, &actions)[0])
        }
    }
}

/// A script's strictest decision over the change rows its code calls.
fn script(actions: &[Action], code: &str, line: &str) -> Decision {
    let names: Vec<&str> = actions
        .iter()
        .filter(|a| !a.facts.read_only && called(code, a.name))
        .map(|a| a.name)
        .collect();
    if names.is_empty() {
        return Decision::Run;
    }
    let planned: Vec<Planned> = names
        .iter()
        .map(|name| Planned {
            name,
            args: &[],
            control: Label::Literal,
        })
        .collect();
    consent::decide(line, &planned, actions)
        .into_iter()
        .map(Decision::of)
        .max_by_key(|d| match d {
            Decision::Talk | Decision::Run => 0,
            Decision::TakenAs => 1,
            Decision::Ask => 2,
            Decision::Never => 3,
        })
        .unwrap_or(Decision::Run)
}

/// Whether `code` calls `name` (`name(`, not part of a longer name).
pub fn called(code: &str, name: &str) -> bool {
    code.match_indices(&format!("{name}(")).any(|(at, _)| {
        code[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
    })
}

#[cfg(test)]
mod tests {
    use super::super::registry::rows;
    use super::*;
    use serde_json::json;

    fn decided(call: Value, line: &str) -> Decision {
        decide(&rows(40), &call, line, "")
    }

    #[test]
    fn the_mock_rows_pass_the_registrys_own_check() {
        assert_eq!(agentabi::check(&actions(&rows(80))), Ok(()));
    }

    #[test]
    fn plain_undoable_calls_run_and_no_undo_ones_ask() {
        let policy = json!({"name":"set_policy","arguments":{"name":"round-robin"}});
        assert_eq!(decided(policy, "switch to round-robin"), Decision::Run);
        let restart = json!({"name":"service_restart","arguments":{"name":"netd"}});
        assert_eq!(decided(restart.clone(), "restart netd"), Decision::Ask);
        assert_eq!(decided(restart, "restart the network"), Decision::Ask);
        assert_eq!(
            decided(json!({"name":"cpus","arguments":{}}), "cpus?"),
            Decision::Run
        );
        assert_eq!(
            decided(json!({"name":"rm_rf","arguments":{}}), "x"),
            Decision::Never
        );
    }

    #[test]
    fn negation_and_state_values_never_run_a_change() {
        let policy = json!({"name":"set_policy","arguments":{"name":"round-robin"}});
        assert_eq!(
            decided(policy.clone(), "don't switch to round robin"),
            Decision::Ask
        );
        let steered = decide(&rows(40), &policy, "what is here?", "here=round-robin");
        assert!(!steered.runs(), "{steered:?}");
    }

    #[test]
    fn scripts_take_their_strictest_call() {
        let code = |c: &str| json!({"name":"script","arguments":{"code":c}});
        assert_eq!(
            decided(code("print(cpus());"), "how many cpus"),
            Decision::Run
        );
        assert_eq!(
            decided(code("power_off();"), "power off now"),
            Decision::Ask
        );
        assert!(called("x = cpus();", "cpus"));
        assert!(!called("x = ncpus();", "cpus"));
    }
}
