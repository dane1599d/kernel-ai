//! `cargo xtask tokens --gguf PATH`: spike S10-0 (ADR 0010), counting only, no
//! inference. With the vocabulary in a GGUF file (the whole file or its first
//! 32 MiB), it prints a Markdown table of the real token count of each part of
//! today's script prompt, of each script API card, of the chat template, and of
//! the `dsl-1` pack `llmproto::pack` renders from the registry, with bytes per
//! token against the pack's estimator, and the conversation-memory budget the
//! pack leaves (P1 plan section 4). It fails when the pack and MODE go over
//! `llmproto::pack::PACK_ALARM` real tokens (headroom under `PACK_BUDGET`) or
//! the estimate undercounts them.
//! The token-count workflow runs it on GitHub's runner.

use bpe::Tokenizer;
use bpe::chat::{Chat, Role, Turn};
use llmproto::script::{self, API};
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;

/// Bytes of the GGUF file read: the header and the whole vocabulary of a Qwen3
/// model fit in the first 8 MB, so this leaves room.
const HEAD_BYTES: u64 = 32 * 1024 * 1024;
use llmproto::pack::{self, Format, MODE_ACT, PACK_ALARM, PACK_BUDGET};
/// A plain request, as today's user turn carries it.
const REQUEST: &str = "switch to round robin";
/// ADR 0010 §4 and P1 plan §4.
const CONTEXT_CAP: usize = 3072;
const STATE_TOKENS: usize = 64;
const REQUEST_TOKENS: usize = 160;
const ANSWER_TOKENS: usize = 1024;
const HISTORY_MAX: usize = 512;

pub fn main(args: &[String]) -> Result<(), String> {
    let path = match args {
        [flag, path] if flag == "--gguf" => path,
        _ => return Err("usage: cargo xtask tokens --gguf PATH".into()),
    };
    let mut head = Vec::new();
    File::open(path)
        .and_then(|file| file.take(HEAD_BYTES).read_to_end(&mut head))
        .map_err(|error| format!("{path}: {error}"))?;
    let header = gguf::Header::parse(&head).map_err(|error| format!("{path}: {error:?}"))?;
    let tok = Tokenizer::from_gguf(&header).map_err(|error| format!("{path}: {error:?}"))?;
    let chat = Chat::new(&tok).map_err(|error| format!("{path}: {error:?}"))?;
    print!("{}", report(&tok, &chat));
    check_budget(&tok)
}

/// The pack and MODE within the alarm in real tokens, below the budget, and the
/// estimate the host test uses not under the real count.
fn check_budget(tok: &Tokenizer) -> Result<(), String> {
    let text = pack::render(Format::Dsl1);
    let real = count(tok, &text) + count(tok, MODE_ACT);
    let estimate = pack::estimate_tokens(&text) + pack::estimate_tokens(MODE_ACT);
    match (real <= PACK_ALARM, estimate >= real) {
        (true, true) => Ok(()),
        (false, _) => Err(format!(
            "the pack and MODE take {real} tokens, over the {PACK_ALARM} that keeps \
             headroom under {PACK_BUDGET}: put a group on the index level (ADR 0010 §6)"
        )),
        (true, false) => Err(format!(
            "the pack estimate ({estimate}) undercounts the real {real} tokens"
        )),
    }
}

/// Tokens in `text` on its own, as one segment.
fn count(tok: &Tokenizer, text: &str) -> usize {
    let mut ids = Vec::new();
    tok.encode(text, &mut ids);
    ids.len()
}

/// The text of each `## ` section of a pack, with its heading.
fn sections(pack: &str) -> Vec<(&str, &str)> {
    let starts: Vec<usize> = pack
        .match_indices("## ")
        .filter(|&(at, _)| at == 0 || pack.as_bytes()[at - 1] == b'\n')
        .map(|(at, _)| at)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(pack.len());
            let text = &pack[start..end];
            (text.lines().next().unwrap_or(""), text)
        })
        .collect()
}

