//! The session's builds (M9d). `build` starts one in the background
//! (BUILD_START) and answers at once. The service looks at it (BUILD_POLL)
//! before each open, command and sync, and every 250 ms between requests while
//! it runs; once its result is in, it is checked and stored as a build always
//! was, logged, and kept for `status`. A service that starts first finds out
//! whether the one before it left a build, and refuses builds and syncs while
//! one runs: the host's compile holds the lock a sync needs.

use super::building::{body_of, check_artifact, location, refusal, short};
use super::{Env, Service, err, ok};
use crate::wire::{self, BuildResult};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use livecore::artifact;

/// Diagnostic lines a failed build shows.
const DIAGNOSTICS: usize = 8;
/// How a failed build that moved the cursor begins, after `err `.
const MOVED: &str = "build failed; the cursor is on the first error";
/// What `status` says of a build the service before this one started.
const INHERITED: &str = "started before the edit service restarted";

/// A build this service started, and the text it sent.
pub(super) struct Running {
    pub(super) component: String,
    pub(super) next: String,
    pub(super) base: String,
    pub(super) bundle: Vec<u8>,
}

pub(super) enum Job {
    Running(Running),
    /// A build the service before this one may have left; `busy` while the last
    /// look found it running.
    Inherited {
        busy: bool,
    },
}

/// What the service knows of its builds.
pub(super) struct Builds {
    pub(super) job: Option<Job>,
    /// The answer the last build gave, once it finished.
    pub(super) last: Option<String>,
    /// An open or command came since the session was replaced, so a finished
    /// build's result may be taken; before that, it is only looked at.
    pub(super) decided: bool,
    /// The text the journal gave back, when an open or command first came after
    /// the session was replaced with a build from before still unknown: a build
    /// is adopted only if it was built from exactly this.
    pub(super) restart_text: Option<Vec<u8>>,
    /// Where a failed build moved the cursor, for the next open or command's
    /// reply to say ([`crate::ipc::with_moved`]).
    pub(super) moved: Option<String>,
}

impl Builds {
    /// A new service does not know yet whether a build runs; its first look says.
    pub(super) const fn new() -> Self {
        Self {
            job: Some(Job::Inherited { busy: true }),
            last: None,
            decided: false,
            restart_text: None,
            moved: None,
        }
    }
}

/// `err busy building <label>`, or without a label when none is known.
pub(super) fn busy_reply(label: &str) -> String {
    match label {
        "" => err("busy building"),
        label => err(format!("busy building {label}")),
    }
}

/// Poll errors after which the job is still the kernel's: it runs, or the reply
/// could not be copied and was put back.
fn kept(error: Error) -> bool {
    matches!(error, Error::Busy | Error::Fault | Error::TooBig)
}

/// Stores a build's artifact as the next upload; why not, as the founder reads it.
fn store(
    env: &mut dyn Env,
    component: &str,
    artifact: &[u8],
    base: &str,
    commit: &str,
    next: &str,
) -> Result<(), String> {
    match env.put(component, artifact, base, commit) {
        Ok(()) => Ok(()),
        Err(Error::Invalid) => Err(refusal(env, component, next, base)),
        Err(Error::NoMemory) => Err(String::from("the store has no room for another upload")),
        Err(error) => Err(format!("the store refused {next}: {error:?}")),
    }
}

impl Service {
    /// Whether a build is known to run, so the service should look again soon.
    pub fn polling(&self) -> bool {
        matches!(
            self.builds.job,
            Some(Job::Running(_) | Job::Inherited { busy: true })
        )
    }

    /// An open or a command arrived: from now on a result may be taken. The first
    /// after a restart keeps the replayed text, which an inherited build must match.
    pub(super) fn decide(&mut self) {
        if self.builds.decided {
            return;
        }
        self.builds.decided = true;
        if matches!(self.builds.job, Some(Job::Inherited { .. })) {
            self.builds.restart_text = self.session.as_ref().map(|session| session.to_bundle());
        }
    }

