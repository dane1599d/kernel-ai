// SPDX-License-Identifier: 0BSD
// SPDX-FileCopyrightText: The Kernel AI contributors

//! `c-app-1` (plan "Kai builds apps", piece 2): the Kai C card for app
//! requests. The model fills in [`TEMPLATE`], a window app on `<kaiapp.h>`
//! (`ccserve/kai/kaiapp.h`), and answers in the script envelope: the whole
//! `main.c` as `script`, and the install card's line as `summary`. xtask's
//! test `cc build`s the template and every example, and holds each call
//! here to `kaiapp.h`'s, so what the model reads builds.

use alloc::string::String;
use core::fmt::Write;

/// The rules before the template.
const RULES: &str = concat!(
    "You write window apps in Kai C for ",
    kabi::product_name!(),
    ".\n\
     Answer JSON only: {\"script\": \"...\", \"summary\": \"...\"}. script: the whole main.c. \
     summary: under 100 characters on what the app does.\n\
     Fill in the template. Never declare what kaiapp.h or <string.h> declares. Whole numbers \
     only (I64). At most 80 lines of 120 characters.\n\
     struct state: plain values and fixed arrays, no pointers, at most 4096 bytes. init sets it, \
     on_event changes it, view draws the whole window from it.\n\
     Node ids are 1 to 128, each once; a parent is 0 (the window) or an earlier ui_column or ui_row.\n\
     e->kind is KAI_PRESSED (a button), KAI_CHANGED (a field's text in e->text, a toggle's 1 or 0 \
     in e->number) or KAI_SUBMITTED (Enter in a field).\n\
     To keep e->text, copy it with Format; Format also makes text from numbers.\n\
     If the calls cannot do it, make the app show why.\n\
     Calls:\n",
);

/// The calls `view` and the app may make, as `kaiapp.h` declares them.
pub const CALLS: &[(&str, &str)] = &[
    ("void ui_column(I64 node, I64 parent);", "top to bottom"),
    ("void ui_row(I64 node, I64 parent);", "left to right"),
    ("void ui_label(I64 node, I64 parent, const char *text);", ""),
    (
        "void ui_heading(I64 node, I64 parent, const char *text);",
        "",
    ),
    (
        "void ui_button(I64 node, I64 parent, const char *text);",
        "",
    ),
    (
        "void ui_field(I64 node, I64 parent, const char *label, const char *value);",
        "a text box",
    ),
    (
        "void ui_toggle(I64 node, I64 parent, const char *label, I64 on);",
        "",
    ),
    (
        "I64 Format(char *out, I64 cap, const char *fmt, ...);",
        "printf: %lld %s %c",
    ),
];

/// The calls' names and the runtime's `kai_state_size`, for
/// `precheck::prepare`: a variable may not take one.
pub const KAIAPP_CALLS: &[&str] = &[
    "ui_column",
    "ui_row",
    "ui_label",
    "ui_heading",
    "ui_button",
    "ui_field",
    "ui_toggle",
    "Format",
    "kai_state_size",
];

/// The template, filled in for a counter.
pub const TEMPLATE: &str = "#include <kaiapp.h>
const char TITLE[] = \"Counter\";
struct state { I64 count; };
void init(struct state *s) { s->count = 0; }
void on_event(struct state *s, const struct kai_event *e) {
    if (e->kind == KAI_PRESSED && e->node == 3) s->count++;
    if (e->kind == KAI_PRESSED && e->node == 4) s->count = 0;
}
void view(const struct state *s) {
    char text[32];
    Format(text, 32, \"Count: %lld\", s->count);
    ui_heading(1, 0, text);
    ui_row(2, 0);
    ui_button(3, 2, \"Add one\");
    ui_button(4, 2, \"Reset\");
}
";

/// (request, main.c, summary): a list kept from a field.
pub const EXAMPLES: &[(&str, &str, &str)] = &[(
    "make me a shopping list app",
    "#include <kaiapp.h>
const char TITLE[] = \"Shopping\";
struct state { char draft[64]; char items[8][64]; I64 n; };
void init(struct state *s) { s->n = 0; }
void on_event(struct state *s, const struct kai_event *e) {
    if (e->kind == KAI_CHANGED && e->node == 1) Format(s->draft, 64, \"%s\", e->text);
    if (e->kind == KAI_SUBMITTED && s->n < 8 && s->draft[0]) {
        Format(s->items[s->n], 64, \"%s\", s->draft);
        s->n++;
        s->draft[0] = 0;
    }
    if (e->kind == KAI_PRESSED && e->node == 2) s->n = 0;
}
void view(const struct state *s) {
    ui_field(1, 0, \"Item\", s->draft);
    ui_button(2, 0, \"Clear all\");
    for (I64 i = 0; i < s->n; i++) ui_label(10 + i, 0, s->items[i]);
}
",
    "A shopping list: type an item, press Enter, or clear them all.",
)];

/// Bytes a token for this card. C and its JSON escapes take more tokens a
/// byte than the dsl-1 pack's prose: the first card was 3,244 bytes in 1,088
/// real tokens, 2.98 a token (token-count run 41705507721).
const BYTES_PER_TOKEN: f32 = 2.8;

/// A token count of the card, at its measured rate, above the real count. The
/// token-count workflow checks the real count.
pub fn estimate_tokens(card: &str) -> usize {
    (card.len() as f32 / BYTES_PER_TOKEN) as usize + 1
}

/// The card.
pub fn render() -> String {
    let mut card = String::from(RULES);
    for (call, doc) in CALLS {
        let _ = match *doc {
            "" => writeln!(card, "{call}"),
            _ => writeln!(card, "{call} // {doc}"),
        };
    }
    let _ = write!(card, "Template:\n{TEMPLATE}");
    card.push_str("Example:\n");
    for (request, program, summary) in EXAMPLES {
        let envelope = serde_json::json!({ "script": program, "summary": summary });
        let _ = writeln!(card, "Request: {request}\n{envelope}");
    }
    card
}
