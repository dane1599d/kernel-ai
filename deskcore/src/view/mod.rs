//! The agent's view of the desktop (ADR 0005 §6): the window list, then one
//! window's interactive and labelled nodes, with short ids, role, label and
//! value, at most [`MAX_TOKENS`] tokens, ending `+N more` past that:
//!
//! ```text
//! windows: [w1] notes (focused)  [w2] terminal
//! w1: [t1] textbox "Search"  [b2] button "New note"  [l3] list "Notes" 12 items, selected "Groceries"
//! ```
//!
//! `ui_view` answers with it as `Data` (ADR 0002 D6): nothing an app puts in its
//! tree can start an action or lower a tier. Names and values are quoted, with
//! `"`, `\` and line breaks escaped, so an app's text cannot leave its quotes.
//! The short ids name the nodes `ui_act` acts on, and [`View::node`] turns one
//! back into the tree's id; `desktop` keeps each window's last view, so an id
//! means what the model read.
//!
//! What keeps a view small (S05-2's fail rule, applied up front): containers
//! with no action of their own are left out, a list shows its count, its
//! selected item and its first [`MAX_ITEMS`] items, a list's items and a
//! table's rows drop their role word (the id's letter says it), a row is its
//! first cell, and long names and values are cut. Only if that is still too
//! long does static text go, and then the nodes past the budget become
//! `+N more`.
//!
//! The count is an estimate at [`BYTES_PER_TOKEN`], below the real rate for
//! this text; `cargo xtask tokens` checks it with the GGUF vocabulary on S05-2's
//! mock trees ([`mock`]).

#[cfg(any(test, feature = "mock"))]
pub mod mock;
#[cfg(test)]
mod tests;

use crate::desk::{Desk, Focus, WindowId};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use surfabi::tree::{Node, Role, Tree, flag};

/// The most a view may take, in tokens (ADR 0005 §6).
pub const MAX_TOKENS: usize = 300;
/// The most the window list may take of that.
pub const MAX_LIST_TOKENS: usize = 60;
/// Bytes per token for [`estimate_tokens`]: under the real rate for views, so
/// the estimate never undercounts. Views are dense (ids, digits, quotes): S05-2's
/// mock views took 2.15 to 2.65 bytes a token with Qwen3's vocabulary, and
/// `cargo xtask tokens` checks it on every change.
pub const BYTES_PER_TOKEN: f32 = 2.0;
/// Items, rows, tabs or menu entries shown of one container.
pub const MAX_ITEMS: usize = 4;
/// Characters of a name shown, and of a value.
pub const MAX_NAME: usize = 32;
pub const MAX_VALUE: usize = 48;
/// Between entries, as ADR 0005 §6 writes them.
const GAP: &str = "  ";

/// A token count of view text, at [`BYTES_PER_TOKEN`], at or above the real one.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() as f32 / BYTES_PER_TOKEN) as usize + 1
}

/// One open window as `ui_windows` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: WindowId,
    /// The app it belongs to (`toplevel.set_app_id`), or the process label.
    pub app: String,
    pub title: String,
    pub focused: bool,
}

/// The open windows, front to back; minimized ones last.
pub fn listed(desk: &Desk) -> Vec<Listed> {
    let focused = match desk.focus() {
        Focus::Window(id) => Some(id),
        _ => None,
    };
    let mut out: Vec<Listed> = desk
        .windows()
        .iter()
        .rev()
        .map(|w| Listed {
            id: w.id,
            app: match w.app_id.is_empty() {
                true => w.label.clone(),
                false => w.app_id.clone(),
            },
            title: w.title.clone(),
            focused: focused == Some(w.id),
        })
        .collect();
    let minimized = |l: &Listed| desk.window(l.id).is_some_and(|w| w.minimized);
    out.sort_by_key(minimized);
    out
}

/// `w3`, a window's short id.
pub fn window_id(id: WindowId) -> String {
    format!("w{}", id.0)
}

