//! A request the model writes a script for, and code's spoken answer.

#[allow(unused_imports)]
use super::*;

impl Shell {
    /// Asks the LLM for a script for `request`, checks it, and proposes it. A refused
    /// answer goes back to the model once, with the reason.
    pub(crate) fn ask_for_script(&mut self, request: &str) -> Reply {
        if request.is_empty() {
            return Reply::err("usage: ? <what you want done>");
        }
        let id = self.next_id;
        self.next_id += 1;
        self.host.audit(&format!(
            "script {id} requested: {}",
            show::truncate(request, 60)
        ));
        // The time and the date come from the kernel's clock, never the model.
        if let Some(text) = clock::answer(request, self.host.unix_seconds()) {
            // Not `worded`: the panel shows this line itself (#893).
            self.record(id, request, "", "answered from the clock");
            return Reply::ok(text);
        }
        // Sound has one answer until there is an audio driver.
        if let Some(text) = sound::answer(request) {
            self.record(id, request, "", "answered: no sound yet");
            self.worded = true;
            return Reply::ok(text);
        }
        // What Kai can do with something comes from the rows, never the model.
        if let Some(text) = abilities::answer(request) {
            self.record(id, request, "", "answered from the registry");
            // A list in words: the panel sends it to the terminal whole.
            self.worded = true;
            return Reply::ok(text);
        }
        self.asked_model = true;
        if let Some(reply) = self.try_act(id, request) {
            return reply;
        }
        let mut repair: Option<(String, String)> = None;
        let mut outcome = String::new();
        for round in 0..2 {
            let ask = ScriptRequest {
                request: request.to_string(),
                repair: repair.take(),
                timeout_ms: SCRIPT_ROUND_MS,
                lang: llmproto::script::Lang::Rhai,
            };
            let failure = match self.host.script(&ask) {
                // No script: the model answered in words.
                Err(AnswerError::NoScript(reply)) if !reply.is_empty() => {
                    return self.say(id, request, reply);
                }
                Ok(answer) => match script::check(&answer, &self.host, &self.config) {
                    Ok(checked) => match guard::in_plan(&checked.plan, request) {
                        Some(why) => Some((answer.script, why)),
                        None => match (
                            self.told_only(request, &checked),
                            self.tier_of(request, &checked),
                        ) {
                            (Some(why), _) => Some((answer.script, why)),
                            (None, (Tier::Never, at)) => Some((
                                answer.script,
                                format!("{} may not be used in scripts here", at.join(", ")),
                            )),
                            (None, tier) => return self.propose(id, request, checked, tier),
                        },
                    },
                    Err(script::Refusal::Script(why)) => Some((answer.script, why)),
                    Err(refusal @ script::Refusal::Interrupted) => {
                        outcome = refusal.to_string();
                        None
                    }
                },
                Err(error) if error.repairable() => Some((
                    String::from("(the answer was refused before it was read)"),
                    error.describe(),
                )),
                Err(error) => {
                    outcome = error.describe();
                    None
                }
            };
            match failure {
                Some((script, why)) if round == 0 => repair = Some((script, why)),
                Some((_, why)) => outcome = format!("gave up after one repair: {why}"),
                None => break,
            }
        }
        self.record(id, request, "", &format!("refused: {outcome}"));
        self.host.audit(&format!(
            "script {id} refused: {}",
            show::truncate(&outcome, 120)
        ));
        Reply::err(outcome)
    }

    pub(crate) fn say(&mut self, id: u64, request: &str, reply: String) -> Reply {
        self.worded = true;
        self.record(id, request, "", "answered in words");
        Reply::ok(reply)
    }
}
