//! S05-2's mock trees (ADR 0005, "Spikes that settle it"): one window of each
//! P4 app, as full as a working session leaves it, for the view's size check.
//! The host tests hold each view to [`super::MAX_TOKENS`] by estimate, and
//! `cargo xtask tokens` counts them with the GGUF vocabulary.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use surfabi::tree::{Node, Role, Tree, Update, action, flag};

/// A tree built node by node: the window is node 1.
pub struct Mock {
    nodes: Vec<Node>,
    focus: u32,
}

impl Mock {
    pub fn new(title: &str) -> Self {
        Self {
            nodes: alloc::vec![node(1, Role::Window, title)],
            focus: 1,
        }
    }

    /// A child of `parent` with the actions its role usually offers.
    pub fn add(&mut self, parent: u32, role: Role, name: &str) -> u32 {
        let id = self.nodes.len() as u32 + 1;
        let mut child = node(id, role, name);
        child.actions = default_actions(role);
        self.nodes.push(child);
        self.at(parent).children.push(id);
        id
    }

    pub fn value(&mut self, id: u32, value: &str) -> &mut Self {
        self.at(id).value = String::from(value);
        self
    }

    pub fn flags(&mut self, id: u32, flags: u8) -> &mut Self {
        self.at(id).flags |= flags;
        self
    }

    pub fn focus(&mut self, id: u32) -> &mut Self {
        self.focus = id;
        self
    }

    /// The tree as `desktop` keeps it, through the same checks a client's
    /// update meets.
    pub fn tree(self) -> Tree {
        let mut tree = Tree::default();
        tree.apply(Update {
            root: 1,
            focus: self.focus,
            nodes: self.nodes,
        })
        .expect("a mock tree is a valid update");
        tree
    }

    fn at(&mut self, id: u32) -> &mut Node {
        &mut self.nodes[id as usize - 1]
    }
}

fn node(id: u32, role: Role, name: &str) -> Node {
    Node {
        id,
        role,
        flags: 0,
        actions: 0,
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        name: String::from(name),
        value: String::new(),
        children: Vec::new(),
    }
}

fn default_actions(role: Role) -> u16 {
    use action::*;
    match role {
        Role::Button | Role::ListItem | Role::Row | Role::Tab | Role::MenuItem => CLICK,
        Role::Switch => CLICK,
        Role::TextInput | Role::MultilineTextInput => FOCUS | SET_VALUE,
        Role::Slider => SET_VALUE | INCREMENT | DECREMENT,
        Role::List | Role::Table => FOCUS,
        Role::Menu => EXPAND,
        _ => 0,
    }
}

/// Each P4 app's window: (app, title, tree).
pub fn apps() -> Vec<(&'static str, &'static str, Tree)> {
    alloc::vec![
        ("terminal", "Terminal", terminal()),
        ("files", "Documents", files()),
        ("notes", "Notes", notes()),
        ("settings", "Settings", settings()),
        ("monitor", "System monitor", monitor()),
        ("models", "Models", models()),
        ("editor", "main.rs", editor()),
    ]
}

/// Lines of scrollback, about `bytes` long.
fn scrollback(bytes: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    while out.len() < bytes {
        n += 1;
        out.push_str(&format!(
            "kai> fs_list(\"/home/user/build-{n}\")\nmain.rs  notes.txt\n"
        ));
    }
    out
}

fn terminal() -> Tree {
    let mut m = Mock::new("Terminal");
    let output = m.add(1, Role::MultilineTextInput, "Output");
    m.value(output, &scrollback(3_000));
    let line = m.add(1, Role::TextInput, "kai>");
    m.focus(line);
    m.tree()
}

fn files() -> Tree {
    let mut m = Mock::new("Documents");
    let bar = m.add(1, Role::Group, "");
    for name in ["Back", "Up", "New folder", "Rename", "Trash"] {
        m.add(bar, Role::Button, name);
    }
    let path = m.add(bar, Role::TextInput, "Path");
    m.value(path, "/home/user/Documents");
    m.add(bar, Role::TextInput, "Search");
    let split = m.add(1, Role::Splitter, "");
    let places = m.add(split, Role::List, "Places");
    for name in ["Home", "Documents", "Downloads", "Pictures", "Trash"] {
        m.add(places, Role::ListItem, name);
    }
    let table = m.add(split, Role::Table, "Files");
    for n in 0..24 {
        let row = m.add(table, Role::Row, "");
        if n == 3 {
            m.flags(row, flag::SELECTED);
        }
        m.add(row, Role::Cell, &format!("report-{n:02}.txt"));
        let size = m.add(row, Role::Cell, "");
        m.value(size, &format!("{} KiB", 4 + n * 3));
        let date = m.add(row, Role::Cell, "");
        m.value(date, "2026-10-27 14:02");
    }
    m.add(1, Role::Label, "24 items, 1 selected");
    m.tree()
}

fn notes() -> Tree {
    let mut m = Mock::new("Notes");
    let split = m.add(1, Role::Splitter, "");
    let side = m.add(split, Role::Group, "");
    m.add(side, Role::TextInput, "Search");
    m.add(side, Role::Button, "New note");
    let list = m.add(side, Role::List, "Notes");
    let titles = [
        "Groceries",
        "Ideas for the desktop: a launcher",
        "Call the plumber about the kitchen sink",
        "Books to read",
        "Packing list",
        "Meeting notes 27 October",
        "Recipes",
        "Birthday presents",
        "Garden",
        "Car service",
        "Passwords to change",
        "Holiday",
    ];
    for (n, title) in titles.iter().enumerate() {
        let item = m.add(list, Role::ListItem, title);
        if n == 0 {
            m.flags(item, flag::SELECTED);
        }
    }
    let main = m.add(split, Role::Group, "");
    let editor = m.add(main, Role::MultilineTextInput, "Note");
    m.value(editor, "Groceries\nmilk and eggs\nbread\ncoffee\napples");
    m.add(main, Role::Button, "Delete note");
    m.focus(editor);
    m.add(1, Role::Label, "12 notes, 80 words");
    m.tree()
}