    /// Looks at the build, and handles its result once it is in.
    pub fn poll(&mut self, env: &mut dyn Env) {
        match self.builds.job.take() {
            None => {}
            Some(Job::Inherited { busy }) => self.poll_inherited(env, busy),
            Some(job @ Job::Running(_)) if !self.builds.decided => self.builds.job = Some(job),
            Some(Job::Running(running)) => match env.build_poll(true) {
                Err(error) if kept(error) => self.builds.job = Some(Job::Running(running)),
                Err(Error::NotFound) => {
                    let lost = format!("the build of {} was lost; build again", running.next);
                    self.done(env, err(lost));
                }
                Err(error) => self.done(env, err(format!("build service: {error:?}"))),
                Ok(response) => {
                    let answer = self.finish(env, &running, &response);
                    self.done(env, answer);
                }
            },
        }
        if self.builds.job.is_none() {
            self.builds.restart_text = None;
        }
    }

    /// A build from before this service: looked at until an open or command
    /// comes, then taken.
    fn poll_inherited(&mut self, env: &mut dyn Env, busy: bool) {
        let take = self.builds.decided;
        match env.build_poll(take) {
            Err(Error::Busy) => self.builds.job = Some(Job::Inherited { busy: true }),
            Err(error) if kept(error) => self.builds.job = Some(Job::Inherited { busy }),
            Err(Error::NotFound) => {}
            Ok(_) if !take => self.builds.job = Some(Job::Inherited { busy: false }),
            Err(error) => {
                let failed =
                    format!("a build from before the restart failed ({error:?}); build again");
                self.done(env, err(failed));
            }
            Ok(response) => {
                let answer = self.adopt(env, &response);
                self.done(env, answer);
            }
        }
    }

    /// A result from before the restart, taken: stored only if it was built on
    /// the session's version from the text the journal gave back. It never moves
    /// the cursor.
    fn adopt(&mut self, env: &mut dyn Env, response: &[u8]) -> String {
        let text = self.builds.restart_text.take();
        let failed = |why: String| {
            err(format!(
                "a build from before the restart failed ({why}); build again"
            ))
        };
        let body = match body_of(response) {
            Ok(body) => body,
            Err(why) => return failed(format!("build service: {why}")),
        };
        let built = match wire::decode_built(&body) {
            Ok(built) => built,
            Err(why) => return failed(format!("build service: a malformed reply ({why})")),
        };
        if built.result != BuildResult::Ok {
            return err("a build from before the restart failed; build again");
        }
        let label = artifact::parse(built.artifact)
            .map(|parsed| parsed.manifest.version)
            .unwrap_or_else(|_| String::from("the upload"));
        let dropped = |why: &str| {
            err(format!(
                "dropped {label}: it was built before the edit service restarted, {why}"
            ))
        };
        let Some(session) = self.session.as_ref() else {
            return dropped("and no session came back");
        };
        let (component, base) = (session.component.clone(), session.label.clone());
        let next = match env.info(&component) {
            Ok(info) => info.next().to_string(),
            Err(error) => return err(format!("cannot read {component}: {error:?}")),
        };
        let Some(text) = text.filter(|text| {
            built.base == base && check_artifact(built.artifact, &component, &next, text).is_ok()
        }) else {
            return dropped("from other text");
        };
        if let Err(why) = store(env, &component, built.artifact, &base, built.commit, &next) {
            return err(why);
        }
        let unchanged = self
            .session
            .as_mut()
            .filter(|session| session.to_bundle() == text);
        let suffix = match unchanged {
            Some(session) => {
                session.built = Some(next.clone());
                ""
            }
            None => "; your edits since are not in it",
        };
        ok(format!(
            "built {next} from {base} (commit {}); :apply runs it{suffix}",
            short(built.commit)
        ))
    }

    /// A build ended: its answer is logged and kept for `status`.
    fn done(&mut self, env: &mut dyn Env, answer: String) {
        let shown = answer
            .strip_prefix("ok ")
            .or_else(|| answer.strip_prefix("err "))
            .unwrap_or(&answer);
        env.log(shown);
        self.builds.last = Some(answer);
    }

