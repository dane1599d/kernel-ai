//! The row a stop, start or restart runs, found by its name (`targets`).

use super::{Outcome, outcome, text_arg};
use crate::targets::{self, Unresolved, Verb};
use crate::tiers::Tier;
use crate::{Reply, Shell};
use agentabi::Action;
use agentabi::consent::Why;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use llmproto::act::Call;

impl Shell {
    /// The row a stop, start or restart runs: code looks the name up among the
    /// supervisor's services and the running processes, whichever row the model
    /// picked, and asks when the name is both or neither. Any other row, a
    /// protected name, or a host that cannot list services keeps the model's row.
    /// With the row comes how the menu names the target ("Stop service indexer").
    pub(super) fn target(
        &mut self,
        id: u64,
        request: &str,
        row: &'static Action,
        call: &Call,
    ) -> Result<(&'static Action, Option<String>), Outcome> {
        let kept = Ok((row, None));
        let Some(verb) = Verb::of(row) else {
            return kept;
        };
        let name = text_arg(call, "name");
        if agentabi::PROTECTED.contains(&name.as_str()) {
            return kept;
        }
        let (Ok(services), Ok(metrics)) = (self.host.services(), self.host.metrics()) else {
            return kept;
        };
        let processes: Vec<(u64, String)> = metrics
            .processes
            .iter()
            .take(metrics.process_count as usize)
            .map(|p| (p.pid, p.name().to_string()))
            .collect();
        match targets::resolve(verb, &name, &services, &processes) {
            Ok(found) => {
                if found.row.name != row.name {
                    self.host.audit(&format!(
                        "request {id}: {} on {name} runs as {}",
                        row.name, found.row.name
                    ));
                }
                Ok((found.row, Some(found.question())))
            }
            Err(Unresolved::Clarify(question)) => {
                self.record(id, request, &call.text(), "asked the user to clarify");
                self.clarifying = Some(request.to_string());
                Err(outcome(Tier::Run, Why::Read, Reply::ok(question)))
            }
            Err(Unresolved::Say(text)) => {
                let reply = self.say(id, request, text);
                Err(outcome(Tier::Run, Why::Read, reply))
            }
        }
    }
}
