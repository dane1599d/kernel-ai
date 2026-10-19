//! Judging a run: whether a missing script was a fair refusal, and whether the
//! output and policy changes are what the task asked for.

use super::Failure;
use super::tasks::{Policy, REFUSAL_WORDS, Task};
use llmproto::script::{Answer, AnswerError};

/// No script was proposed. That fails, except for an impossible request the model
/// declined to write a script for.
pub(super) fn no_proposal(
    task: &Task,
    last: Option<&Result<Answer, AnswerError>>,
    text: &str,
) -> Option<(Failure, String)> {
    let failure = match last {
        Some(Err(AnswerError::NoScript(_))) if task.refusal => return None,
        Some(Err(AnswerError::NoScript(_))) => Failure::NoScript,
        Some(Err(AnswerError::Server(_) | AnswerError::CutOff)) | None => Failure::Server,
        Some(_) => Failure::Refused,
    };
    Some((failure, text.to_string()))
}

/// Whether a run that printed `output` and made `changes` did what `task` asks.
pub(super) fn judge(
    task: &Task,
    ok: bool,
    output: &str,
    changes: &[u64],
) -> Option<(Failure, String)> {
    if !ok {
        return Some((Failure::RunError, output.to_string()));
    }
    if task.refusal {
        if !changes.is_empty() {
            return Some((Failure::WrongChange, format!("policy changes {changes:?}")));
        }
        if !says_refusal(output) {
            return Some((Failure::NotRefused, output.to_string()));
        }
        return None;
    }
    if let Some(missing) = task
        .prints
        .iter()
        .find(|piece| !contains_piece(output, piece))
    {
        return Some((
            Failure::WrongOutput,
            format!("no {missing:?} in {output:?}"),
        ));
    }
    let all_to = |name| changes.iter().all(|&id| id == policy_id(name));
    let right = match task.policy {
        Policy::Unchanged => changes.is_empty(),
        Policy::Set(name) => !changes.is_empty() && all_to(name),
        Policy::SetOrKeep(name) => all_to(name),
    };
    (!right).then(|| {
        (
            Failure::WrongChange,
            format!("policy changes {changes:?}, wanted {:?}", task.policy),
        )
    })
}

/// Whether `output` says the request cannot be done: it holds one of
/// [`REFUSAL_WORDS`] as whole words, with at most one other word inside the phrase.
fn says_refusal(output: &str) -> bool {
    let words = words_of(output);
    REFUSAL_WORDS.iter().any(|phrase| {
        let phrase = words_of(phrase);
        (0..words.len()).any(|start| matches_from(&words[start..], &phrase, 1))
    })
}

/// Whether `words` starts with `phrase`, skipping at most `gaps` extra words
/// after its first word.
fn matches_from(words: &[String], phrase: &[String], gaps: usize) -> bool {
    match (words.first(), phrase.first()) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(word), Some(first)) if word == first => {
            let rest = &phrase[1..];
            matches_from(&words[1..], rest, gaps)
                || (gaps > 0
                    && !rest.is_empty()
                    && words.len() > 1
                    && matches_from(&words[2..], rest, gaps - 1))
        }
        _ => false,
    }
}

/// Lower-case words, apostrophes kept so "can't" is one word.
fn words_of(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '\''))
        .filter(|word| !word.is_empty())
        .map(String::from)
        .collect()
}

fn policy_id(name: &str) -> u64 {
    if name == "round-robin" {
        kabi::policy::ROUND_ROBIN
    } else {
        kabi::policy::INTERACTIVE
    }
}

/// Whether `output` contains `piece`. A piece that starts or ends with a digit must
/// not run on into more of a number there: "4" is not in "4321" or "4.321", though
/// it is in "4 s" and in "it is 4.". A piece without capitals matches in any case.
pub(super) fn contains_piece(output: &str, piece: &str) -> bool {
    let haystack = if piece.chars().any(|c| c.is_ascii_uppercase()) {
        output.to_string()
    } else {
        output.to_ascii_lowercase()
    };
    let bytes = haystack.as_bytes();
    let digit = |index: usize| bytes.get(index).is_some_and(u8::is_ascii_digit);
    let point = |index: usize| bytes.get(index) == Some(&b'.');
    // A digit, or a decimal point with a digit beyond it, carries a number on.
    let runs_on_after = |end: usize| digit(end) || (point(end) && digit(end + 1));
    let runs_on_before = |start: usize| {
        start > 0 && (digit(start - 1) || (start > 1 && point(start - 1) && digit(start - 2)))
    };
    let starts_digit = piece.starts_with(|c: char| c.is_ascii_digit());
    let ends_digit = piece.ends_with(|c: char| c.is_ascii_digit());
    haystack.match_indices(piece).any(|(start, _)| {
        !(starts_digit && runs_on_before(start))
            && !(ends_digit && runs_on_after(start + piece.len()))
    })
}
