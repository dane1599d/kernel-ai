//! App rounds as llm jobs (plan "Kai builds apps"). A shell worker thread
//! cannot make IPC calls, so `app_build` starts a round and comes back for it:
//!
//! - `app-start <script request, lang app>`: `ok <ticket>` at once, `busy
//!   <why>` while another job runs, or `inline` where rounds are not jobs (a
//!   server backend, or no model), so the shell sends the `app` request itself;
//! - `app-take <ticket>`: `pending`, the round's answer as a `script` reply
//!   (`ok <answer>` or `err <kind> <why>`), or `ended <why>` for a job that is
//!   gone: stopped, finished and taken, or a ticket of another run of llm;
//! - `app-stop <ticket>`: `ok`, the job's generation stopped and the job
//!   dropped.
//!
//! The local engine writes the app between requests: each step it looks for
//! one, and [`Jobs::while_running`] answers it. A job is never pending
//! forever: its round is bounded by the request's time limit. This is the
//! state; llm's service loop runs the engine.

use crate::act::{self, ActError};
use crate::health::{State as Health, Status};
use crate::script::{self, AnswerError, Lang, ScriptRequest};
use alloc::format;
use alloc::string::{String, ToString};

/// A job's state.
enum State {
    /// Generating; a stop asked for ends it at the next step.
    Running { stop: bool },
    /// The round's reply, waiting for `app-take`.
    Done(String),
}

struct Job {
    ticket: u64,
    /// What the refusals name: the app and whether this round is a repair.
    about: String,
    state: State,
}

/// llm's app jobs: one at a time.
pub struct Jobs {
    /// Tickets carry the run of llm they came from, so one from before a
    /// restart is unknown.
    restart: u64,
    issued: u64,
    job: Option<Job>,
}

/// What `app-start` does.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    /// Reply `ok <ticket>`, then run the round.
    Run(u64, ScriptRequest),
    /// Reply this, and run nothing.
    Reply(String),
}

impl Jobs {
    pub const fn new(restart: u64) -> Self {
        Self {
            restart,
            issued: 0,
            job: None,
        }
    }

    /// `app-start`'s request `rest`, where `local` says whether rounds run as
    /// jobs here.
    pub fn start(&mut self, rest: &str, local: bool) -> Start {
        if !local {
            return Start::Reply(String::from("inline"));
        }
        let Some(request) = ScriptRequest::decode(rest).filter(|r| r.lang == Lang::App) else {
            return Start::Reply(String::from("err malformed app request"));
        };
        if let Some(Job {
            about,
            state: State::Running { .. },
            ..
        }) = &self.job
        {
            return Start::Reply(format!("busy Kai is writing {about}"));
        }
        self.issued += 1;
        let ticket = (self.restart << 32) | (self.issued & 0xffff_ffff);
        self.job = Some(Job {
            ticket,
            about: about(&request),
            state: State::Running { stop: false },
        });
        Start::Run(ticket, request)
    }

    /// The reply to `text` while the job runs, and whether to stop it.
    pub fn while_running(&mut self, text: &str) -> (String, bool) {
        let (command, rest) = text.split_once([' ', '\n']).unwrap_or((text, ""));
        let Some(job) = &mut self.job else {
            return (String::from("err no app job runs"), false);
        };
        let mine = rest.trim().parse::<u64>().ok() == Some(job.ticket);
        let busy = format!(
            "Kai is writing {}; say `stop building` to stop it",
            job.about
        );
        match command {
            "app-take" if mine => (String::from("pending"), false),
            "app-take" => (String::from("ended unknown ticket"), false),
            "app-stop" if mine => {
                job.state = State::Running { stop: true };
                (String::from("ok"), true)
            }
            "app-stop" => (String::from("ok"), false),
            "app-start" => (format!("busy Kai is writing {}", job.about), false),
            // Both slots are not warm while a job holds the script slot.
            "warm" => (String::from("ok no"), false),
            // The Monitor app reads the model as busy, not unknown.
            "health" => {
                let status = Status {
                    state: Health::Busy,
                    tps10: 0,
                };
                (
                    format!("ok Kai is writing {}; {}", job.about, status.line()),
                    false,
                )
            }
            "script" | "program" | "app" => (
                format!(
                    "err {}",
                    script::encode_error(&AnswerError::Unavailable(busy))
                ),
                false,
            ),
            "act" => (
                format!(
                    "ok {}",
                    act::encode_answer(&Err(ActError::Unavailable(busy)))
                ),
                false,
            ),
            _ => (format!("err {busy}"), false),
        }
    }

    /// The round of job `ticket` ended with `reply`, a `script` reply; a
    /// stopped job is dropped instead.
    pub fn finish(&mut self, ticket: u64, reply: String) {
        let Some(job) = self.job.as_mut().filter(|job| job.ticket == ticket) else {
            return;
        };
        match job.state {
            State::Running { stop: true } => self.job = None,
            _ => job.state = State::Done(reply),
        }
    }

    /// `app-take`'s reply while no job runs.
    pub fn take(&mut self, rest: &str) -> String {
        let ticket = rest.trim().parse::<u64>().ok();
        match self.job.take() {
            Some(Job {
                ticket: t,
                state: State::Done(reply),
                ..
            }) if Some(t) == ticket => reply,
            other => {
                self.job = other;
                String::from("ended unknown ticket")
            }
        }
    }

    /// `app-stop`'s reply while no job runs: a finished job of that ticket is
    /// dropped.
    pub fn stop(&mut self, rest: &str) -> String {
        let ticket = rest.trim().parse::<u64>().ok();
        if self.job.as_ref().map(|job| job.ticket) == ticket {
            self.job = None;
        }
        String::from("ok")
    }
}

/// The app a request is for, as its first line names it, and whether it is a
/// repair.
fn about(request: &ScriptRequest) -> String {
    let name = request.request.split('\n').next().unwrap_or("").trim();
    let name = match name.is_empty() {
        true => "an app",
        false => name,
    };
    match request.repair {
        Some(_) => format!("{name} (a repair)"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests;
