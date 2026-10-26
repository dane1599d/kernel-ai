//! Rows the kernel terminal runs for the shell (`agentabi::TERMINAL_ROWS`): open
//! the editor at a line, apply a component's newest build, revert one. After
//! consent, the shell's reply is a `do` line (`termcore::doing`); the terminal
//! parses it into a closed request, runs its own `:edit`, `:apply` or `:revert`
//! code, and answers `done <id> ok|err ...`, which ends the log entry and gives
//! the result line. A call that asked carries its menu's `ONCE` on the `do` line,
//! and the kernel checks it there, so the shell does not use it up itself.

use crate::actions::{How, Pending};
use crate::actlog::Form;
use crate::rules::Offer;
use crate::script::Checked;
use crate::tiers::Tier;
use crate::{Reply, Shell};
use agentabi::Action;
use agentabi::consent::{Decision, Why};
use alloc::format;
use alloc::string::{String, ToString};
use llmproto::act::Call;
use termcore::doing::{Do, Request};
use termcore::menu::{Answer, read_answer};

/// A `do` the terminal is running: its entry, until its `done` arrives.
pub(crate) struct Doing {
    pub id: u64,
    pub request: String,
    pub call: String,
    pub how: How,
}

/// The terminal's request for `row` with `values`, checked as the terminal will
/// check it: names, paths and numbers in range. `editor_open`'s path is
/// `<component>/<file>`.
pub(crate) fn request_for(
    id: u64,
    row: &Action,
    values: &[(&'static str, String)],
) -> Result<Request, String> {
    let get = |name: &str| {
        values
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    };
    let what = match row.name {
        "editor_open" => {
            let path = get("path").unwrap_or_default();
            let (component, file) = path
                .split_once('/')
                .ok_or("the path is <component>/<file>, such as notes/src/lib.rs")?;
            Do::Edit {
                component: component.to_string(),
                path: file.to_string(),
                line: get("line")
                    .map_or(Ok(1), str::parse)
                    .map_err(|_| "a bad line")?,
            }
        }
        "component_apply" => Do::Apply {
            component: get("component").unwrap_or_default().to_string(),
        },
        "component_revert" => Do::Revert {
            component: get("component").unwrap_or_default().to_string(),
            back: get("back")
                .map_or(Ok(1), str::parse)
                .map_err(|_| "a bad count")?,
        },
        other => return Err(format!("{other} is not run by the terminal")),
    };
    let request = Request {
        id,
        what,
        once: None,
    };
    // What the terminal would refuse is refused here first, with its reason.
    Request::decode(&request.encode())
        .map(|_| request)
        .map_err(String::from)
}

impl Shell {
    /// A terminal row consent let through, or that asks: the `do` line, or the
    /// Allow / Deny menu with the `do` waiting on its answer.
    pub(crate) fn terminal_row(
        &mut self,
        id: u64,
        request: &str,
        call: &Call,
        shown: &str,
        decision: Decision,
        what: Request,
        offer: Option<Offer>,
    ) -> Reply {
        let (tier, why) = (decision.tier, decision.why);
        let how = How {
            form: Form::Call,
            tier,
            consent: String::from(match why {
                Why::TakenAs => "taken-as",
                _ => "asked",
            }),
        };
        if tier == Tier::Run {
            self.host
                .audit(&format!("request {id}: the terminal runs {}", call.name));
            return self.send_do(id, request, &call.text(), how, what);
        }
        let reply = self.ask_call(id, request, Checked::shown_only(shown), why, offer);
        if let Some(pending) = &mut self.pending {
            pending.terminal = Some(what);
        }
        reply
    }

    fn send_do(&mut self, id: u64, request: &str, call: &str, how: How, what: Request) -> Reply {
        let line = what.encode();
        self.doing = Some(Doing {
            id,
            request: request.to_string(),
            call: call.to_string(),
            how,
        });
        Reply::to_terminal(id, line)
    }

    /// The answer to a terminal row's menu: an Allow sends the `do` with the
    /// menu's `ONCE`, which the terminal checks; anything else declines.
    pub(crate) fn answer_terminal(&mut self, pending: Pending, text: &str) -> Reply {
        let id = pending.id;
        let Some(what) = pending.terminal.clone() else {
            return Reply::err("nothing to confirm");
        };
        let granted = match read_answer(text.trim()) {
            Some((Answer::Deny, _)) | None => {
                self.record(id, &pending.request, &pending.checked.script, "declined");
                self.remember(&pending, "Not run: declined");
                return Reply::ok(format!("script {id} not run"));
            }
            Some((_, granted)) => granted,
        };
        let how = How {
            consent: String::from("yes"),
            ..pending.how
        };
        let what = Request {
            once: granted.once,
            ..what
        };
        // The call as `act` returned it, for llm's conversation memory.
        let call = pending
            .call
            .clone()
            .unwrap_or_else(|| pending.checked.script.clone());
        self.send_do(id, &pending.request, &call, how, what)
    }

    /// `done <id> ok <text>` or `done <id> err <why>` from the terminal: the end
    /// of the `do` it ran, and the result line.
    pub(crate) fn done_line(&mut self, rest: &str) -> Reply {
        let (id, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        let id = id.parse::<u64>().ok();
        let Some(doing) = self.doing.take().filter(|d| Some(d.id) == id) else {
            return Reply::err("nothing to finish");
        };
        let (outcome, reply) = match rest.split_once(' ').unwrap_or((rest, "")) {
            ("ok", text) => ("ran", Reply::ok(format!("Done: {text}"))),
            ("err", why) => ("refused", Reply::err(format!("Not run: {why}"))),
            _ => (
                "refused",
                Reply::err("Not run: the terminal's answer was malformed"),
            ),
        };
        self.host.audit(&format!(
            "request {} {outcome} by the terminal ({})",
            doing.id, doing.how.consent
        ));
        self.record(doing.id, &doing.request, &doing.call, outcome);
        crate::agentcore::remember(self.host.as_ref(), &doing.request, &doing.call, &reply.text);
        reply
    }
}
