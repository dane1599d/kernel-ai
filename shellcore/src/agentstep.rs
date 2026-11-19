//! An agent's step through the one door (ADR 0013 §1): `agent-step <call>`
//! on the agent's own route to the shell, which the kernel badges (#1047).
//!
//! - **The gate.** A request from a route with an agent's badge may be only
//!   `agent-step …`; every other request an agent sends (`undo`, `net
//!   denied`, a panel or Terminal line, an `answer`) is refused. The kernel's
//!   own requests come with badge 0, and kai's badge is the person's shell:
//!   neither is an agent's step.
//! - **Who.** The caller is the roster's answer for the badge the kernel
//!   delivered, never a pid and never the request's text.
//! - **The ceiling.** The agent's own grants decide whether it may call the
//!   row at all (`agentid::Roster::admit`); the tiers decide below that.
//! - **No borrowed consent.** The step is decided on the empty line: the
//!   person's words never cue an agent's call, so every change asks and a
//!   read runs.
//! - **Its log entry** is stamped with the agent's name (`actlog 2`).
//! - **An ask** goes to the person's card, as a desktop line's does, and the
//!   step waits: `agent-step wait <id>` says what it came to, and `err unknown
//!   step` for an id it never asked.
//! - **A goal's step** (the `goals` agent, G2): `agent-step goal=<id> <call>`
//!   is admitted under that goal's ceiling (`agentid::Roster::
//!   admit_for_goal_in`), never `goals`' own grants, and refused once the
//!   goal ended or its ceiling was revoked; `agent-step goal=<id> end` drops
//!   the ceiling when the goal ends.

use crate::deskask::Asker;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};

/// The one request an agent's route may send.
pub const STEP: &str = "agent-step ";
/// What a card's answers to agents are kept for, every agent together.
pub(crate) const MAX_TOLD: usize = 16;
/// What an agent's step is told while the person's own question waits.
pub const PERSON_ASKED: &str = "busy: a question waits for the person; step again later";
/// What every other request from an agent's route is told.
pub const AGENTS_ONLY_STEP: &str = "an agent may send only agent-step";
/// What a step that carried handles is told: a step moves none.
pub const STEP_NO_HANDLES: &str = "an agent's step carries no handles";

/// What came of an agent's asked step on the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Told {
    pub(crate) agent: String,
    pub(crate) id: u64,
    pub(crate) ran: bool,
    pub(crate) line: String,
}

impl Shell {
    /// The gate for a request that came through a route with badge `badge`
    /// and moved `handles` handles (which the caller has closed): `None` for
    /// the kernel's own (badge 0), which the caller answers as before; for
    /// any other, the agent's step's wire, or a refusal.
    pub fn from_route(&mut self, badge: u64, handles: usize, text: &str) -> Option<String> {
        if badge == 0 {
            return None;
        }
        if handles > 0 {
            return Some(Reply::err(STEP_NO_HANDLES).to_wire());
        }
        let reply = match text.strip_prefix(STEP) {
            Some(rest) => self.agent_step(badge, rest.trim()),
            None => Reply::err(AGENTS_ONLY_STEP),
        };
        Some(reply.to_wire())
    }

    /// Agent `badge`'s step: `<call>` as the act model writes one, or
    /// `wait <id>` for an asked step's answer.
    pub fn agent_step(&mut self, badge: u64, text: &str) -> Reply {
        let caller = match u32::try_from(badge).map(|b| self.roster.caller(b)) {
            Ok(Ok(caller)) if !caller.is_kai() => caller,
            Ok(Ok(_)) => return Reply::err("kai's lines are typed, not stepped"),
            _ => return Reply::err("that route is no agent's"),
        };
        let name = caller.name().as_str().to_string();
        if let Some(reply) = self.goal_queue_step(&caller, text) {
            return reply;
        }
        if let Some(id) = text.strip_prefix("wait ") {
            return self.agent_waits(&name, id.trim());
        }
        let (goal, text) = match text.strip_prefix("goal=") {
            Some(rest) => {
                let (goal, call) = rest.split_once(' ').unwrap_or((rest, ""));
                match goal.parse::<u64>() {
                    Ok(goal) => (Some(goal), call.trim()),
                    Err(_) => return Reply::err("usage: agent-step goal=<id> <call>|end"),
                }
            }
            None => (None, text),
        };
        if let (Some(goal), "end") = (goal, text) {
            return self.goal_end(&caller, goal);
        }
        let call = match llmproto::act::parse(text) {
            Ok(call) => call,
            Err(why) => return Reply::err(format!("not a call: {why:?}")),
        };
        let admitted = match goal {
            Some(goal) => self
                .roster
                .admit_for_goal_in(&caller, goal, &call.name, &agentabi::ROWS),
            None => self.roster.admit(&caller, &call.name),
        };
        let admission = match admitted {
            Ok(admission) => admission,
            Err(denied) => {
                self.host
                    .audit(&format!("{name}'s step {} refused: {denied}", call.name));
                return Reply::err(format!("refused: {denied}"));
            }
        };
        // A question at the console waits for its answer: an agent's step
        // never takes its place. Only an admitted step learns that.
        if self.pending.is_some() || self.clarifying.is_some() {
            self.host.audit(&format!(
                "{name}'s step {} waits: a question is up",
                call.name
            ));
            return Reply::err(PERSON_ASKED);
        }
        let stamp = admission.stamp();
        let id = self.take_id();
        // The words the door gives an agent's line: none, so an agent
        // borrows no consent (ADR 0013 §1), and no rule of the person's
        // covers it while it runs (`agentconsent`). `acting` stays set for
        // the whole step, so every entry it logs is the agent's.
        let line = agentid::Line::new(text, agentid::Source::agent(caller.id()));
        let words = admission.consent_words(&line);
        self.acting = Some(admission);
        self.acting_from = id;
        // `show` and `why` stay about the person's last script.
        let shown = self.shown.take();
        let reply = self.agent_runs(id, words, &call, name);
        self.shown = shown;
        self.acting = None;
        self.stamp_entries(id, id, stamp.name());
        self.save_log();
        reply
    }

