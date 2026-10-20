//! The `local-ask` boot case's answer, worked out on the host: the generated model
//! `infer::fixture::local_ask` asked `hi` through the same engine (`infer::chat`),
//! context cap, memory budget and answer check (`llmproto::chat::answer`) the llm
//! service runs in the guest. Every engine tier must give it, so it does not matter
//! which one the guest's CPU picks.

use infer::Next;
use infer::chat::{Budget, Engine};
use llmproto::chat::{self, Completion};
use quant::Tier;

/// What the harness types after `ask`.
pub const QUESTION: &str = "hi";
/// Longest piece of a line the terminal prints in one go (`terminal::PIECE_CHARS`).
const PIECE_CHARS: usize = 160;

/// The answer the guest must show for `ask hi` on the model in `bytes`.
pub fn golden(bytes: &[u8]) -> Result<String, String> {
    let request = chat::ask(QUESTION);
    let (system, user) = request.single_turn().ok_or("ask is not one turn")?;
    let budget = Budget {
        kv_bytes: kabi::model::KV_BYTES as usize,
        other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize,
    };
    let mut answers = Vec::new();
    for tier in Tier::all() {
        let mut engine = Engine::load(bytes, tier, kabi::model::CONTEXT_CAP, budget)
            .map_err(|e| format!("the local-ask model: {e}"))?;
        let reply = engine
            .answer(system, user, request.max_tokens as usize, |_| {
                Next::Continue
            })
            .map_err(|e| format!("the local-ask model: {e}"))?;
        let completion = Completion {
            text: reply.text(),
            cut_off: reply.cut_off(),
        };
        answers.push((tier.name(), chat::answer(&completion)?));
    }
    let (_, first) = answers.first().ok_or("no engine tier")?.clone();
    if let Some((tier, other)) = answers.iter().find(|(_, answer)| *answer != first) {
        return Err(format!("the {tier} tier answers {other:?}, not {first:?}"));
    }
    Ok(first)
}

/// The console lines the terminal prints for the shell's answer `text`: each line
/// in pieces of at most 160 characters, after the shell's `= ` lead.
pub fn terminal_lines(text: &str) -> Vec<String> {
    shown_lines("=", text)
}

/// The console lines the terminal prints for a reply `text` after `lead`.
pub fn shown_lines(lead: &str, text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            lines.push(format!("{lead} "));
        }
        for piece in chars.chunks(PIECE_CHARS) {
            lines.push(format!("{lead} {}", piece.iter().collect::<String>()));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_golden_answer_is_short_plain_and_the_same_every_time() {
        let bytes = infer::fixture::local_ask();
        let golden = golden(&bytes).unwrap();
        assert_eq!(golden, super::golden(&infer::fixture::local_ask()).unwrap());
        assert!(golden.ends_with(chat::CUT_OFF), "{golden:?}");
        let words = golden.strip_suffix(chat::CUT_OFF).unwrap();
        assert!(
            words
                .bytes()
                .all(|b| b == b' ' || b.is_ascii_alphanumeric()),
            "{golden:?}"
        );
        assert!(words.split(' ').count() >= 5, "{golden:?}");
        // One console line, so the terminal shows it whole.
        assert_eq!(terminal_lines(&golden), [format!("= {golden}")]);
        println!("golden: {golden}");
    }

    #[test]
    fn long_answers_are_split_as_the_terminal_splits_them() {
        let long = format!("{}\n\nend", "x".repeat(170));
        assert_eq!(
            terminal_lines(&long),
            [
                format!("= {}", "x".repeat(160)),
                format!("= {}", "x".repeat(10)),
                String::from("= "),
                String::from("= end"),
            ]
        );
    }
}
