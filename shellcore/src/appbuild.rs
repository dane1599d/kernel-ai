//! `app_build(name, request)` ("Kai builds apps", piece 3): the act model calls
//! it for an app request, and the shell:
//!
//! 1. asks the model for the app's `main.c` with the `c-app-1` card (a script
//!    request whose `lang` is [`Lang::App`]: the app's name, then the request);
//! 2. checks it with `llmproto::precheck`, as a `c>` program is;
//! 3. keeps it as `/home/user/apps/<name>/main.c` ([`crate::Host::app_source`]),
//!    the folder "Show code" opens and removal leaves;
//! 4. builds it with `cc build` from there, at the version `pkg`'s `local-next`
//!    proposes ([`crate::Host::cc_build`]);
//! 5. tries the build once on stub imports with `cc`'s `trial`
//!    ([`crate::Host::cc_trial`]);
//! 6. repairs a refused, failed or failing one at most [`MAX_REPAIRS`] times,
//!    each with exactly why: the check's problems, `cc`'s diagnostics or the
//!    trial's. A build or trial that failed through no fault of the program's
//!    ([`INTERNAL`]) is reported, and no repair is spent on it.
//!
//! Nothing is installed: the install card asks for that (piece 4), so the
//! build runs. The log records the request, the source's SHA-256, `cc`'s
//! `built-by cc` line and the trial's report, which `:why` shows.

use crate::actions::{How, Pending};
use crate::actlog::{Change, Entry, Form, State};
use crate::ccbuild::{APPS_DIR, CcBuilt, shown, valid_name, valid_version};
use crate::cmode::program::MAX_REPAIRS;
use crate::script::Checked;
use crate::tiers::Tier;
use crate::{Reply, SCRIPT_ROUND_MS, Shell};
use agentabi::consent::{Decision, Why};
use alloc::format;
use alloc::string::{String, ToString};
use llmproto::script::{Answer, AnswerError, Lang, ScriptRequest};
use sha2::{Digest, Sha256};

/// The terminal's wait for one line's answer: every round of `app_build` must
/// fit in it whole.
pub const LINE_WAIT_MS: u64 = 310_000;
/// The longest the host waits for `cc build` (`shell/build.rs`), which a round
/// reserves beside the model's.
pub const CC_BUILD_WAIT_MS: u64 = 150_000;
/// The longest the host waits for `cc`'s `trial`: its four steps and the
/// app's load, each within a line budget (12 s when QEMU emulates the CPU),
/// with room to spare.
pub const CC_TRIAL_WAIT_MS: u64 = 90_000;
/// How a trial's failure that is not the program's fault starts, as
/// `ccserve::trial::INTERNAL` says it.
pub const INTERNAL: &str = "internal: ";
/// What one round reserves: the model's answer, the build and the trial.
const ROUND_MS: u64 = SCRIPT_ROUND_MS + CC_BUILD_WAIT_MS + CC_TRIAL_WAIT_MS;
/// The longest request the card gets, as the row takes it.
const MAX_REQUEST: usize = 300;

/// An `app_build` waiting on its menu's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    name: String,
    wanted: String,
}

/// What `app_build` built and has not installed: what the install card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub name: String,
    pub version: String,
    /// The card's one line, from the model's answer.
    pub summary: String,
    /// The SHA-256 of `main.c`, in hex.
    pub source: String,
    pub built: CcBuilt,
    /// The trial's one-line report.
    pub trial: String,
}

/// Where an `app_build` is in its rounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rounds {
    pub(crate) name: String,
    pub(crate) wanted: String,
    /// 0 for the first answer, then one more for each repair.
    pub(crate) round: usize,
    /// The source the next round repairs, and why.
    repair: Option<(String, String)>,
}

impl Rounds {
    pub(crate) fn new(name: &str, wanted: &str) -> Self {
        Self {
            name: name.to_string(),
            wanted: wanted.to_string(),
            round: 0,
            repair: None,
        }
    }

    /// The model's request for this round, with what it repairs.
    pub(crate) fn ask(&mut self, timeout_ms: u64) -> ScriptRequest {
        ScriptRequest {
            request: format!("{}\n{}", self.name, self.wanted),
            repair: self.repair.take(),
            timeout_ms,
            lang: Lang::App,
        }
    }

