//! Spike S12-2 (ADR 0012): the 50 fluency tasks written as Kai C. The model
//! gets the `c-1` card (`llmproto::program`) and answers one `c>` entry in the
//! script envelope. The entry goes to the shell at `c>`, which hands it to the
//! real `cc` service (`ccserve`, cc.wasm on the host) with the OS's rows as
//! `kai` imports, so each call goes through the one door to the stand-in OS
//! ([`GateHost`]). A program that does not compile goes back once with wcc's
//! message, as a refused script does; a program that compiled and failed is
//! not repaired, as a script's run error is not. The tasks, their checks and
//! the judge are the Rhai gate's ([`super::tasks::TASKS`], [`judge`]); only the reference
//! answers are C ([`REFERENCES`]), and the unit tests run them through the
//! same path.

use super::host::{GateHost, Transport};
use super::judge::{judge, no_proposal};
use super::tasks::Task;
use super::{Failure, Outcome};
use ccserve::rows::Rows;
use ccserve::{Service, XCC_HEADERS};
use llmproto::script::{AnswerError, SERVICE_TIMEOUT_MS, ScriptRequest};
use promptcore::wire::{Record, parse_reply};
use shellcore::{DEFAULT_CONFIG, Shell};
use std::rc::Rc;

/// The C answer to each of [`super::tasks::TASKS`], in order.
pub const REFERENCES: [&str; 50] = [
    "I64 n;\ncpus(&n);\nPrint(\"%lld\\n\", n);",
    "struct kai_memory m;\nmemory(&m);\nPrint(\"%lld\\n\", m.free_mib);",
    "struct kai_memory m;\nmemory(&m);\nPrint(\"%lld\\n\", m.total_mib);",
    "I64 ms;\nuptime_ms(&ms);\nPrint(\"%lld\\n\", ms);",
    "char p[32];\nget_policy(p, 32);\nPrint(\"%s\\n\", p);",
    "struct kai_threads t;\nthreads(&t);\nPrint(\"%lld\\n\", t.switches);",
    "struct kai_processes_item p[16];\nPrint(\"%lld\\n\", processes(p, 16));",
    "struct kai_memory m;\nmemory(&m);\nPrint(\"%lld\\n\", m.heap_kib);",
    "struct kai_threads t;\nthreads(&t);\nPrint(\"%lld\\n\", t.migrations);",
    "struct kai_threads t;\nthreads(&t);\nPrint(\"%lld\\n\", t.ejections);",
    // Reads combined with arithmetic and formatting.
    "struct kai_memory m;\nmemory(&m);\nPrint(\"%lld/%lld MiB\\n\", m.free_mib, m.total_mib);",
    "struct kai_memory m;\nmemory(&m);\nPrint(\"%lld\\n\", (m.total_mib - m.free_mib) * 100 / m.total_mib);",
    "struct kai_memory m;\nmemory(&m);\nPrint(\"%lld\\n\", m.total_mib - m.free_mib);",
    "I64 ms;\nuptime_ms(&ms);\nPrint(\"%lld\\n\", ms / 1000);",
    "I64 n;\ncpus(&n);\nchar p[32];\nget_policy(p, 32);\nPrint(\"%lld CPUs, policy %s\\n\", n, p);",
    "struct kai_memory m;\nmemory(&m);\nI64 n;\ncpus(&n);\nPrint(\"%lld\\n\", m.free_mib / n);",
    "struct kai_processes_item p[16];\nprocesses(p, 16);\nPrint(\"%s\\n\", p[0].name);",
    "struct kai_processes_item p[16];\nprocesses(p, 16);\nPrint(\"%lld %lld\\n\", p[0].pid, p[1].pid);",
    "I64 ms;\nuptime_ms(&ms);\nPrint(\"%lld.%03lld\\n\", ms / 1000, ms % 1000);",
    "struct kai_threads t;\nthreads(&t);\nI64 n;\ncpus(&n);\nPrint(\"%lld\\n\", t.migrations / n);",
    // Working through lists.
    "struct kai_processes_item p[16];\nI64 n = processes(p, 16);\nfor (I64 i = 0; i < n; i++) Print(\"%s\\n\", p[i].name);",
    "#include <string.h>\nstruct kai_processes_item p[16];\nprocesses(p, 16);\nPrint(\"%lld\\n\", (I64)strlen(p[0].name));",
    "struct kai_processes_item p[16];\nI64 n = processes(p, 16);\nPrint(\"%s\\n\", p[n - 1].name);",
    "#include <string.h>\nstruct kai_processes_item p[16];\nprocesses(p, 16);\nPrint(\"%s\\n\", strcmp(p[1].name, \"llm\") == 0 ? \"yes\" : \"no\");",
    "struct kai_processes_item p[16];\nprocesses(p, 16);\nPrint(\"%lld\\n\", p[0].pid + p[1].pid + p[2].pid);",
    "struct kai_processes_item p[16];\nprocesses(p, 16);\nI64 best = 0;\nfor (I64 i = 1; i < 3; i++) if (p[i].pid > p[best].pid) best = i;\nPrint(\"%s\\n\", p[best].name);",
    "struct kai_processes_item p[16];\nprocesses(p, 16);\nfor (I64 i = 0; p[1].name[i]; i++) if (p[1].name[i] >= 'a' && p[1].name[i] <= 'z') p[1].name[i] -= 32;\nPrint(\"%s\\n\", p[1].name);",
    "struct kai_processes_item p[16];\nI64 n;\ncpus(&n);\nPrint(\"%lld:%lld\\n\", processes(p, 16), n);",
    "#include <string.h>\nstruct kai_processes_item p[16];\nprocesses(p, 16);\nPrint(\"%lld\\n\", (I64)(strlen(p[0].name) + strlen(p[1].name) + strlen(p[2].name)));",
    "struct kai_processes_item p[16];\nprocesses(p, 16);\nPrint(\"%s\\n\", p[0].pid < 15 ? \"true\" : \"false\");",
    // Strings, arrays and maps, as C writes them.
    "I64 n;\ncpus(&n);\nstruct kai_memory m;\nmemory(&m);\nPrint(\"{cpus: %lld, free_mib: %lld}\\n\", n, m.free_mib);",
    "struct kai_memory m;\nmemory(&m);\nPrint(\"free memory: %lld MiB\\n\", m.free_mib);",
    "char p[32];\nget_policy(p, 32);\nfor (I64 i = 0; p[i]; i++) if (p[i] >= 'a' && p[i] <= 'z') p[i] -= 32;\nPrint(\"%s\\n\", p);",
    "I64 ms;\nuptime_ms(&ms);\nI64 s = ms / 1000;\nPrint(\"%lldm%llds\\n\", s / 60, s % 60);",
    "struct kai_threads t;\nthreads(&t);\nPrint(\"%lld %lld %lld\\n\", t.switches, t.ejections, t.migrations);",
    "#include <string.h>\nchar p[32];\nget_policy(p, 32);\nPrint(\"%s\\n\", strcmp(p, \"round-robin\") == 0 ? \"true\" : \"false\");",
    "struct kai_processes_item p[16];\nI64 n;\ncpus(&n);\nPrint(\"[%lld, %lld]\\n\", n, processes(p, 16));",
    "struct kai_memory m;\nmemory(&m);\nPrint(\"{used_mib: %lld, free_mib: %lld}\\n\", m.total_mib - m.free_mib, m.free_mib);",
    "#include <string.h>\nchar p[32];\nget_policy(p, 32);\nPrint(\"%lld\\n\", (I64)strlen(p));",
    "struct kai_threads t;\nthreads(&t);\nI64 d = 1;\nfor (I64 v = t.switches; v >= 10; v /= 10) d++;\nPrint(\"%lld\\n\", d);",
    // Conditions and changes.
    "set_policy(\"round-robin\");",
    "I64 n;\ncpus(&n);\nif (n < 8) set_policy(\"round-robin\");",
    "#include <string.h>\nchar p[32];\nget_policy(p, 32);\nif (strcmp(p, \"interactive\") != 0) set_policy(\"interactive\");\nget_policy(p, 32);\nPrint(\"%s\\n\", p);",
    "struct kai_memory m;\nmemory(&m);\nif (m.free_mib > 100) set_policy(\"round-robin\");\nchar p[32];\nget_policy(p, 32);\nPrint(\"policy: %s\\n\", p);",
    "char p[32];\nget_policy(p, 32);\nPrint(\"%s\\n\", p);\nset_policy(\"round-robin\");\nget_policy(p, 32);\nPrint(\"%s\\n\", p);",
    // Things the shell cannot do.
    "Print(\"I cannot format disks from a program.\\n\");",
    "Print(\"I cannot open websites from a program.\\n\");",
    "Print(\"I cannot reboot the computer from a program.\\n\");",
    "Print(\"I cannot restart services from a program.\\n\");",
    "Print(\"I cannot read email: there is no access to it.\\n\");",
];

