//! `cargo xtask tokens --gguf PATH`: spike S10-0 (ADR 0010), counting only, no
//! inference. With the vocabulary in a GGUF file (the whole file or its first
//! 32 MiB), it prints a Markdown table of the real token count of each part of
//! today's script prompt, of each script API card, of the chat template, and of
//! the `dsl-1` pack `llmproto::pack` renders from the registry, with bytes per
//! token against the pack's estimator, and the conversation-memory budget the
//! pack leaves (P1 plan section 4). It fails when the pack and MODE go over
//! `llmproto::pack::PACK_ALARM` real tokens (headroom under `PACK_BUDGET`) or
//! the estimate undercounts them. It also counts S05-2's agent views (ADR 0005):
//! each P4 app's mock window as `ui_view` shows it, which fails past
//! `deskcore::view::MAX_TOKENS` or when the view's estimate undercounts it.
//! The `c-app-1` card is held to the same alarm on its own. For the trims the
//! agent rows need (plan agents-os §4, T0), it also counts each line of the
//! pack's sections on its own (an example's request and answer together),
//! largest first, with the headroom left under the alarm: measurement only.
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
    print!("{}", views(&tok));
    check_budget(&tok)?;
    check_app_card(&tok)?;
    check_views(&tok)
}

/// S05-2's views: each P4 app's mock window, with the other apps' windows in
/// the list, as `ui_view` would answer.
fn mock_views() -> Vec<(&'static str, String)> {
    use deskcore::view::{self, Listed, mock};
    let apps = mock::apps();
    let windows: Vec<Listed> = apps
        .iter()
        .enumerate()
        .map(|(n, (app, title, _))| Listed {
            id: deskcore::desk::WindowId(n as u32 + 1),
            app: String::from(*app),
            title: String::from(*title),
            focused: n == 0,
        })
        .collect();
    apps.iter()
        .zip(&windows)
        .map(|((app, _, tree), w)| (*app, view::project(&windows, w.id, tree).text))
        .collect()
}

/// S05-2's views of each shipped app's real window, as each `-ui` crate's host
/// test commits them (`tests/golden/view-*.txt`, made by `testkit/appview.rs`:
/// the window's real frame, `wasmhost::surface::tree_update`, then
/// `deskcore::view::project`), so the real tokenizer counts what the agent is
/// shown and not only the mock trees.
fn real_views() -> Vec<(&'static str, &'static str)> {
    macro_rules! golden {
        ($name:literal, $path:literal) => {
            ($name, include_str!(concat!("../../", $path)).trim_end())
        };
    }
    vec![
        golden!("notes", "components/notes-ui/tests/golden/view-notes.txt"),
        golden!("files", "apps/files-ui/tests/golden/view-files.txt"),
        golden!("monitor", "apps/monitor-ui/tests/golden/view-monitor.txt"),
        golden!("models", "apps/models-ui/tests/golden/view-models.txt"),
        golden!(
            "terminal",
            "apps/terminal-ui/tests/golden/view-terminal.txt"
        ),
        golden!("editor", "apps/editor-ui/tests/golden/view-editor.txt"),
        golden!(
            "settings, About",
            "apps/settings-ui/tests/golden/view-settings-about.txt"
        ),
        golden!(
            "settings, Appearance",
            "apps/settings-ui/tests/golden/view-settings-appearance.txt"
        ),
        golden!(
            "settings, Keep running",
            "apps/settings-ui/tests/golden/view-settings-keep-running.txt"
        ),
        golden!(
            "settings, Network",
            "apps/settings-ui/tests/golden/view-settings-network.txt"
        ),
        golden!(
            "settings, Rules and profiles",
            "apps/settings-ui/tests/golden/view-settings-rules-and-profiles.txt"
        ),
        golden!(
            "settings, Updates",
            "apps/settings-ui/tests/golden/view-settings-updates.txt"
        ),
        golden!(
            "settings, What apps can do",
            "apps/settings-ui/tests/golden/view-settings-what-apps-can-do.txt"
        ),
    ]
}

