//! One line as the shell reads it: a command, code, an entry left open, or a
//! request for the model.

#[allow(unused_imports)]
use super::*;

impl Shell {
    pub(crate) fn eval_line(&mut self, input: &str) -> Reply {
        // An open Rhai entry takes the next line first, `:c` included.
        if let Some(entry) = self.partial.take() {
            return self.continue_entry(entry, input);
        }
        if let Some(reply) = self.c_line(input) {
            return reply;
        }
        let input = input.trim();
        self.last_output = None;
        if let Some(reply) = self.app_job_line(input) {
            return reply;
        }
        // `answer <id> <text>` comes from the terminal's confirm prompt. A line such as
        // `answer + 1` is Rhai about a variable, so the id must be a number.
        if let Some(rest) = input.strip_prefix("answer ")
            && rest
                .split(' ')
                .next()
                .is_some_and(|id| id.parse::<u64>().is_ok())
        {
            return self.answer(rest);
        }
        if let Some(reply) = self.answer_bare(input) {
            return reply;
        }
        // Anything but the answer leaves a proposal unrun, and an undo for later.
        self.tuner.discard();
        self.undo_asking = None;
        if let Some(pending) = self.pending.take() {
            let outcome = "discarded: another line came first";
            self.record(
                pending.id,
                &pending.request,
                &pending.checked.script,
                outcome,
            );
            // A desktop line's question on the card: where it was typed is told.
            self.desk_question_dropped(pending.id);
        }
        self.line.begin(self.host.as_ref(), &self.config);
        if input.is_empty() {
            return Reply::ok("");
        }
        // The answer to a clarifying question joins the request it clarifies; a typed
        // command, a Rhai line or a request of its own instead leaves it.
        if let Some(request) = self.clarifying.take()
            && reads_like_words(input)
            && split_command(input, |name| self.scope.contains(name)).is_none()
        {
            if clarify::answers(input) {
                return self.ask_for_script(&format!("{request} {input}"));
            }
            self.host.audit(&format!(
                "the question about \"{request}\" was left: \"{input}\" reads as a new request"
            ));
            let note = format!(
                "(Left the question about \"{}\".)",
                show::truncate(&request, LEFT_NOTE_CHARS)
            );
            let mut reply = self.eval_line(input);
            match reply.confirm.is_none() && !reply.more {
                true if reply.text.is_empty() => reply.text = note,
                true => reply.text = format!("{note}\n{}", reply.text),
                false => self.notices.push(note),
            }
            return reply;
        }
        if input == "reset" {
            self.scope.clear();
            self.functions = AST::empty();
            return Reply::ok("variables and functions cleared");
        }
        if !self.scope.contains(input) {
            match input {
                "undo" => return self.undo_command(""),
                "tune undo" => return self.tune_undo(),
                "new" => return self.forget(),
                "rules" => return Reply::ok(self.list_rules()),
                "profile" => return self.list_profiles(),
                "show" | "why" => return self.show_last(),
                _ => {}
            }
        }
        // `open <app>` alone opens it with no model (the Apps sheet's line).
        if let Some(reply) = self.open_line(input) {
            return reply;
        }
        // Installing Kai's last app, or showing its code, comes before a
        // package line of the same words.
        if !self.scope.contains("install")
            && !self.scope.contains("show")
            && let Some(reply) = self.app_install_line(input)
        {
            return reply;
        }
        // Package lines come before Rhai, unless their first word is a variable.
        if let Some(word) = pkg::line::first_word(input)
            && !self.scope.contains(word)
            && let Some(parsed) = pkg::line::parse(input)
        {
            return self.pkg_line(input, parsed);
        }
        // So do `models` lines: with no model yet, a person imports the first.
        if !self.scope.contains("models")
            && let Some(parsed) = models::parse_line(input)
        {
            return self.models_line(input, parsed);
        }
        // A restart is the person's own line, before Rhai and the model.
        if power::is_restart_line(input)
            && !self.scope.contains("restart")
            && !self.scope.contains("reboot")
        {
            return self.power_line(input);
        }
        if let Some(which) = input.strip_prefix("rules_remove ")
            && !self.scope.contains("rules_remove")
        {
            return match which.trim().starts_with('S') {
                true => self.remove_instruction(input, which.trim()),
                false => self.remove_rule(which),
            };
        }
        if let Some(text) = input.strip_prefix("rules_add ")
            && !self.scope.contains("rules_add")
        {
            return self.add_instruction(input, text);
        }
        if let Some(name) = input.strip_prefix("profile ")
            && !self.scope.contains("profile")
        {
            return self.profile_command(input, name);
        }
        if let Some(which) = input.strip_prefix("undo ")
            && !self.scope.contains("undo")
            && which.trim().trim_start_matches('#').parse::<u64>().is_ok()
        {
            return self.undo_command(which);
        }
        if (input == "tune" || input.starts_with("tune ")) && !self.scope.contains("tune") {
            let id = self.next_id;
            self.next_id += 1;
            return self.tuner.command(self.host.as_ref(), &input[4..], id);
        }
        if let Some(request) = input.strip_prefix('?') {
            return self.ask_for_script(request.trim());
        }
        if let Some((name, rest)) = split_command(input, |name| self.scope.contains(name)) {
            if name == "log" {
                return self.show_log(rest.trim());
            }
            return match commands::run(self.host.as_ref(), name, rest) {
                Ok(text) => Reply::ok(text),
                Err(error) => Reply::err(error),
            };
        }
        if brackets::open(input) {
            return self.gather(String::from(input));
        }
        self.run_code(input)
    }