/// A window's short id read back.
pub fn parse_window(text: &str) -> Option<WindowId> {
    let n = text.strip_prefix('w')?;
    if n.is_empty() || n.starts_with('0') || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok().map(WindowId)
}

/// One window's view, and the nodes its short ids name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub text: String,
    ids: Vec<(String, u32)>,
}

impl View {
    /// The tree's id of the node shown as `short`, such as `b2`.
    pub fn node(&self, short: &str) -> Option<u32> {
        self.ids.iter().find(|(s, _)| s == short).map(|&(_, id)| id)
    }
}

/// The view of `window`, whose tree is `tree`, under the list of `windows`.
pub fn project(windows: &[Listed], window: WindowId, tree: &Tree) -> View {
    let list = windows_line(windows);
    let head = format!("{}:", window_id(window));
    let room = MAX_TOKENS.saturating_sub(estimate_tokens(&list) + estimate_tokens(&head));
    let all = entries(tree, true);
    let entries = match fits(&all, room) {
        true => all,
        false => entries(tree, false),
    };
    let mut text = format!("{list}\n{head}");
    let mut ids = Vec::new();
    let mut used = 0;
    for (n, entry) in entries.iter().enumerate() {
        let line = entry.line();
        let left = entries.len() - n - 1;
        let reserve = if left > 0 { MORE_TOKENS } else { 0 };
        let cost = estimate_tokens(&line) + 1;
        if used + cost + reserve > room {
            text.push_str(&format!(" +{} more", entries.len() - n));
            break;
        }
        used += cost;
        text.push_str(if n == 0 { " " } else { GAP });
        text.push_str(&line);
        if let Some(short) = &entry.short {
            ids.push((short.clone(), entry.node));
        }
    }
    View { text, ids }
}

/// A tree's nodes by id.
type Nodes<'a> = BTreeMap<u32, &'a Node>;

/// What a trailing `+N more` costs, at most.
const MORE_TOKENS: usize = 4;

/// `windows: [w1] notes (focused)  [w2] terminal "kai>"`, cut to
/// [`MAX_LIST_TOKENS`] with `+N more`.
pub fn windows_line(windows: &[Listed]) -> String {
    let mut line = String::from("windows:");
    if windows.is_empty() {
        line.push_str(" none");
    }
    for (n, w) in windows.iter().enumerate() {
        let mut entry = format!("[{}] {}", window_id(w.id), bare(&w.app));
        let title = w.title.trim();
        if !title.is_empty() && !title.eq_ignore_ascii_case(w.app.trim()) {
            entry.push_str(&format!(" {}", quoted(title, MAX_NAME)));
        }
        if w.focused {
            entry.push_str(" (focused)");
        }
        let gap = if n == 0 { " " } else { GAP };
        let more = windows.len() - n;
        let next = estimate_tokens(&line) + estimate_tokens(&entry);
        if more > 1 && next + MORE_TOKENS > MAX_LIST_TOKENS || next > MAX_LIST_TOKENS {
            line.push_str(&format!(" +{more} more"));
            break;
        }
        line.push_str(gap);
        line.push_str(&entry);
    }
    line
}

fn fits(entries: &[Entry], room: usize) -> bool {
    entries
        .iter()
        .map(|e| estimate_tokens(&e.line()) + 1)
        .sum::<usize>()
        <= room
}

/// One shown node: a short id when the agent can act on it.
struct Entry {
    short: Option<String>,
    node: u32,
    body: String,
}

impl Entry {
    fn line(&self) -> String {
        match &self.short {
            Some(short) => format!("[{short}] {}", self.body),
            None => self.body.clone(),
        }
    }
}