    /// The rest of an admitted step `id`, while `acting` holds its admission.
    fn agent_runs(
        &mut self,
        id: u64,
        words: &str,
        call: &llmproto::act::Call,
        name: String,
    ) -> Reply {
        let outcome = self.execute(id, words, call);
        // A "which one?" is the agent's to answer, never the person's next
        // line: the console's own clarifying was empty when the step began.
        if self.clarifying.take().is_some() {
            return Reply::err(format!("unclear #{id}: {}", outcome.reply.text));
        }
        if self.pending.as_ref().is_some_and(|p| p.id == id) {
            // The admission of the row that will run, which `call_row`
            // re-admitted if the row changed.
            let Some(admission) = self.acting.clone() else {
                return Reply::err("not asked: no admission");
            };
            if let Err(why) = self.open_agent_ask(id, &admission) {
                return Reply::err(format!("not asked: {why}"));
            }
            return match self.wait_for_card(&outcome.reply, Asker::Agent(name)) {
                Ok(_) => Reply::ok(format!("asked {id}: on the person's card")),
                Err(why) => {
                    self.close_agent_ask(id);
                    Reply::err(format!("not asked: {why}"))
                }
            };
        }
        let tier = outcome.tier.name();
        match outcome.reply.ok {
            true => Reply::ok(format!("{tier} #{id}: {}", outcome.reply.text)),
            false => Reply::err(format!("{tier} #{id}: {}", outcome.reply.text)),
        }
    }

    /// Stamps log entry `id` and every entry from `since` on with agent
    /// `name`. Every one of them is the step's (or its yes's run): the shell
    /// runs one request at a time on one thread, so no other line's entry
    /// can land between `since` being read and here. Entries between `id`
    /// and `since`, the person's lines while a card waited, stay theirs.
    pub(crate) fn stamp_entries(&mut self, id: u64, since: u64, name: &agentid::Name) {
        let stamped: alloc::vec::Vec<u64> = self
            .log
            .entries()
            .filter(|e| e.id == id || e.id >= since)
            .map(|e| e.id)
            .collect();
        for entry in stamped {
            if let Some(e) = self.log.get_mut(entry) {
                e.agent = name.clone();
            }
        }
    }

    /// `agent-step goal=<id> end`: goal `goal` ended, so its ceiling goes.
    /// Only `goals` ends a goal.
    fn goal_end(&mut self, caller: &agentid::Caller, goal: u64) -> Reply {
        if caller.name().as_str() != "goals" {
            return Reply::err(crate::goalrows::GOALS_ONLY);
        }
        let held = self.roster.revoke_goal(goal);
        self.goal_ended_by_goals(goal);
        self.host.audit(&format!(
            "goals ended goal {goal}; its ceiling went: {held}"
        ));
        Reply::ok(format!("ended goal {goal}"))
    }

    /// What agent `name`'s asked step `id` came to on the card, once.
    fn agent_waits(&mut self, name: &str, id: &str) -> Reply {
        let Ok(id) = id.trim_start_matches('#').parse::<u64>() else {
            return Reply::err("usage: agent-step wait <id>");
        };
        let at = self
            .agents_told
            .iter()
            .position(|t| t.id == id && t.agent == name);
        match at {
            Some(at) => {
                let told = self.agents_told.remove(at);
                match told.ran {
                    true => Reply::ok(format!("ran #{id}: {}", told.line)),
                    false => Reply::err(format!("not run #{id}: {}", told.line)),
                }
            }
            // Still on the card, or waiting for its place there.
            None if self.agent_asked(name, id) => Reply::ok(format!("waiting #{id}")),
            None => Reply::err(format!("unknown step #{id}")),
        }
    }

    /// Whether agent `name` asked step `id` and it waits for an answer.
    fn agent_asked(&self, name: &str, id: u64) -> bool {
        self.agent_admissions
            .iter()
            .any(|(entry, admission)| *entry == id && admission.caller().name().as_str() == name)
    }

    /// The card's answer to agent `agent`'s step `id`, kept for its wait.
    pub(crate) fn agent_told(&mut self, agent: String, id: u64, ran: bool, line: String) {
        self.close_agent_ask(id);
        if self.agents_told.len() >= MAX_TOLD {
            self.agents_told.remove(0);
        }
        self.agents_told.push(Told {
            agent,
            id,
            ran,
            line,
        });
    }

    /// The agents the shell knows, to install a packaged one into.
    pub fn roster_mut(&mut self) -> &mut agentid::Roster {
        &mut self.roster
    }
}