    /// What a build's response comes to: stored, refused or failed.
    fn finish(&mut self, env: &mut dyn Env, running: &Running, response: &[u8]) -> String {
        let body = match body_of(response) {
            Ok(body) => body,
            Err(why) => return err(format!("build service: {why}")),
        };
        let built = match wire::decode_built(&body) {
            Ok(built) => built,
            Err(why) => return err(format!("build service: a malformed reply ({why})")),
        };
        let log = String::from_utf8_lossy(built.log);
        let Running {
            component,
            next,
            base,
            bundle,
        } = running;
        match built.result {
            BuildResult::Refused => err(format!("build refused: {}", log.trim())),
            BuildResult::Failed => self.failed(running, &log),
            BuildResult::Ok => {
                if let Err(why) = check_artifact(built.artifact, component, next, bundle) {
                    return err(format!(
                        "the build service returned the wrong artifact: {why}"
                    ));
                }
                if let Err(why) = store(env, component, built.artifact, base, built.commit, next) {
                    return err(why);
                }
                // Built only if the founder's text is still what was sent.
                let unchanged = self.session.as_mut().filter(|session| {
                    session.component == *component
                        && session.label == *base
                        && session.to_bundle() == *bundle
                });
                let suffix = match unchanged {
                    Some(session) => {
                        session.built = Some(next.clone());
                        ""
                    }
                    None => "; your edits since are not in it",
                };
                ok(format!(
                    "built {next} from {base} (commit {}); :apply runs it{suffix}",
                    short(built.commit)
                ))
            }
        }
    }

    /// A failed build: its first errors. The cursor goes to the first only when
    /// the session's text is still what was sent.
    fn failed(&mut self, running: &Running, log: &str) -> String {
        let lines: Vec<&str> = log
            .lines()
            .filter(|line| line.contains("error"))
            .take(DIAGNOSTICS)
            .collect();
        if let Some(session) = self.session.as_mut()
            && session.component == running.component
            && session.label == running.base
            && session.to_bundle() == running.bundle
            && let Some((path, line)) = lines.iter().find_map(|line| location(line))
            && path.len() <= livecore::location::MAX_PATH
            && session.goto(path, line)
        {
            self.builds.moved = Some(format!("{path}:{}", session.line()));
            let mut reply = err(MOVED);
            for line in &lines {
                reply.push('\n');
                reply.push_str(line);
            }
            return reply;
        }
        let mut reply = err("build failed");
        for line in if lines.is_empty() {
            log.lines().take(DIAGNOSTICS).collect()
        } else {
            lines
        } {
            reply.push('\n');
            reply.push_str(line);
        }
        reply
    }

    /// The label a running build will get, while one runs: Some("") when the
    /// service before this one started it and no session says which.
    pub(super) fn busy(&mut self, env: &mut dyn Env) -> Option<String> {
        match &self.builds.job {
            Some(Job::Running(running)) => Some(running.next.clone()),
            Some(Job::Inherited { busy: true }) => Some(
                self.session
                    .as_ref()
                    .and_then(|session| env.info(&session.component).ok())
                    .map(|info| info.next().to_string())
                    .unwrap_or_default(),
            ),
            _ => None,
        }
    }

    /// `status`: the first line is `ok <component> <label>: unchanged|edited[,
    /// built <label>][; building ...]`, or `ok no file is open[; building ...]`;
    /// a `last build: <answer>` line follows once a build has finished.
    pub(super) fn status(&mut self, env: &mut dyn Env) -> String {
        let label = self.busy(env);
        let building = match (&self.builds.job, label) {
            (Some(Job::Running(running)), _) => {
                format!("; building {} from {}", running.next, running.base)
            }
            (_, Some(label)) if label.is_empty() => format!("; building, {INHERITED}"),
            (_, Some(label)) => format!("; building {label}, {INHERITED}"),
            _ => String::new(),
        };
        let mut out = match &self.session {
            Some(session) => {
                let state = if session.changed() {
                    "edited"
                } else {
                    "unchanged"
                };
                let built = match &session.built {
                    Some(built) => format!(", built {built}"),
                    None => String::new(),
                };
                ok(format!(
                    "{} {}: {state}{built}{building}",
                    session.component, session.label
                ))
            }
            None => ok(format!("no file is open{building}")),
        };
        if let Some(last) = &self.builds.last {
            out.push_str("\nlast build: ");
            out.push_str(last);
        }
        out
    }
}
