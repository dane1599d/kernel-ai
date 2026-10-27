//! The accessibility tree, built from the same widgets that draw (ADR 0005
//! section 4): an app cannot show a control the agent cannot see. comphost turns
//! it into `surfabi`'s `a11y.tree_update` records, a pinned AccessKit subset.

use crate::geom::Rect;
use crate::layout::Places;
use crate::state::State;
use crate::tree::Tree;
use crate::widget::{NodeId, ROOT, Widget};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Window,
    Group,
    Label,
    Button,
    TextInput,
    MultilineText,
    List,
    Table,
    TabList,
    Switch,
    Slider,
    Menu,
    ScrollView,
    Split,
    Image,
    Progress,
}

impl Role {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Group => "group",
            Self::Label => "label",
            Self::Button => "button",
            Self::TextInput => "text_input",
            Self::MultilineText => "multiline_text",
            Self::List => "list",
            Self::Table => "table",
            Self::TabList => "tab_list",
            Self::Switch => "switch",
            Self::Slider => "slider",
            Self::Menu => "menu",
            Self::ScrollView => "scroll_view",
            Self::Split => "split",
            Self::Image => "image",
            Self::Progress => "progress",
        }
    }
}

/// What the agent, or a screen reader, may ask a node to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Focus,
    Click,
    SetValue,
    Increment,
    Decrement,
}

impl Action {
    pub const ALL: [Self; 5] = [
        Self::Focus,
        Self::Click,
        Self::SetValue,
        Self::Increment,
        Self::Decrement,
    ];

    pub const fn bit(self) -> u8 {
        1 << self as u8
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Click => "click",
            Self::SetValue => "set_value",
            Self::Increment => "increment",
            Self::Decrement => "decrement",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A11yNode {
    pub id: NodeId,
    pub role: Role,
    pub name: String,
    pub value: Option<String>,
    pub bounds: Rect,
    /// [`Action::bit`]s.
    pub actions: u8,
    pub children: Vec<NodeId>,
    pub focused: bool,
    /// The declared action a button runs, so `ui_act` takes its tier.
    pub bound: Option<String>,
}

impl A11yNode {
    pub fn allows(&self, a: Action) -> bool {
        self.actions & a.bit() != 0
    }
}

fn acts(list: &[Action]) -> u8 {
    list.iter().fold(0, |b, a| b | a.bit())
}

/// One node's entry, or none for a node with no place.
/// A node's role, name, value, action bits and bound action.
type Entry = (Role, String, Option<String>, u8, Option<String>);

fn node_of(tree: &Tree, id: NodeId, title: &str) -> Option<Entry> {
    use Action::*;
    let n = tree.get(id)?;
    let pick = |items: &[String], i: Option<u32>| i.and_then(|i| items.get(i as usize)).cloned();
    Some(match &n.widget {
        Widget::Stack { .. } if id == ROOT => (Role::Window, String::from(title), None, 0, None),
        Widget::Stack { .. } => (Role::Group, String::new(), None, 0, None),
        Widget::Label { text, .. } => (Role::Label, text.clone(), None, 0, None),
        Widget::Button { text, action } => (
            Role::Button,
            text.clone(),
            None,
            acts(&[Focus, Click]),
            action.clone(),
        ),
        Widget::TextLine(f) => (
            Role::TextInput,
            f.label.clone(),
            Some(f.value.clone()),
            acts(&[Focus, SetValue]),
            None,
        ),
        Widget::TextArea(f) => (
            Role::MultilineText,
            f.label.clone(),
            Some(f.value.clone()),
            acts(&[Focus, SetValue]),
            None,
        ),
        Widget::ItemList { label, selected } => {
            let v = format!(
                "{} items{}",
                n.children.len(),
                selected.map_or(String::new(), |s| format!(", selected {s}"))
            );
            (
                Role::List,
                label.clone(),
                Some(v),
                acts(&[Focus, SetValue]),
                None,
            )
        }
        Widget::Table {
            label,
            rows,
            selected,
            ..
        } => {
            let row = selected
                .and_then(|s| rows.get(s as usize))
                .map(|r| r.join(" | "));
            let v = format!(
                "{} rows{}",
                rows.len(),
                row.map_or(String::new(), |r| format!(", selected \"{r}\""))
            );
            (
                Role::Table,
                label.clone(),
                Some(v),
                acts(&[Focus, SetValue]),
                None,
            )
        }
        Widget::Tabs { labels, selected } => (
            Role::TabList,
            String::new(),
            pick(labels, Some(*selected)),
            acts(&[Focus, SetValue]),
            None,
        ),
        Widget::Toggle { label, on } => (
            Role::Switch,
            label.clone(),
            Some(String::from(if *on { "on" } else { "off" })),
            acts(&[Focus, Click]),
            None,
        ),
        Widget::Slider { label, value, .. } => (
            Role::Slider,
            label.clone(),
            Some(format!("{value}")),
            acts(&[Focus, SetValue, Increment, Decrement]),
            None,
        ),
        Widget::Menu { label, .. } => (
            Role::Menu,
            label.clone(),
            None,
            acts(&[Focus, SetValue]),
            None,
        ),
        Widget::Scroll { .. } => (Role::ScrollView, String::new(), None, 0, None),
        Widget::Split { .. } => (Role::Split, String::new(), None, 0, None),
        Widget::Image { label, .. } => (Role::Image, label.clone(), None, 0, None),
        Widget::Progress { label, done } => {
            let v = done.map_or(String::from("working"), |d| {
                format!("{}%", d.min(1000) / 10)
            });
            (Role::Progress, label.clone(), Some(v), 0, None)
        }
    })
}

/// The window's tree, in preorder; nodes not shown (other tabs' pages) are left out.
pub fn build(tree: &Tree, places: &Places, st: &State, title: &str) -> Vec<A11yNode> {
    let mut out = Vec::new();
    for id in tree.preorder() {
        let Some(p) = places.get(&id) else {
            continue;
        };
        let Some((role, name, value, actions, bound)) = node_of(tree, id, title) else {
            continue;
        };
        let children = tree
            .get(id)
            .map(|n| {
                n.children
                    .iter()
                    .copied()
                    .filter(|c| places.contains_key(c))
                    .collect()
            })
            .unwrap_or_default();
        out.push(A11yNode {
            id,
            role,
            name,
            value,
            bounds: p.rect.intersect(&p.clip),
            actions,
            children,
            focused: st.focus == Some(id),
            bound,
        });
    }
    out
}