    fn again(&mut self, source: String, why: String) -> Next {
        self.repair = Some((source, why));
        self.round += 1;
        Next::Ask
    }
}

/// What comes after a round's answer.
pub(crate) enum Next {
    /// Another round: the model's repair.
    Ask,
    Built(Built),
    Failed(String),
}

/// Why a round's build did not come to be.
enum Unbuilt {
    /// The program's fault, which a repair gets.
    Repair(String),
    /// No fault of the program's: reported, with no repair spent on it.
    Final(String),
}

/// The log's line for app `name`'s `main.c` at `sha`.
fn source_line(name: &str, sha: &str) -> String {
    format!("source {APPS_DIR}/{name}/main.c sha256 {sha}")
}

/// The SHA-256 of `text`, in hex.
pub(crate) fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Shell {
    /// An `app_build` the AI called, after consent.
    pub(crate) fn app_call(
        &mut self,
        id: u64,
        request: &str,
        values: &[(&'static str, String)],
        decision: Decision,
    ) -> Reply {
        let value = |name: &str| {
            values
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.trim())
                .unwrap_or_default()
        };
        let (name, wanted) = (value("name"), value("request"));
        if !valid_name(name) {
            return Reply::err(format!(
                "an app's name is 2-12 of a-z, 0-9 and -, starting with a letter, not {name:?}"
            ));
        }
        if wanted.is_empty() || wanted.len() > MAX_REQUEST || wanted.chars().any(char::is_control) {
            return Reply::err("app_build takes what the app should do, in one line");
        }
        let call = termcore::once::call_text(
            "app_build",
            &[
                termcore::once::CallArg::Text(name),
                termcore::once::CallArg::Text(wanted),
            ],
        )
        .unwrap_or_else(|| String::from("app_build"));
        // It runs when the line asked for it; a vague line's reading, a call
        // the model added, or one read content steered asks once (D6).
        if (decision.tier, decision.why) != (Tier::Run, Why::Asked) {
            self.host.audit(&format!("request {id}: {call} asks"));
            let checked = Checked::shown_only(&call);
            let reply = self.ask_call(id, request, checked, decision.why, None, None);
            if let Some(pending) = &mut self.pending {
                pending.app = Some(Asked {
                    name: name.to_string(),
                    wanted: wanted.to_string(),
                });
            }
            return reply;
        }
        let how = How {
            form: Form::Call,
            tier: Tier::Run,
            consent: String::from("asked"),
        };
        self.app_run(id, request, name, wanted, how)
    }

    /// The answer to an `app_build`'s menu: an Allow whose ONCE checks runs it.
    pub(crate) fn answer_app(&mut self, pending: Pending, text: &str) -> Reply {
        let Some(asked) = pending.app.clone() else {
            return Reply::err("nothing to confirm");
        };
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        let how = How {
            consent: String::from("yes"),
            ..pending.how.clone()
        };
        let reply = self.app_run(
            pending.id,
            &pending.request,
            &asked.name,
            &asked.wanted,
            how,
        );
        self.remember(&pending, &reply.text);
        reply
    }

    fn app_run(&mut self, id: u64, request: &str, name: &str, wanted: &str, how: How) -> Reply {
        if let Some(reply) = self.app_start(id, request, name, wanted, &how) {
            return reply;
        }
        match self.app_build(name, wanted) {
            Ok(built) => self.log_app_build(id, request, wanted, how, built),
            Err(why) => {
                self.log_app_failed(id, request, name, wanted, how, &why);
                Reply::err(why)
            }
        }
    }

    /// Refuses a `main.c` that `app_build` did not write: it is the person's,
    /// never overwritten, since nothing would bring it back.
    pub(crate) fn app_check_source(&self, name: &str) -> Result<(), String> {
        let now = self
            .host
            .app_source_read(name)
            .map_err(|why| format!("{APPS_DIR}/{name}/main.c: {}", shown(&why)))?;
        match now {
            Some(text) if !self.app_wrote(name, &sha256(&text)) => Err(format!(
                "{APPS_DIR}/{name}/main.c was changed since Kai wrote it, so it is left as it is: give the app another name"
            )),
            _ => Ok(()),
        }
    }

    /// The write, check, build and repair loop within one line: what was
    /// built, or why not. A host that builds in the background does not
    /// come here ([`Shell::app_start`]).
    fn app_build(&mut self, name: &str, wanted: &str) -> Result<Built, String> {
        self.app_check_source(name)?;
        let started = self.host.now_ns();
        let mut rounds = Rounds::new(name, wanted);
        loop {
            // A round is the model's answer, cc's build and its trial: each
            // starts only if all three fit in what is left of the terminal's
            // wait.
            let spent_ms = self.host.now_ns().saturating_sub(started) / 1_000_000;
            if rounds.round > 0 && spent_ms + ROUND_MS > LINE_WAIT_MS {
                let why = rounds.repair.map(|(_, why)| why).unwrap_or_default();
                return Err(format!(
                    "Kai's app {name} did not build in time, after {} tries:\n{why}",
                    rounds.round
                ));
            }
            let ask = rounds.ask(SCRIPT_ROUND_MS);
            let answer = self.host.script(&ask);
            match self.app_answer(&mut rounds, answer) {
                Next::Ask => {}
                Next::Built(built) => return Ok(built),
                Next::Failed(why) => return Err(why),
            }
        }
    }

    /// One round's answer: checked, kept, built and tried. What the next
    /// step is: another round with its repair, the build, or why there is none.
    pub(crate) fn app_answer(
        &mut self,
        rounds: &mut Rounds,
        answer: Result<Answer, AnswerError>,
    ) -> Next {
        let name = rounds.name.clone();
        let last = rounds.round >= MAX_REPAIRS;
        let answer = match answer {
            Ok(answer) => answer,
            Err(AnswerError::NoScript(why)) => {
                return Next::Failed(format!("Kai wrote no app: {}", shown(&why)));
            }
            Err(error) if error.repairable() && !last => {
                return rounds.again(String::new(), error.describe());
            }
            Err(error) => return Next::Failed(error.describe()),
        };
        if answer.script.trim().is_empty() {
            return Next::Failed(format!("Kai wrote no app: {}", shown(&answer.summary)));
        }
        let prepared = llmproto::precheck::prepare(&answer.script, &[]);
        let why = match prepared.problems.is_empty() {
            false => format!(
                "That app was refused before compiling:\n{}",
                prepared.problems.join("\n")
            ),
            true => match self.app_compile(&name, &prepared.source) {
                Ok((version, built, trial)) => {
                    return Next::Built(Built {
                        name,
                        version,
                        summary: answer.summary,
                        source: sha256(&prepared.source),
                        built,
                        trial,
                    });
                }
                Err(Unbuilt::Repair(why)) => why,
                Err(Unbuilt::Final(why)) => return Next::Failed(why),
            },
        };
        if last {
            return Next::Failed(format!(
                "Kai's app {name} did not build after {MAX_REPAIRS} repairs:\n{why}"
            ));
        }
        rounds.again(prepared.source, why)
    }

    /// Keeps `source` as the app's `main.c`, builds it and tries it: the
    /// version, what `cc` built and the trial's report, or why not.
    fn app_compile(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<(String, CcBuilt, String), Unbuilt> {
        let fail = |why: String| Unbuilt::Final(why);
        // Between rounds in the background the person may have edited it.
        self.app_check_source(name).map_err(fail)?;
        self.host
            .app_source(name, source)
            .map_err(|why| fail(format!("the app's source was not kept: {}", shown(&why))))?;
        self.app_written.retain(|(n, _)| n != name);
        self.app_written.push((name.to_string(), sha256(source)));
        let version = match self.host.pkg(&format!("local-next {name}")) {
            Ok(version) if valid_version(&version) => version,
            Ok(other) => {
                let why = format!("pkg's next version is not one: {}", shown(&other));
                return Err(fail(why));
            }
            Err(why) => return Err(fail(format!("pkg: {}", shown(&why)))),
        };
        let built = self
            .host
            .cc_build(APPS_DIR, name, &version)
            .map_err(|why| match why.strip_prefix(INTERNAL) {
                Some(why) => fail(format!(
                    "cc could not build Kai's app {name}: {}",
                    shown(why)
                )),
                None => Unbuilt::Repair(format!("cc build refused it:\n{why}")),
            })?;
        let trial = match self.host.cc_trial(name) {
            Ok(report) => report,
            Err(why) => {
                return Err(match why.strip_prefix(INTERNAL) {
                    Some(why) => fail(format!(
                        "Kai's app {name} built, but cc could not try it: {}",
                        shown(why)
                    )),
                    None => Unbuilt::Repair(format!("its trial failed:\n{why}")),
                });
            }
        };
        Ok((version, built, trial))
    }

    /// Whether `sha` is a `main.c` this shell wrote for app `name`: this
    /// session's last, or one an `app_build` entry of the log records.
    fn app_wrote(&self, name: &str, sha: &str) -> bool {
        let line = source_line(name, sha);
        let res = format!("{APPS_DIR}/{name}");
        // The entry must be app_build's own, for this app: its script's
        // source line, and its change.
        let logged = |e: &Entry| {
            e.script.starts_with("app_build(")
                && e.script.lines().any(|l| l == line)
                && e.changes
                    .iter()
                    .any(|c| c.action == "app_build" && c.res == res)
        };
        self.app_written.iter().any(|(n, s)| n == name && s == sha)
            || self.log.entries().rev().any(logged)
    }

    /// Logs a build: the request, the source's hash and `cc`'s provenance.
    pub(crate) fn log_app_build(
        &mut self,
        id: u64,
        request: &str,
        wanted: &str,
        how: How,
        built: Built,
    ) -> Reply {
        let Built {
            name,
            version,
            summary,
            source,
            built: cc,
            trial,
        } = &built;
        let script = format!(
            "app_build(\"{name}\", {wanted:?})\n{}\n{}\ntrial {}",
            source_line(name, source),
            cc.provenance,
            shown(trial)
        );
        let change = Change {
            action: String::from("app_build"),
            owner: String::from("cc"),
            res: format!("{APPS_DIR}/{name}"),
            before: String::new(),
            after: format!("{version} built, {} bytes, not installed", cc.bytes),
            inverse: None,
        };
        self.push_app_entry(
            id,
            request,
            how,
            script,
            String::from("ran"),
            State::Done,
            change,
        );
        self.host
            .audit(&format!("app_build {name} {version}: {}", cc.provenance));
        let reply = format!(
            "Built {name} {version} ({} bytes): {}. Its source is in {APPS_DIR}/{name}/.",
            cc.bytes,
            shown(summary).trim_end_matches('.')
        );
        self.app_built = Some(built);
        Reply::ok(reply)
    }

    /// Logs a build that did not come to be, with why.
    pub(crate) fn log_app_failed(
        &mut self,
        id: u64,
        request: &str,
        name: &str,
        wanted: &str,
        how: How,
        why: &str,
    ) {
        // The source it last wrote stays, and may be built again.
        let written = self.app_written.iter().find(|(n, _)| n == name);
        let script = match written {
            Some((_, sha)) => format!(
                "app_build(\"{name}\", {wanted:?})\n{}",
                source_line(name, sha)
            ),
            None => format!("app_build(\"{name}\", {wanted:?})"),
        };
        let change = Change {
            action: String::from("app_build"),
            owner: String::from("cc"),
            res: format!("{APPS_DIR}/{name}"),
            before: String::new(),
            after: String::from("not built"),
            inverse: None,
        };
        let outcome = format!("failed: {}", shown(why));
        // The source stays, as it does once an app is removed.
        self.push_app_entry(id, request, how, script, outcome, State::Nothing, change);
    }

    #[allow(clippy::too_many_arguments)]
    fn push_app_entry(
        &mut self,
        id: u64,
        request: &str,
        how: How,
        script: String,
        outcome: String,
        state: State,
        change: Change,
    ) {
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: how.form,
            of: None,
            tier: Some(how.tier),
            pack: None,
            consent: how.consent,
            state,
            request: String::from(request),
            script,
            outcome,
            changes: alloc::vec![change],
        });
    }
}
