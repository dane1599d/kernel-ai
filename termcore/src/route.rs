//! Where a line typed at the terminal's prompt goes: one of the kernel's `:`
//! commands, the shell, or nowhere. The kernel's terminal decides with this,
//! so the rule is tested on the host.
//!
//! - `answer <id> …` is what only a question's own prompt sends, after the
//!   person at the keyboard answered it there. Typed or pasted at a prompt,
//!   at `kai>` or `c>` alike, it is refused: it would answer a question that
//!   was not shown.
//! - A bare `:c` or `:rhai` goes to the shell, and only the shell's answer
//!   to one of them sets the prompt (ADR 0012 D1). `:c <entry>`, and at `c>`
//!   the C session's `:dis`, `:defs` and `:reset`, go to the shell too, but
//!   their output cannot change the prompt. Every other `:` line is the
//!   kernel's.

/// Where a typed line goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// An empty line.
    Nothing,
    /// A kernel command: the text after the `:`.
    Kernel,
    /// A bare `:c` or `:rhai`: to the shell, whose answer decides the prompt.
    Mode,
    /// Another `:` line for the shell: `:c <entry>`, and at `c>` `:dis`,
    /// `:defs` and `:reset`.
    ShellCommand,
    /// `answer <id> …`, which only a question's prompt may send.
    Refused,
    /// To the shell (or to the editor, while one is open).
    Shell,
}

/// Where `line` (trimmed) goes; `c_mode` is whether the prompt is `c> `.
pub fn route(line: &str, c_mode: bool) -> Route {
    match line.strip_prefix(':') {
        Some("c" | "rhai") => Route::Mode,
        Some(command) if shell_command(command.trim(), c_mode) => Route::ShellCommand,
        Some(_) => Route::Kernel,
        None if line.is_empty() => Route::Nothing,
        None if is_answer(line) => Route::Refused,
        None => Route::Shell,
    }
}

/// Whether a line looks like an answer to a question, `answer <id> ...`,
/// which only the question's own prompt may send.
pub fn is_answer(line: &str) -> bool {
    line.strip_prefix("answer ")
        .and_then(|rest| rest.split(' ').next())
        .is_some_and(|id| id.parse::<u64>().is_ok())
}

/// Whether the shell answers `:command` rather than the kernel.
fn shell_command(command: &str, c_mode: bool) -> bool {
    match command.split_whitespace().next().unwrap_or("") {
        "c" => true,
        "dis" | "defs" | "reset" => c_mode,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Route, route};

    #[test]
    fn an_answer_typed_at_any_prompt_is_refused() {
        for c_mode in [false, true] {
            for line in [
                "answer 7 yes",
                "answer 7 y",
                "answer 12 allow once=0123456789abcdef",
                "answer 0 no",
            ] {
                assert_eq!(route(line, c_mode), Route::Refused, "{line} at c> {c_mode}");
            }
        }
        // Not an answer: C or Rhai, for the shell.
        for line in [
            "n",
            "y",
            "yes",
            "answer",
            "answer x yes",
            "answers 7",
            "I64 answer = 7;",
        ] {
            assert_eq!(route(line, true), Route::Shell, "{line}");
        }
    }

    #[test]
    fn mode_lines_go_to_the_shell_and_the_rest_to_the_kernel() {
        for c_mode in [false, true] {
            assert_eq!(route(":c", c_mode), Route::Mode);
            assert_eq!(route(":rhai", c_mode), Route::Mode);
            // One entry: its output must not be read as a mode change.
            assert_eq!(route(":c I64 x = 1;", c_mode), Route::ShellCommand);
            assert_eq!(route(":c Print(\"C mode\");", c_mode), Route::ShellCommand);
            assert_eq!(route(":rhai now", c_mode), Route::Kernel);
            assert_eq!(route(":ps", c_mode), Route::Kernel);
            assert_eq!(route(":cpus", c_mode), Route::Kernel);
            assert_eq!(route("", c_mode), Route::Nothing);
        }
        // The C session's commands are the shell's only at c>.
        for line in [":dis Sq", ":defs", ":reset"] {
            assert_eq!(route(line, true), Route::ShellCommand, "{line}");
            assert_eq!(route(line, false), Route::Kernel, "{line}");
        }
    }
}