fn settings() -> Tree {
    let mut m = Mock::new("Settings");
    let tabs = m.add(1, Role::TabList, "Pages");
    let pages = [
        "Appearance",
        "Display",
        "Keyboard",
        "Sound",
        "Network",
        "Models",
        "Privacy",
        "About",
    ];
    for (n, page) in pages.iter().enumerate() {
        let tab = m.add(tabs, Role::Tab, page);
        if n == 0 {
            m.flags(tab, flag::SELECTED);
        }
    }
    let panel = m.add(1, Role::TabPanel, "Appearance");
    m.add(panel, Role::Label, "Choose how the desktop looks.");
    let theme = m.add(panel, Role::Menu, "Theme");
    m.value(theme, "Ember Workbench");
    let size = m.add(panel, Role::Slider, "Text size");
    m.value(size, "100%");
    let scale = m.add(panel, Role::Slider, "Scale");
    m.value(scale, "125%");
    m.add(panel, Role::Switch, "Reduced motion");
    let legible = m.add(panel, Role::Switch, "Legible type");
    m.flags(legible, flag::CHECKED);
    m.add(panel, Role::Switch, "High contrast");
    m.add(
        panel,
        Role::Label,
        "Changes apply at once and can be undone.",
    );
    m.add(panel, Role::Button, "Reset to defaults");
    m.tree()
}

fn monitor() -> Tree {
    let mut m = Mock::new("System monitor");
    let tabs = m.add(1, Role::TabList, "Views");
    let processes = m.add(tabs, Role::Tab, "Processes");
    m.flags(processes, flag::SELECTED);
    m.add(tabs, Role::Tab, "Resources");
    let cpu = m.add(1, Role::ProgressIndicator, "CPU");
    m.value(cpu, "23%");
    let mem = m.add(1, Role::ProgressIndicator, "Memory");
    m.value(mem, "41%");
    m.add(1, Role::Label, "Model: ready, 12 tokens/s");
    let table = m.add(1, Role::Table, "Processes");
    let names = [
        "llm",
        "desktop",
        "shell",
        "comphost",
        "fs",
        "supervisor",
        "net",
        "pkg",
        "notes",
        "files",
        "monitor",
        "terminal",
        "updated",
        "editor",
        "settings",
        "models",
        "work",
        "probe",
    ];
    for (n, name) in names.iter().enumerate() {
        let row = m.add(table, Role::Row, "");
        m.add(row, Role::Cell, name);
        let cpu = m.add(row, Role::Cell, "");
        m.value(cpu, &format!("{}.{}%", 20 - n, n % 10));
        let mem = m.add(row, Role::Cell, "");
        m.value(mem, &format!("{} MiB", 2400 / (n + 1)));
    }
    let end = m.add(1, Role::Button, "End process");
    m.flags(end, flag::DISABLED);
    m.tree()
}

fn models() -> Tree {
    let mut m = Mock::new("Models");
    let list = m.add(1, Role::List, "Models");
    for (n, name) in ["qwen3-4b-2507", "qwen3-1.7b", "gemma-3-4b", "phi-4-mini"]
        .iter()
        .enumerate()
    {
        let item = m.add(list, Role::ListItem, name);
        if n == 0 {
            m.flags(item, flag::SELECTED);
        }
    }
    let detail = m.add(1, Role::Group, "qwen3-4b-2507");
    m.add(detail, Role::Label, "Size 2.5 GB, Q4_K_M");
    m.add(detail, Role::Label, "Status: loaded, 12 tokens/s");
    let keep = m.add(detail, Role::Switch, "Keep loaded");
    m.flags(keep, flag::CHECKED);
    for name in ["Load", "Unload", "Remove", "Add from file"] {
        m.add(detail, Role::Button, name);
    }
    let download = m.add(1, Role::ProgressIndicator, "Download");
    m.value(download, "0%");
    m.tree()
}

fn editor() -> Tree {
    let mut m = Mock::new("main.rs");
    let bar = m.add(1, Role::Group, "");
    for name in ["File", "Edit", "View", "Help"] {
        m.add(bar, Role::Menu, name);
    }
    let tabs = m.add(1, Role::TabList, "Open files");
    for (n, name) in ["main.rs", "notes.txt", "README.md"].iter().enumerate() {
        let tab = m.add(tabs, Role::Tab, name);
        if n == 0 {
            m.flags(tab, flag::SELECTED);
        }
    }
    let text = m.add(1, Role::MultilineTextInput, "main.rs");
    let mut code = String::new();
    while code.len() < 4_000 {
        code.push_str("fn main() {\n    println!(\"hello, kai\");\n}\n");
    }
    m.value(text, &code);
    m.focus(text);
    let find = m.add(1, Role::TextInput, "Find");
    m.value(find, "println");
    m.add(1, Role::Button, "Save");
    m.add(1, Role::Button, "Run");
    m.add(1, Role::Label, "Ln 12, Col 4  UTF-8  Rust");
    m.tree()
}