    /// The next line of an entry left open: it is added, and the whole entry runs
    /// once everything it opened is closed.
    pub(crate) fn continue_entry(&mut self, mut entry: String, input: &str) -> Reply {
        self.last_output = None;
        if input == CANCEL_ENTRY {
            return Reply::ok("");
        }
        entry.push('\n');
        entry.push_str(input.trim_end());
        if entry.len() > MAX_ENTRY {
            return Reply::err(format!(
                "dropped: an entry may be at most {} KiB",
                MAX_ENTRY / 1024
            ));
        }
        if brackets::open(&entry) {
            return self.gather(entry);
        }
        self.line.begin(self.host.as_ref(), &self.config);
        self.run_code(&entry)
    }

    /// Keeps `entry` for the next line and asks the terminal for it.
    pub(crate) fn gather(&mut self, entry: String) -> Reply {
        self.partial = Some(entry);
        Reply::more()
    }

    /// Compiles and runs typed Rhai; plain words that do not run go to the LLM.
    pub(crate) fn run_code(&mut self, input: &str) -> Reply {
        let ast = match self.engine.compile_with_scope(&self.scope, input) {
            Ok(ast) => ast,
            Err(_) if reads_like_words(input) => return self.ask_for_script(input),
            Err(error) => return Reply::err(format!("syntax error: {error}")),
        };
        let program = self.functions.merge(&ast);
        let result = engine::eval(&self.engine, &mut self.scope, &program, self.config.exec);
        self.compact_scope();
        let mut text = self.line.take_output();
        match result {
            Ok(value) => {
                self.functions = self.functions.merge(&ast.clone_functions_only());
                if !value.is_unit() {
                    text.push_str(&show::value(&value));
                }
                Reply::ok(text)
            }
            // A word Rhai took for a variable, in a line of plain words: `hello`.
            // A one-letter or code-like name (`x`, `n2`, `max_len`) stays an error.
            Err(error)
                if text.is_empty()
                    && reads_like_words(input)
                    && matches!(
                        error.unwrap_inner(),
                        EvalAltResult::ErrorVariableNotFound(name, _) if is_word(name)
                    ) =>
            {
                self.ask_for_script(input)
            }
            Err(error) => {
                text.push_str(&self.describe(&error));
                Reply::err(text)
            }
        }
    }

    pub(crate) fn describe(&self, error: &EvalAltResult) -> String {
        self.line.describe(error, &self.config)
    }

    pub(crate) fn show_log(&self, which: &str) -> Reply {
        match which {
            "" => return Reply::ok(self.log.list()),
            "actlog" => return Reply::ok(self.log.text()),
            _ => {}
        }
        match which
            .trim_start_matches('#')
            .parse::<u64>()
            .ok()
            .and_then(|id| self.log.script(id))
        {
            Some(text) => Reply::ok(text),
            None => Reply::err(format!("no logged script {which}")),
        }
    }
}
