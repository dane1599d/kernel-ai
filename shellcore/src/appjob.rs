//! `app_build` in the background (docs/decisions.md, 2026-11-03, the
//! manager's option (a)). One full answer from the `c-app-1` card takes
//! minutes, which never fits in the terminal's wait for a line. So on a host
//! that can ask the model off the line ([`crate::Host::script_start`]):
//!
//! - the line answers at once, "Building timer…", and the rounds go on
//!   between lines ([`Shell::app_poll`]), one build at a time;
//! - `build status` says how far it is, and `stop building` stops it,
//!   logged, with the source written so far kept;
//! - what came of it is a notice ([`Shell::take_notices`]), shown ahead of
//!   the next line's reply. The build waits for its install card (piece 4),
//!   which asks for itself.
//!
//! The model's answer is the slow part, so only it runs off the line: each
//! answer is checked, kept, built and tried between lines as one is within a
//! line ([`Shell::app_answer`]).

use crate::Shell;
use crate::actions::How;
use crate::appbuild::{Built, Next, Rounds};
use crate::ccbuild::APPS_DIR;
use crate::cmode::program::MAX_REPAIRS;
use crate::{PanelResult, Reply};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The longest the model may take over one app answer off the line: a full
/// answer at about 4.8 tokens a second, with room for a slower guest. llm
/// gives an app round as long.
pub const APP_ROUND_MS: u64 = llmproto::script::APP_SERVICE_TIMEOUT_MS;
/// The lines that ask after a build in the background.
pub const STATUS_LINE: &str = "build status";
pub const STOP_LINE: &str = "stop building";

/// What the host did with a request to ask the model off the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Started {
    /// It is asking: its answer comes back with this ticket.
    Ticket(u64),
    /// It cannot ask off the line here: the line asks itself.
    Inline,
    /// It is still asking for another request, such as a stopped build's.
    Busy(String),
}

/// A build going on between lines.
#[derive(Clone, Debug)]
pub(crate) struct Job {
    id: u64,
    request: String,
    how: How,
    rounds: Rounds,
    /// The ticket of the answer it waits for.
    ticket: u64,
}

impl Shell {
    /// Starts `app_build` off the line, if the host can: the line's reply.
    /// `None` when it cannot, so the line builds within itself.
    pub(crate) fn app_start(
        &mut self,
        id: u64,
        request: &str,
        name: &str,
        wanted: &str,
        how: &How,
    ) -> Option<Reply> {
        if let Some(job) = &self.app_job {
            return Some(Reply::err(format!(
                "Kai is already building {}: `{STATUS_LINE}` says how far it is, and `{STOP_LINE}` stops it",
                job.rounds.name
            )));
        }
        if let Err(why) = self.app_check_source(name) {
            self.log_app_failed(id, request, name, wanted, how.clone(), &why);
            return Some(Reply::err(why));
        }
        let mut rounds = Rounds::new(name, wanted);
        let ask = rounds.ask(APP_ROUND_MS);
        let ticket = match self.host.script_start(&ask) {
            Started::Inline => return None,
            Started::Busy(why) => {
                return Some(Reply::err(format!(
                    "Kai cannot start building {name} yet: {why}"
                )));
            }
            Started::Ticket(ticket) => ticket,
        };
        self.host.audit(&format!(
            "request {id}: app_build {name} runs in the background"
        ));
        self.app_job = Some(Job {
            id,
            request: String::from(request),
            how: how.clone(),
            rounds,
            ticket,
        });
        Some(Reply::ok(format!(
            "Building {name}: Kai is writing it, which takes a few minutes. `{STATUS_LINE}` says how far it is, and `{STOP_LINE}` stops it."
        )))
    }

    /// Goes on with a build in the background once the model has answered:
    /// the host calls it between lines. What the build came to when it
    /// ends, for the panel, which shows it on its own as Done or Refused.
    pub fn app_poll(&mut self) -> Option<PanelResult> {
        let (ticket, answer) = self.host.script_take()?;
        let mut job = self.app_job.take()?;
        // A stopped build's late answer is not this build's.
        if job.ticket != ticket {
            self.app_job = Some(job);
            return None;
        }
        let ended = match self.app_answer(&mut job.rounds, answer) {
            Next::Ask => {
                let ask = job.rounds.ask(APP_ROUND_MS);
                match self.host.script_start(&ask) {
                    Started::Ticket(ticket) => {
                        job.ticket = ticket;
                        self.app_job = Some(job);
                        None
                    }
                    Started::Inline | Started::Busy(_) => {
                        let why = String::from("the model could not take its repair");
                        Some(self.app_finish(job, Err(why)))
                    }
                }
            }
            Next::Built(built) => Some(self.app_finish(job, Ok(*built))),
            Next::Failed(why) => Some(self.app_finish(job, Err(why))),
        };
        self.save_log();
        ended
    }