/// The nodes shown, in tree order; `labels` keeps static text.
fn entries(tree: &Tree, labels: bool) -> Vec<Entry> {
    enum Step {
        Visit(u32),
        /// `n` of a container's items were not shown.
        Rest(u32, usize),
    }
    let index: BTreeMap<u32, &Node> = tree.nodes.iter().map(|n| (n.id, n)).collect();
    let mut out = Vec::new();
    let mut count = 0;
    let mut stack = alloc::vec![Step::Visit(tree.root)];
    // A tree that came from `Tree::apply` has no cycle; a bound keeps a hand-made
    // one from looping.
    let mut budget = 2 * surfabi::tree::MAX_NODES;
    while let Some(step) = stack.pop() {
        budget = match budget.checked_sub(1) {
            Some(left) => left,
            None => break,
        };
        let node = match step {
            Step::Rest(container, n) => {
                out.push(Entry {
                    short: None,
                    node: container,
                    body: format!("+{n} more items"),
                });
                continue;
            }
            Step::Visit(id) => match index.get(&id) {
                Some(node) => *node,
                None => continue,
            },
        };
        if let Some(body) = body(&index, tree.focus, node, labels) {
            let short = match node.actions {
                0 => None,
                _ => {
                    count += 1;
                    Some(format!("{}{count}", letter(node.role)))
                }
            };
            out.push(Entry {
                short,
                node: node.id,
                body,
            });
        }
        // A row is one line of its cells' text: only cells the agent can act on
        // get their own.
        let kids: Vec<u32> = match node.role {
            Role::Row => node
                .children
                .iter()
                .filter(|c| index.get(c).is_some_and(|c| c.actions != 0))
                .copied()
                .collect(),
            _ => node.children.clone(),
        };
        let shown = match is_container(node.role) {
            true => kids.len().min(MAX_ITEMS),
            false => kids.len(),
        };
        if shown < kids.len() {
            stack.push(Step::Rest(node.id, kids.len() - shown));
        }
        stack.extend(kids[..shown].iter().rev().map(|&c| Step::Visit(c)));
    }
    out
}

/// A node's text after its short id, or `None` when it is not shown.
fn body(index: &Nodes, focus: u32, node: &Node, labels: bool) -> Option<String> {
    let named = !node.name.trim().is_empty();
    let shown = match node.role {
        Role::Window => false,
        Role::Group | Role::ScrollView | Role::Splitter | Role::TabPanel => node.actions != 0,
        Role::Label | Role::Cell | Role::Image => {
            node.actions != 0 || labels && (named || !node.value.is_empty())
        }
        _ => true,
    };
    if !shown {
        return None;
    }
    // A list's items, a table's rows and a tab list's tabs follow their
    // container, so their short id's letter says what they are.
    let mut out = match is_member(node.role) && node.actions != 0 {
        true => String::new(),
        false => String::from(word(node.role)),
    };
    let name = match (named, node.role) {
        (true, _) => Some(quoted(&node.name, MAX_NAME)),
        (false, Role::Row) => Some(quoted(&row_text(index, node), MAX_NAME)),
        _ => None,
    };
    if let Some(name) = name {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&name);
    }
    match node.role {
        Role::Switch => out.push_str(if node.flags & flag::CHECKED != 0 {
            " = on"
        } else {
            " = off"
        }),
        _ if is_container(node.role) => {
            out.push_str(&format!(" {} items", node.children.len()));
            let selected = node
                .children
                .iter()
                .filter_map(|c| index.get(c))
                .find(|c| c.flags & flag::SELECTED != 0);
            if let Some(item) = selected {
                let name = item_name(index, item);
                out.push_str(&format!(", selected {}", quoted(&name, MAX_NAME)));
            }
        }
        _ if !node.value.is_empty() => {
            out.push_str(" = ");
            out.push_str(&quoted(&node.value, MAX_VALUE));
        }
        _ => {}
    }
    let flags = [
        (flag::SELECTED, " selected"),
        (flag::CHECKED, " checked"),
        (flag::EXPANDED, " expanded"),
        (flag::DISABLED, " disabled"),
    ];
    for (bit, word) in flags {
        let said = bit == flag::CHECKED && node.role == Role::Switch
            || bit == flag::SELECTED && is_container(node.role);
        if node.flags & bit != 0 && !said {
            out.push_str(word);
        }
    }
    if node.id == focus && node.role != Role::Window {
        out.push_str(" focused");
    }
    // An unnamed member still says what it is.
    match out.trim_start() {
        "" => Some(String::from(word(node.role))),
        rest if rest.len() < out.len() => Some(format!("{} {rest}", word(node.role))),
        _ => Some(out),
    }
}