/// cc's clock and Ctrl-C on the host.
struct CcClock;

impl ccserve::Host for CcClock {
    fn now_ns(&self) -> u64 {
        let start = *START.get_or_init(std::time::Instant::now);
        u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    fn interrupted(&self) -> bool {
        false
    }
}

static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// A stand-in OS with its own `cc` service, and a shell at `c>` on it.
fn c_shell() -> Result<(Rc<GateHost>, Shell), String> {
    let host = Rc::new(GateHost::new(Box::new(|_: &ScriptRequest| {
        Err(AnswerError::Unavailable(
            "program requests go to the C card".into(),
        ))
    })));
    let service = Service::new(ccserve::CC_WASM, &XCC_HEADERS, Rows::os(), &CcClock)
        .map_err(|e| format!("cc does not start: {e:?}"))?;
    host.start_cc(service, Box::new(CcClock));
    let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
    let entered = shell.eval(":c");
    if !entered.ok {
        return Err(format!("no c> prompt: {}", entered.text));
    }
    Ok((host, shell))
}

/// What one entry did.
struct Ran {
    ok: bool,
    output: String,
    /// It stopped before running: wcc refused it.
    refused: bool,
}

/// Runs `program` at `c>`, saying yes to a call that asks.
fn run(host: &GateHost, shell: &mut Shell, program: &str) -> Ran {
    let first = host.cc_replies.borrow().len();
    let mut reply = shell.eval(program);
    if let Some(id) = reply.confirm {
        reply = shell.eval(&format!("answer {id} yes"));
    }
    // wcc refused it: cc's replies to the entry hold an `err`, and nothing ran
    // (no output, no call, no trap).
    let replies = host.cc_replies.borrow();
    let records: Vec<Record> = replies[first..]
        .iter()
        .filter_map(|bytes| parse_reply(bytes).ok())
        .flatten()
        .collect();
    let refused = !reply.ok
        && records.iter().any(|r| matches!(r, Record::Err { .. }))
        && !records.iter().any(|r| {
            matches!(
                r,
                Record::Out(_) | Record::Need { .. } | Record::Trap { .. }
            )
        });
    Ran {
        ok: reply.ok,
        output: without_value(&reply.text),
        refused,
    }
}

/// The output without the shell's `= value` line for the entry's last
/// expression (a last `Print` shows its count), which is not the program's
/// own output.
fn without_value(text: &str) -> String {
    match text.rsplit_once('\n') {
        Some((head, last)) if last.starts_with("= ") => head.to_string(),
        None if text.starts_with("= ") => String::new(),
        _ => text.to_string(),
    }
}

/// One task through the C path: the program request, at most one repair for
/// a program wcc refused, and the judge.
pub fn attempt(task: &Task, transport: &Transport) -> Outcome {
    let (host, mut shell) = match c_shell() {
        Ok(started) => started,
        Err(why) => {
            return Outcome {
                rounds: 0,
                script: None,
                output: why.clone(),
                failure: Some((Failure::Server, why)),
            };
        }
    };
    let mut repair = None;
    for round in 1..=2 {
        let request = ScriptRequest {
            request: task.request.to_string(),
            repair: repair.take(),
            timeout_ms: SERVICE_TIMEOUT_MS,
        };
        let answer = match transport(&request) {
            Ok(answer) => answer,
            Err(error) if error.repairable() && round == 1 => {
                repair = Some((String::new(), error.describe()));
                continue;
            }
            Err(error) => {
                let text = error.describe();
                return Outcome {
                    rounds: round,
                    script: None,
                    failure: no_proposal(task, Some(&Err(error)), &text),
                    output: text,
                };
            }
        };
        let ran = run(&host, &mut shell, &answer.script);
        if ran.refused && round == 1 {
            repair = Some((answer.script, ran.output));
            continue;
        }
        let failure = match ran.refused {
            true => Some((Failure::Refused, ran.output.clone())),
            false => judge(task, ran.ok, &ran.output, &host.changes.borrow()),
        };
        return Outcome {
            rounds: round,
            script: Some(answer.script),
            output: ran.output,
            failure,
        };
    }
    unreachable!("the second round returns")
}

/// Every task's reference program through [`attempt`], answered as given.
#[cfg(test)]
fn references() -> Vec<(usize, Outcome)> {
    super::tasks::TASKS
        .iter()
        .zip(REFERENCES)
        .enumerate()
        .map(|(n, (task, program))| {
            let answer = llmproto::script::Answer {
                summary: String::new(),
                script: program.to_string(),
            };
            let transport: Transport = Box::new(move |_| Ok(answer.clone()));
            (n, attempt(task, &transport))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reference_program_passes_its_task() {
        let failed: Vec<String> = references()
            .into_iter()
            .filter_map(|(n, outcome)| {
                outcome
                    .failure
                    .map(|(kind, detail)| format!("{}: {} {detail}", n + 1, kind.name()))
            })
            .collect();
        assert!(failed.is_empty(), "{failed:#?}");
    }

    #[test]
    fn a_program_wcc_refuses_goes_back_once_with_its_message() {
        let asked = std::cell::RefCell::new(Vec::new());
        let asked = Rc::new(asked);
        let seen = asked.clone();
        let transport: Transport = Box::new(move |request: &ScriptRequest| {
            seen.borrow_mut().push(request.repair.clone());
            let script = match request.repair {
                None => "I64 n;\ncpus(n, n, n);\nPrint(\"%lld\\n\", n);",
                Some(_) => REFERENCES[0],
            };
            Ok(llmproto::script::Answer {
                summary: String::new(),
                script: script.into(),
            })
        });
        let outcome = attempt(&super::super::tasks::TASKS[0], &transport);
        assert!(outcome.failure.is_none(), "{outcome:?}");
        assert_eq!(outcome.rounds, 2);
        let asked = asked.borrow();
        let (refused, why) = asked[1].as_ref().expect("a repair");
        assert!(refused.contains("cpus(n, n, n)"));
        assert!(why.contains("line 2"), "{why}");
    }

    #[test]
    fn a_wrong_value_is_wrong_output_and_not_repaired() {
        let transport: Transport = Box::new(|_| {
            Ok(llmproto::script::Answer {
                summary: String::new(),
                script: "Print(\"8\\n\");".into(),
            })
        });
        let outcome = attempt(&super::super::tasks::TASKS[0], &transport);
        assert_eq!(outcome.rounds, 1);
        assert!(matches!(outcome.failure, Some((Failure::WrongOutput, _))));
    }

    #[test]
    fn a_print_last_does_not_show_its_count_as_output() {
        assert_eq!(without_value("ab\n= 3"), "ab");
        assert_eq!(without_value("= 3"), "");
        assert_eq!(without_value("7"), "7");
    }

    /// The card's prototypes are kai.h's, so what the model reads compiles.
    #[test]
    fn the_c1_cards_prototypes_are_kai_hs() {
        let kai_h = ccserve::service::kai_h(&Rows::os());
        for row in agentabi::SCRIPT_API.iter() {
            for line in llmproto::pack::c1::declare(row).lines() {
                let decl = match line.split_once(" // ") {
                    Some((decl, _)) => decl,
                    None => line,
                };
                let imported = format!("__KAI_IMPORT(\"{}\") {decl}", row.name);
                assert!(
                    kai_h.contains(decl)
                        && (decl.starts_with("struct") || kai_h.contains(&imported)),
                    "{decl}"
                );
            }
        }
    }
}
