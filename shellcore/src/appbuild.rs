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
//! 5. repairs a refused or failed one at most [`MAX_REPAIRS`] times, each with
//!    exactly why: the check's problems or `cc`'s diagnostics.
//!
//! Nothing is installed: the install card asks for that (piece 4), so the
//! build runs. The log records the request, the source's SHA-256 and `cc`'s
//! `built-by cc` line, which `:why` shows.

use crate::actions::How;
use crate::actlog::{Change, Entry, Form, State};
use crate::ccbuild::{APPS_DIR, CcBuilt, shown, valid_name, valid_version};
use crate::cmode::program::MAX_REPAIRS;
use crate::tiers::Tier;
use crate::{Reply, SCRIPT_ROUND_MS, Shell};
use agentabi::consent::{Decision, Why};
use alloc::format;
use alloc::string::{String, ToString};
use llmproto::script::{AnswerError, Lang, ScriptRequest};
use sha2::{Digest, Sha256};

/// The most time one `app_build` spends asking the model and building, so the
/// whole of it answers within the terminal's wait for a line (310 s).
pub const BUILD_BUDGET_MS: u64 = 280_000;
/// The longest request the card gets, as the row takes it.
const MAX_REQUEST: usize = 300;

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
        let consent = match decision.why {
            Why::TakenAs => "taken-as",
            _ => "asked",
        };
        let how = How {
            form: Form::Call,
            tier: Tier::Run,
            consent: String::from(consent),
        };
        match self.app_build(name, wanted) {
            Ok(built) => self.log_app_build(id, request, wanted, how, built),
            Err(why) => {
                self.log_app_failed(id, request, name, wanted, how, &why);
                Reply::err(why)
            }
        }
    }

    /// The write, check, build and repair loop: what was built, or why not.
    fn app_build(&mut self, name: &str, wanted: &str) -> Result<Built, String> {
        let started = self.host.now_ns();
        let mut repair: Option<(String, String)> = None;
        for round in 0..=MAX_REPAIRS {
            let spent_ms = self.host.now_ns().saturating_sub(started) / 1_000_000;
            if round > 0 && spent_ms + SCRIPT_ROUND_MS > BUILD_BUDGET_MS {
                let why = repair.map(|(_, why)| why).unwrap_or_default();
                return Err(format!(
                    "Kai's app {name} did not build in time, after {round} tries:\n{why}"
                ));
            }
            let ask = ScriptRequest {
                request: format!("{name}\n{wanted}"),
                repair: repair.take(),
                timeout_ms: SCRIPT_ROUND_MS,
                lang: Lang::App,
            };
            let answer = match self.host.script(&ask) {
                Ok(answer) => answer,
                Err(AnswerError::NoScript(why)) => {
                    return Err(format!("Kai wrote no app: {}", shown(&why)));
                }
                Err(error) if error.repairable() && round < MAX_REPAIRS => {
                    repair = Some((String::new(), error.describe()));
                    continue;
                }
                Err(error) => return Err(error.describe()),
            };
            if answer.script.trim().is_empty() {
                return Err(format!("Kai wrote no app: {}", shown(&answer.summary)));
            }
            let prepared = llmproto::precheck::prepare(&answer.script, &[]);
            let why = match prepared.problems.is_empty() {
                false => format!(
                    "That app was refused before compiling:\n{}",
                    prepared.problems.join("\n")
                ),
                true => match self.app_compile(name, &prepared.source) {
                    Ok((version, built)) => {
                        return Ok(Built {
                            name: name.to_string(),
                            version,
                            summary: answer.summary,
                            source: sha256(&prepared.source),
                            built,
                        });
                    }
                    Err(why) => why,
                },
            };
            if round == MAX_REPAIRS {
                return Err(format!(
                    "Kai's app {name} did not build after {MAX_REPAIRS} repairs:\n{why}"
                ));
            }
            repair = Some((prepared.source, why));
        }
        Err(format!("Kai wrote no app {name}"))
    }

    /// Keeps `source` as the app's `main.c` and builds it: the version and
    /// what `cc` built, or why not, as the repair gets it.
    fn app_compile(&mut self, name: &str, source: &str) -> Result<(String, CcBuilt), String> {
        self.host
            .app_source(name, source)
            .map_err(|why| format!("the app's source was not kept: {}", shown(&why)))?;
        let version = match self.host.pkg(&format!("local-next {name}")) {
            Ok(version) if valid_version(&version) => version,
            Ok(other) => return Err(format!("pkg's next version is not one: {}", shown(&other))),
            Err(why) => return Err(format!("pkg: {}", shown(&why))),
        };
        let built = self
            .host
            .cc_build(APPS_DIR, name, &version)
            .map_err(|why| format!("cc build refused it:\n{why}"))?;
        Ok((version, built))
    }

    /// Logs a build: the request, the source's hash and `cc`'s provenance.
    fn log_app_build(
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
        } = &built;
        let script = format!(
            "app_build(\"{name}\", {wanted:?})\nsource {APPS_DIR}/{name}/main.c sha256 {source}\n{}",
            cc.provenance
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
            shown(summary)
        );
        self.app_built = Some(built);
        Reply::ok(reply)
    }

    /// Logs a build that did not come to be, with why.
    fn log_app_failed(
        &mut self,
        id: u64,
        request: &str,
        name: &str,
        wanted: &str,
        how: How,
        why: &str,
    ) {
        let script = format!("app_build(\"{name}\", {wanted:?})");
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