    /// Logs what came of a build in the background, and keeps its notice:
    /// the panel's result.
    fn app_finish(&mut self, job: Job, result: Result<Built, String>) -> PanelResult {
        let Job {
            id,
            request,
            how,
            rounds,
            ..
        } = job;
        let (notice, shown) = match result {
            Ok(built) => {
                // Its install card opens by itself when the terminal may.
                self.card = Some(rounds.name.clone());
                let text = self
                    .log_app_build(id, &request, &rounds.wanted, how, built)
                    .text;
                (text.clone(), PanelResult::Done(text))
            }
            Err(why) => {
                self.log_app_failed(id, &request, &rounds.name, &rounds.wanted, how, &why);
                let text = format!("Kai's app {} was not built: {why}", rounds.name);
                (text, PanelResult::Refused(why))
            }
        };
        self.notices.push(notice);
        shown
    }

    /// `build status` and `stop building`; `None` for any other line.
    pub(crate) fn app_job_line(&mut self, input: &str) -> Option<Reply> {
        match input {
            STATUS_LINE => Some(self.app_status()),
            STOP_LINE => Some(self.app_stop()),
            _ => None,
        }
    }

    fn app_status(&self) -> Reply {
        let Some(job) = &self.app_job else {
            return Reply::ok("Kai is building no app.");
        };
        let name = &job.rounds.name;
        Reply::ok(match job.rounds.round {
            0 => format!("Kai is writing {name}."),
            n => format!("Kai is repairing {name}: repair {n} of {MAX_REPAIRS}."),
        })
    }

    fn app_stop(&mut self) -> Reply {
        let Some(job) = self.app_job.take() else {
            return Reply::err("Kai is building no app");
        };
        let name = job.rounds.name.clone();
        self.host.script_stop(job.ticket);
        let kept = self.app_written.iter().any(|(n, _)| *n == name);
        self.app_finish(job, Err(String::from("stopped by the person")));
        // Its notice is this reply.
        self.notices.pop();
        Reply::ok(match kept {
            true => format!(
                "Stopped building {name}. What Kai wrote so far stays in {APPS_DIR}/{name}/."
            ),
            false => format!("Stopped building {name}. Kai had written none of it yet."),
        })
    }

    /// Esc on the panel: stops the build in the background, if one runs.
    pub fn stop_app_build(&mut self) -> Option<Reply> {
        self.app_job.as_ref()?;
        Some(self.app_stop())
    }

    /// Whether an install card waits to open by itself: the host then says so
    /// to the kernel's terminal (CARD_READY), which asks for it with
    /// [`termcore::menu::TAKE_CARD`] when it may.
    pub fn card_waiting(&self) -> bool {
        self.card.is_some() || self.net_card_waiting()
    }

    /// The terminal's [`termcore::menu::TAKE_CARD`]: the waiting install
    /// card, the same card `install <name>` opens, with its checks; a bare
    /// `ok` when none waits, or while another ask is up (the card then
    /// waits on).
    pub(crate) fn take_card(&mut self) -> Reply {
        if self.pending.is_some() {
            return Reply::ok("");
        }
        let Some(name) = self.card.take() else {
            // An app's network ask waits as a card too (P5 task 13).
            return self.take_net_card().unwrap_or_else(|| Reply::ok(""));
        };
        // No one typed this line: a card that may not open (its build edited
        // or replaced since it ended) is a notice, not an error or a log entry
        // of a request no one made (A-core on #647).
        if let Some(why) = self.card_refusal(&name) {
            self.notices.push(format!(
                "Kai's app {name}'s install card did not open: {why}"
            ));
            return Reply::ok("");
        }
        let id = self.take_id();
        let request = format!("install {name} (the card opened when the build ended)");
        self.app_install_ask(id, &request, &name)
    }

    /// What builds in the background came to since the last look, for the
    /// desktop; the terminal sees them ahead of its next reply.
    pub fn take_notices(&mut self) -> Vec<String> {
        core::mem::take(&mut self.notices)
    }
}
