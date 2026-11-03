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
    "You write small window apps in Kai C for ",
    kabi::product_name!(),
    ", a small operating system.\n\
     Answer JSON only: {\"script\": \"...\", \"summary\": \"...\"}. script: the whole main.c. \
     summary: one sentence under 100 characters on what the app does.\n\
     Not an app request (a greeting, a question): empty script; the summary answers.\n\
     Fill in the template: TITLE, struct state, init, on_event and view. kaiapp.h and <string.h> are \
     included, so never declare their functions. I64 is 64-bit; whole numbers only.\n\
     At most 120 characters a line, at most 80 lines.\n\
     struct state holds plain values and fixed arrays, no pointers, at most 4096 bytes. init sets it; on_event changes it; \
     view draws the whole window from it every time.\n\
     A node id is a number from 1 to 128, each once. A parent is 0 (the window) or a ui_column or ui_row \
     made before it.\n\
     An event: e->node is the node. e->kind is KAI_PRESSED (a button), KAI_CHANGED (a field's new text \
     in e->text, a toggle's 1 or 0 in e->number) or KAI_SUBMITTED (Enter in a field).\n\
     Copy e->text before keeping it: Format(s->text, 64, \"%s\", e->text). Text from numbers: Format.\n\
     If an app cannot do it with these calls, make it show why.\n\
     Calls:\n",
);

/// The calls `view` and the app may make, as `kaiapp.h` declares them.
pub const CALLS: &[(&str, &str)] = &[
    (
        "void ui_column(I64 node, I64 parent);",
        "children top to bottom",
    ),
    (
        "void ui_row(I64 node, I64 parent);",
        "children left to right",
    ),
    (
        "void ui_label(I64 node, I64 parent, const char *text);",
        "text",
    ),
    (
        "void ui_heading(I64 node, I64 parent, const char *text);",
        "large text",
    ),
    (
        "void ui_button(I64 node, I64 parent, const char *text);",
        "KAI_PRESSED",
    ),
    (
        "void ui_field(I64 node, I64 parent, const char *label, const char *value);",
        "a text box: KAI_CHANGED, KAI_SUBMITTED",
    ),
    (
        "void ui_toggle(I64 node, I64 parent, const char *label, I64 on);",
        "KAI_CHANGED",
    ),
    (
        "I64 Format(char *out, I64 cap, const char *fmt, ...);",
        "printf into out: %lld, %s, %c",
    ),
];

/// The calls' names, for `precheck::prepare`: a variable may not take one.
pub const KAIAPP_CALLS: &[&str] = &[
    "ui_column",
    "ui_row",
    "ui_label",
    "ui_heading",
    "ui_button",
    "ui_field",
    "ui_toggle",
    "Format",
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
void init(struct state *s) { s->n = 0; s->draft[0] = 0; }
void on_event(struct state *s, const struct kai_event *e) {
    if (e->kind == KAI_CHANGED && e->node == 1) Format(s->draft, 64, \"%s\", e->text);
    if ((e->kind == KAI_PRESSED && e->node == 2) || e->kind == KAI_SUBMITTED) {
        if (s->n < 8 && s->draft[0]) { Format(s->items[s->n], 64, \"%s\", s->draft); s->n++; }
        s->draft[0] = 0;
    }
    if (e->kind == KAI_PRESSED && e->node == 3) s->n = 0;
}
void view(const struct state *s) {
    ui_field(1, 0, \"Item\", s->draft);
    ui_button(2, 0, \"Add\");
    ui_button(3, 0, \"Clear all\");
    for (I64 i = 0; i < s->n; i++) ui_label(10 + i, 0, s->items[i]);
}
",
    "A shopping list: type an item and add it, or clear them all.",
)];

/// The card.
pub fn render() -> String {
    let mut card = String::from(RULES);
    for (call, doc) in CALLS {
        let _ = writeln!(card, "{call} // {doc}");
    }
    let _ = write!(card, "Template:\n{TEMPLATE}");
    card.push_str("Example:\n");
    for (request, program, summary) in EXAMPLES {
        let envelope = serde_json::json!({ "script": program, "summary": summary });
        let _ = writeln!(card, "Request: {request}\n{envelope}");
    }
    card
}