/// A row by its first cell's text: what names it (a file, a process), without
/// the digits of its other columns, which cost a token each.
fn row_text(index: &Nodes, row: &Node) -> String {
    row.children
        .iter()
        .filter_map(|c| index.get(c))
        .map(|c| match c.name.trim().is_empty() {
            true => c.value.trim(),
            false => c.name.trim(),
        })
        .find(|text| !text.is_empty())
        .map_or_else(String::new, String::from)
}

/// An item's name, or the first named node below it (a row's first cell).
fn item_name(index: &Nodes, item: &Node) -> String {
    if !item.name.trim().is_empty() {
        return item.name.clone();
    }
    let mut stack: Vec<u32> = item.children.iter().rev().copied().collect();
    let mut steps = 0;
    while let Some(at) = stack.pop() {
        steps += 1;
        if steps > surfabi::tree::MAX_NODES {
            break;
        }
        let Some(node) = index.get(&at) else { continue };
        if !node.name.trim().is_empty() {
            return node.name.clone();
        }
        stack.extend(node.children.iter().rev());
    }
    String::new()
}

fn is_member(role: Role) -> bool {
    matches!(
        role,
        Role::ListItem | Role::Row | Role::Tab | Role::MenuItem
    )
}

fn is_container(role: Role) -> bool {
    matches!(role, Role::List | Role::Table | Role::TabList | Role::Menu)
}

/// The role as the view names it: short, common words.
fn word(role: Role) -> &'static str {
    match role {
        Role::Window => "window",
        Role::Group => "group",
        Role::Label => "text",
        Role::Button => "button",
        Role::TextInput => "textbox",
        Role::MultilineTextInput => "textarea",
        Role::List => "list",
        Role::ListItem => "item",
        Role::Table => "table",
        Role::Row => "row",
        Role::Cell => "cell",
        Role::TabList => "tabs",
        Role::Tab => "tab",
        Role::TabPanel => "panel",
        Role::Switch => "switch",
        Role::Slider => "slider",
        Role::Menu => "menu",
        Role::MenuItem => "menuitem",
        Role::ScrollView => "scroll",
        Role::Splitter => "splitter",
        Role::Image => "image",
        Role::ProgressIndicator => "progress",
    }
}

/// The short id's letter: what kind of thing the agent acts on.
fn letter(role: Role) -> char {
    match role {
        Role::Button => 'b',
        Role::TextInput | Role::MultilineTextInput => 't',
        Role::List | Role::Table => 'l',
        Role::ListItem | Role::Row => 'i',
        Role::TabList | Role::Tab => 'a',
        Role::Switch => 's',
        Role::Slider => 'r',
        Role::Menu | Role::MenuItem => 'm',
        _ => 'n',
    }
}

/// `text` in quotes, cut to `max` characters with `...`: `"` and `\` escaped,
/// a line break as `\n`, and any other control character as `?`.
fn quoted(text: &str, max: usize) -> String {
    let cut = text.chars().count() > max;
    let mut out = String::from("\"");
    out.push_str(&clean(text, max));
    if cut {
        out.push_str("...");
    }
    out.push('"');
    out
}

/// An app id as a bare word: a client sets it, so anything but letters,
/// digits, `.`, `_` and `-` shows as `?` and it cannot pose as list syntax.
fn bare(text: &str) -> String {
    text.trim()
        .chars()
        .take(MAX_NAME)
        .map(
            |c| match c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                true => c,
                false => '?',
            },
        )
        .collect()
}

/// `text`'s first `max` characters, escaped for a quoted string.
fn clean(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.trim().chars().take(max) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push(' '),
            c if c.is_control() => out.push('?'),
            c => out.push(c),
        }
    }
    out
}
