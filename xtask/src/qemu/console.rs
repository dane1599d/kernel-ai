//! Reading the serial console: stamps, programs' lines, prompts, and the
//! lines that fail a case or warn of a slow runner.

/// Fails a case that printed a forbidden line ([`crate::cases::FORBID`]).
pub(super) fn forbidden_verdict(forbidden: Option<String>) -> Result<(), String> {
    match forbidden {
        Some(line) => Err(format!("FAIL: a forbidden line was on the console: {line}")),
        None => Ok(()),
    }
}

/// The kernel's boot line naming the guest's vector features and XCR0, which a
/// test repeats at its end.
pub(super) const CPU_LINE: &str = "cpu: vector features:";

/// A kernel line that a case passed with something a person should see, such
/// as `desk-perf` left ungated on a slow runner: xtask repeats its text as a
/// GitHub `::warning::`, which the run's summary shows.
pub(super) const CI_WARNING: &str = "ci-warning: ";

/// The text of a kernel's [`CI_WARNING`] line; a program's never counts.
pub(super) fn ci_warning(line: &str) -> Option<&str> {
    let line = line.trim_end_matches('\r');
    if from_programs(line) {
        return None;
    }
    line.strip_prefix(CI_WARNING)
        .filter(|text| !text.is_empty())
}

/// A serial line without the kernel's `[<s>.<ms> h<s>.<ms>] ` stamp (`stamp=ms`,
/// kernel/src/serial.rs), so a case booted with stamps matches as one without.
pub fn without_stamp(line: &str) -> &str {
    let Some((stamp, rest)) = line.strip_prefix('[').and_then(|l| l.split_once("] ")) else {
        return line;
    };
    let seconds = |text: &str| {
        text.split_once('.').is_some_and(|(s, ms)| {
            !s.is_empty()
                && ms.len() == 3
                && s.bytes().chain(ms.bytes()).all(|b| b.is_ascii_digit())
        })
    };
    match stamp.split_once(" h") {
        Some((clock, counter)) if seconds(clock) && seconds(counter) => rest,
        _ => line,
    }
}

/// Whether a console line was written for a ring-3 program rather than by the
/// kernel: `[name] ...` from a program's own output, `= ...`, `! ...` and `? ...` from
/// the shell's answers, possibly after the terminal's `kai> `, `edit <name>> `,
/// `run script N? y/n> ` and `apply <label> to <name>? yes/no> ` prompts, which
/// start the line the terminal logs on Enter. `screenlog`'s `screen: ` lines count
/// too: a screen row copies program text and kernel lines alike.
pub fn from_programs(line: &str) -> bool {
    if line.starts_with("screen: ") {
        return true;
    }
    let mut rest = line;
    while let Some(after) = rest
        .strip_prefix("kai> ")
        .or_else(|| strip_confirm_prompt(rest))
        .or_else(|| strip_edit_prompt(rest))
    {
        rest = after;
    }
    ["[", "= ", "! ", "? "]
        .iter()
        .any(|lead| rest.starts_with(lead))
}

fn is_word(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The text after a leading `edit <name>> ` or `apply <label> to <name>? yes/no> `.
fn strip_edit_prompt(line: &str) -> Option<&str> {
    if let Some(after) = line.strip_prefix("edit ") {
        let (name, rest) = after.split_once("> ")?;
        return is_word(name).then_some(rest);
    }
    let after = line.strip_prefix("apply ")?;
    let (label, after) = after.split_once(" to ")?;
    let (name, rest) = after.split_once("? yes/no> ")?;
    (is_word(label) && is_word(name)).then_some(rest)
}

/// The text after a leading `run script N? y/n> ` or `start trial N? yes/no> `
/// prompt.
fn strip_confirm_prompt(line: &str) -> Option<&str> {
    let after = line
        .strip_prefix("run script ")
        .or_else(|| line.strip_prefix("start trial "))?;
    let digits = after.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = &after[digits..];
    after
        .strip_prefix("? yes/no> ")
        .or_else(|| after.strip_prefix("? y/n> "))
}