struct Table {
    text: String,
    tok_bytes: (usize, usize),
}

impl Table {
    fn new(title: &str) -> Self {
        let mut text = format!(
            "\n### {title}\n\n| Part | Bytes | Tokens | Bytes per token |\n|---|---:|---:|---:|\n"
        );
        text.reserve(1024);
        Self {
            text,
            tok_bytes: (0, 0),
        }
    }

    fn row(&mut self, name: &str, bytes: usize, tokens: usize) {
        let ratio = bytes as f64 / tokens.max(1) as f64;
        let _ = writeln!(self.text, "| {name} | {bytes} | {tokens} | {ratio:.2} |");
    }

    /// A row with no text of its own, such as the template's tokens.
    fn tokens(&mut self, name: &str, tokens: usize) {
        let _ = writeln!(self.text, "| {name} | – | {tokens} | – |");
    }

    fn part(&mut self, tok: &Tokenizer, name: &str, text: &str) -> usize {
        let tokens = count(tok, text);
        self.row(name, text.len(), tokens);
        self.tok_bytes.0 += tokens;
        self.tok_bytes.1 += text.len();
        tokens
    }
}

/// The whole report, in Markdown.
pub fn report(tok: &Tokenizer, chat: &Chat) -> String {
    let mut out = String::from("## S10-0 token counts\n");
    let _ = writeln!(
        out,
        "\nVocabulary: {} tokens. Counts are of each part encoded on its own.",
        tok.len()
    );

    // Today's script prompt, by section.
    let system = script::system_prompt();
    let (rules, rest) = system.split_once("\nFunctions:\n").unwrap_or((&system, ""));
    let (cards, examples) = rest.split_once("\nExamples:\n").unwrap_or((rest, ""));
    let mut m8 = Table::new("Today's script system prompt (M8)");
    m8.part(tok, "rules", rules);
    m8.part(tok, "API cards (7 functions)", cards);
    m8.part(tok, "examples", examples);
    let whole = count(tok, &system);
    m8.row("**whole prompt**", system.len(), whole);
    out.push_str(&m8.text);

    let mut api = Table::new("Each script API card");
    for function in API {
        api.part(
            tok,
            &format!("`{}`", function.name),
            &format!("- {}: {}\n", function.signature(), function.doc),
        );
    }
    out.push_str(&api.text);

    let dsl1 = pack::render(Format::Dsl1);
    // The template: what the chat turns add around their text.
    let user = format!("Request: {REQUEST}\n/no_think");
    let one_turn = chat.prompt(tok, Some(&system), &user).len();
    let template = one_turn - whole - count(tok, &user);
    let pack_turns = [
        Turn::new(Role::System, &dsl1),
        Turn::new(Role::System, MODE_ACT),
        Turn::new(Role::User, &user),
    ];
    let three_turns = chat.conversation(tok, &pack_turns).len();
    let three_template = three_turns - count(tok, &dsl1) - count(tok, MODE_ACT) - count(tok, &user);
    let mut turns = Table::new("Chat template and turns");
    turns.tokens(
        "template, system + user (today, with the empty think block)",
        template,
    );
    turns.tokens(
        "template, PACK + MODE + user (ADR 0010 layout, with the empty think block)",
        three_template,
    );
    turns.part(
        tok,
        "user turn: `Request: switch to round robin` + `/no_think`",
        &user,
    );
    turns.part(tok, "`act` MODE", MODE_ACT);
    turns.tokens("**today's whole script request**", one_turn);
    out.push_str(&turns.text);

    // The pack, by section.
    let mut dsl = Table::new("`dsl-1` pack (llmproto::pack, golden llmproto/src/pack/dsl-1.txt)");
    for (heading, text) in sections(&dsl1) {
        dsl.part(tok, &format!("`{heading}`"), text);
    }
    let pack = count(tok, &dsl1);
    dsl.row("**whole pack**", dsl1.len(), pack);
    out.push_str(&dsl.text);
    let longest = dsl1.lines().map(|line| (count(tok, line), line)).max();
    if let Some((tokens, line)) = longest {
        let _ = writeln!(
            out,
            "\nLongest line: {tokens} tokens, {} bytes.",
            line.len()
        );
    }

    // What the pack leaves for conversation memory.
    let left = CONTEXT_CAP.saturating_sub(
        pack + three_template
            + STATE_TOKENS
            + REQUEST_TOKENS
            + ANSWER_TOKENS
            + count(tok, MODE_ACT),
    );
    let history = left.min(HISTORY_MAX);
    let _ = writeln!(
        out,
        "\n### Budgets\n\n- ADR 0010's core-plus-OS-rows bar is 1,024 tokens: the pack plus MODE takes {} (estimated {}).\n- History: {CONTEXT_CAP} − pack {pack} − template {three_template} − MODE {} − State {STATE_TOKENS} − request {REQUEST_TOKENS} − answer {ANSWER_TOKENS} leaves {left}, so `HISTORY_BUDGET` = min({HISTORY_MAX}, {left}) = **{history}**.\n- Bytes per token over every counted part: {:.2} (the pack alone: {:.2}).",
        pack + count(tok, MODE_ACT),
        pack::estimate_tokens(&dsl1) + pack::estimate_tokens(MODE_ACT),
        count(tok, MODE_ACT),
        (m8.tok_bytes.1 + dsl.tok_bytes.1) as f64
            / (m8.tok_bytes.0 + dsl.tok_bytes.0).max(1) as f64,
        dsl1.len() as f64 / pack.max(1) as f64,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pack_is_printable_ascii_in_lines_of_at_most_120() {
        for line in pack::render(Format::Dsl1).lines() {
            assert!(line.len() <= 120, "{line}");
            assert!(line.bytes().all(|b| (b' '..=b'~').contains(&b)), "{line}");
        }
    }

    #[test]
    fn the_pack_splits_into_its_sections_and_a_group_each() {
        let text = pack::render(Format::Dsl1);
        let headings: Vec<&str> = sections(&text).iter().map(|(h, _)| *h).collect();
        assert!(headings[0].starts_with("## Rhai"), "{headings:?}");
        assert!(headings[1].starts_with("## Calls"));
        assert_eq!(headings[2], "## Examples");
        // Then the OS rows, a section per group, then the index level's names.
        let (index, groups) = headings[3..].split_last().unwrap();
        // The file group is on the index level (ADR 0010 §6).
        assert_eq!(groups, ["## system"]);
        assert!(index.starts_with("## index"), "{index}");
        let joined: String = sections(&text).iter().map(|(_, t)| *t).collect();
        assert_eq!(joined, text);
    }

    /// Every OS function a script may call has a row in the pack.
    #[test]
    fn the_pack_has_a_row_for_every_api_function() {
        for function in API {
            assert!(
                pack::shows(&pack::render(Format::Dsl1), function.name),
                "{}",
                function.name
            );
        }
    }

    /// Opt-in, as bpe's real-vocabulary test: set KAI_QWEN3_GGUF to the model file
    /// (or its first 8 MB) and run with `--ignored`.
    #[test]
    #[ignore = "needs KAI_QWEN3_GGUF"]
    fn the_report_counts_with_the_real_vocabulary() {
        let path = std::env::var("KAI_QWEN3_GGUF").expect("KAI_QWEN3_GGUF");
        let bytes = std::fs::read(path).unwrap();
        let header = gguf::Header::parse(&bytes).unwrap();
        let tok = Tokenizer::from_gguf(&header).unwrap();
        let chat = Chat::new(&tok).unwrap();
        let report = report(&tok, &chat);
        assert!(report.contains("**whole pack**"), "{report}");
    }
}