/// S05-2's table (ADR 0005, "Spikes that settle it").
fn views(tok: &Tokenizer) -> String {
    use deskcore::view::{MAX_TOKENS, estimate_tokens};
    let mut table = Table::new(&format!(
        "S05-2: each P4 app's agent view (at most {MAX_TOKENS} tokens)"
    ));
    for (app, text) in mock_views() {
        table.part(
            tok,
            &format!("`{app}` (estimated {})", estimate_tokens(&text)),
            &text,
        );
    }
    for (app, text) in real_views() {
        table.part(
            tok,
            &format!(
                "`{app}`, the real window (estimated {})",
                estimate_tokens(text)
            ),
            text,
        );
    }
    table.text
}

/// Each view within its budget in real tokens, and its estimate not under them.
fn check_views(tok: &Tokenizer) -> Result<(), String> {
    use deskcore::view::{MAX_TOKENS, estimate_tokens};
    for (app, text) in mock_views() {
        let real = count(tok, &text);
        if real > MAX_TOKENS {
            return Err(format!(
                "S05-2: {app}'s view takes {real} tokens, over {MAX_TOKENS}: shorten the                  projection (drop static text, shorten roles) before its format freezes"
            ));
        }
        if estimate_tokens(&text) < real {
            return Err(format!(
                "S05-2: {app}'s view estimate ({}) undercounts the real {real} tokens",
                estimate_tokens(&text)
            ));
        }
    }
    for (app, text) in real_views() {
        let real = count(tok, text);
        if real > MAX_TOKENS {
            return Err(format!(
                "S05-2: the real {app} window's view takes {real} tokens, over {MAX_TOKENS}:                  shorten the projection before its format freezes"
            ));
        }
        if estimate_tokens(text) < real {
            return Err(format!(
                "S05-2: the real {app} window's view estimate ({}) undercounts the real {real} tokens",
                estimate_tokens(text)
            ));
        }
        if text.contains(" more") {
            return Err(format!(
                "S05-2: the real {app} window's view is cut: {text}"
            ));
        }
    }
    Ok(())
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

/// The `c-app-1` card on its own within the alarm in real tokens (plan "Kai
/// builds apps", piece 2), and its estimate, which the host test uses, not
/// under them.
fn check_app_card(tok: &Tokenizer) -> Result<(), String> {
    let card = pack::capp1::render();
    let real = count(tok, &card);
    let estimate = pack::capp1::estimate_tokens(&card);
    match (real <= PACK_ALARM, estimate >= real) {
        (true, true) => Ok(()),
        (false, _) => Err(format!(
            "the c-app-1 card takes {real} tokens, over the {PACK_ALARM} bar"
        )),
        (true, false) => Err(format!(
            "the c-app-1 estimate ({estimate}) undercounts the real {real} tokens"
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

    // Spike S12-2: the Kai C card for program requests, by section.
    let c1 = pack::render(Format::C1);
    let (c_rules, c_rest) = c1.split_once("\nFunctions:\n").unwrap_or((&c1, ""));
    let (c_cards, c_examples) = c_rest.split_once("\nExamples:\n").unwrap_or((c_rest, ""));
    let mut card = Table::new("`c-1` Kai C card (program requests only, spike S12-2)");
    card.part(tok, "rules", c_rules);
    card.part(tok, "prototypes (7 rows, as kai.h declares them)", c_cards);
    card.part(tok, "examples", c_examples);
    card.row("**whole card**", c1.len(), count(tok, &c1));
    out.push_str(&card.text);

    // Plan "Kai builds apps", piece 2: the app card, by section.
    let app = pack::capp1::render();
    let (a_rules, a_rest) = app.split_once("\nCalls:\n").unwrap_or((&app, ""));
    let (a_calls, a_rest) = a_rest.split_once("\nTemplate:\n").unwrap_or((a_rest, ""));
    let (a_template, a_example) = a_rest.split_once("\nExample:\n").unwrap_or((a_rest, ""));
    let mut app_card = Table::new("`c-app-1` Kai C app card (app requests only)");
    app_card.part(tok, "rules", a_rules);
    app_card.part(tok, "calls (kaiapp.h)", a_calls);
    app_card.part(tok, "template", a_template);
    app_card.part(tok, "example", a_example);
    app_card.row("**whole card**", app.len(), count(tok, &app));
    out.push_str(&app_card.text);

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
    out.push_str(&trim_table(tok, &dsl1));
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
    let used = pack + count(tok, MODE_ACT);
    let _ = writeln!(
        out,
        "- Headroom under the alarm: {PACK_ALARM} − {used} = **{}** tokens (under `PACK_BUDGET` {PACK_BUDGET}: {}).",
        PACK_ALARM as i64 - used as i64,
        PACK_BUDGET as i64 - used as i64,
    );
    out
}

/// Each line of the pack's sections as a trim would take it, with its section's
/// heading: an example's request and its answer are one item, and a heading is
/// none. Every other non-empty line is one item, in the pack's order.
fn trim_items(pack: &str) -> Vec<(&str, String)> {
    let mut items = Vec::new();
    for (heading, text) in sections(pack) {
        let mut lines = text.lines().skip(1).filter(|line| !line.is_empty());
        while let Some(line) = lines.next() {
            let item = match (heading == "## Examples", line.starts_with("> ")) {
                (true, true) => format!("{line}\n{}", lines.next().unwrap_or("")),
                _ => line.to_string(),
            };
            items.push((heading, item));
        }
    }
    items
}

/// The trims' table: every item of [`trim_items`] with its real tokens, the
/// largest first (plan agents-os §4, T0).
fn trim_table(tok: &Tokenizer, pack: &str) -> String {
    let mut counted: Vec<(usize, &str, String)> = trim_items(pack)
        .into_iter()
        .map(|(heading, item)| (count(tok, &item), heading, item))
        .collect();
    counted.sort_by_key(|item| core::cmp::Reverse(item.0));
    let mut table = Table::new(
        "`dsl-1` pack lines, largest first (for trims; an example counts its request and answer)",
    );
    for (tokens, heading, item) in counted {
        let shown: String = item.replace('\n', " / ").chars().take(60).collect();
        let shown = shown.replace('|', "\\|").replace('`', "'");
        table.row(
            &format!("{} `{shown}`", heading.trim_start_matches("## ")),
            item.len(),
            tokens,
        );
    }
    table.text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed real views are whole, within the budget by the host's
    /// estimate, and start with the window list, as `ui_view` answers.
    #[test]
    fn the_real_app_views_are_whole_and_within_the_estimate() {
        use deskcore::view::{MAX_TOKENS, estimate_tokens};
        let views = real_views();
        assert_eq!(views.len(), 13);
        for (app, text) in views {
            assert!(estimate_tokens(text) <= MAX_TOKENS, "{app}: {text}");
            assert!(!text.contains(" more"), "{app} is cut: {text}");
            assert!(text.starts_with("windows: [w1]"), "{app}: {text}");
        }
    }

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

    /// Every line of the pack is in exactly one trim item, in order, and an
    /// example's request goes with its answer.
    #[test]
    fn the_trim_items_cover_every_line_of_the_pack_once() {
        let text = pack::render(Format::Dsl1);
        let items = trim_items(&text);
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with("## "))
            .collect();
        let rejoined: Vec<String> = items
            .iter()
            .flat_map(|(_, item)| item.split('\n').map(str::to_string).collect::<Vec<_>>())
            .collect();
        assert_eq!(rejoined, lines);
        let examples: Vec<&String> = items
            .iter()
            .filter(|(heading, _)| *heading == "## Examples")
            .map(|(_, item)| item)
            .collect();
        assert!(!examples.is_empty());
        for example in examples {
            let (request, answer) = example.split_once('\n').expect("a request and its answer");
            assert!(
                request.starts_with("> ") && answer.starts_with('{'),
                "{example}"
            );
        }
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
        println!("{}", views(&tok));
        check_views(&tok).unwrap();
    }
}
