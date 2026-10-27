//! The accessibility tree an app sends with `a11y.tree_update` and applies with
//! `a11y.commit` (ADR 0005 §3, §6). The records mirror a pinned subset of
//! AccessKit 0.25.1's `TreeUpdate`: role, name, value, bounds, actions,
//! children and focus. An AccessKit upgrade changes a converter in `desktop`,
//! never these bytes.
//!
//! An update is little-endian: `root: u32, focus: u32, count: u32`, then
//! `count` nodes. A node is `id: u32, role: u8, flags: u8, actions: u16,
//! x: i32, y: i32, w: u32, h: u32, name_len: u16, value_len: u16,
//! children: u16, 0: u16`, its name and value in UTF-8, padding to 4, then
//! `children` ids of u32. An update replaces the nodes it carries and keeps the
//! rest, as AccessKit's does; after it every child, the root and the focus must
//! name a node, nothing may be its own ancestor, and the tree stays within
//! [`MAX_NODES`] nodes and [`MAX_BYTES`] bytes.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

/// Nodes in one window's tree (ADR 0005 §3).
pub const MAX_NODES: usize = 4096;
/// Bytes one window's tree may take, as its updates' node records add up.
pub const MAX_BYTES: usize = 256 * 1024;
pub const MAX_NAME: usize = 1024;
pub const MAX_VALUE: usize = 4096;
const UPDATE_HEAD: usize = 12;
const NODE_HEAD: usize = 32;

/// The roles of ADR 0005 §4's v0.1 widgets, with AccessKit 0.25.1's names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    Window = 1,
    Group = 2,
    Label = 3,
    Button = 4,
    TextInput = 5,
    MultilineTextInput = 6,
    List = 7,
    ListItem = 8,
    Table = 9,
    Row = 10,
    Cell = 11,
    TabList = 12,
    Tab = 13,
    TabPanel = 14,
    Switch = 15,
    Slider = 16,
    Menu = 17,
    MenuItem = 18,
    ScrollView = 19,
    Splitter = 20,
    Image = 21,
    ProgressIndicator = 22,
}

impl Role {
    pub fn from_raw(raw: u8) -> Option<Role> {
        use Role::*;
        const ALL: [Role; 22] = [
            Window,
            Group,
            Label,
            Button,
            TextInput,
            MultilineTextInput,
            List,
            ListItem,
            Table,
            Row,
            Cell,
            TabList,
            Tab,
            TabPanel,
            Switch,
            Slider,
            Menu,
            MenuItem,
            ScrollView,
            Splitter,
            Image,
            ProgressIndicator,
        ];
        ALL.into_iter().find(|role| *role as u8 == raw)
    }
}

/// Node flags.
pub mod flag {
    pub const FOCUSABLE: u8 = 1 << 0;
    pub const DISABLED: u8 = 1 << 1;
    pub const SELECTED: u8 = 1 << 2;
    pub const CHECKED: u8 = 1 << 3;
    pub const EXPANDED: u8 = 1 << 4;
    pub const ALL: u8 = 0x1F;
}

/// The actions a node offers, as bits; `action_request` names one by its bit
/// number. AccessKit 0.25.1's `Action` names.
pub mod action {
    pub const CLICK: u16 = 1 << 0;
    pub const FOCUS: u16 = 1 << 1;
    pub const SET_VALUE: u16 = 1 << 2;
    pub const INCREMENT: u16 = 1 << 3;
    pub const DECREMENT: u16 = 1 << 4;
    pub const EXPAND: u16 = 1 << 5;
    pub const COLLAPSE: u16 = 1 << 6;
    pub const SCROLL_INTO_VIEW: u16 = 1 << 7;
    pub const SHOW_CONTEXT_MENU: u16 = 1 << 8;
    pub const ALL: u16 = 0x1FF;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: u32,
    pub role: Role,
    pub flags: u8,
    pub actions: u16,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub name: String,
    pub value: String,
    pub children: Vec<u32>,
}

impl Node {
    /// The bytes this node takes in an update.
    pub fn encoded_len(&self) -> usize {
        NODE_HEAD
            + (self.name.len() + self.value.len()).next_multiple_of(4)
            + 4 * self.children.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub root: u32,
    pub focus: u32,
    pub nodes: Vec<Node>,
}

/// Why an update was refused. `desktop` keeps the tree it had and closes the
/// connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    Trailing,
    BadRole,
    BadFlags,
    BadActions,
    BadText,
    TooLong,
    /// Id 0, or a node given twice in one update.
    BadId,
    /// A child, the root or the focus names no node.
    Dangling,
    /// A node is its own ancestor, or has two parents.
    Cycle,
    TooBig,
}

struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.0.len() {
            return Err(Error::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn text(&mut self, len: usize, max: usize) -> Result<String, Error> {
        if len > max {
            return Err(Error::TooLong);
        }
        let bytes = self.take(len)?;
        let text = core::str::from_utf8(bytes).map_err(|_| Error::BadText)?;
        if text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(Error::BadText);
        }
        Ok(String::from(text))
    }
}

impl Update {
    /// Decodes an update. The node count is checked against the bytes before
    /// anything is allocated, so a huge count costs nothing.
    pub fn decode(bytes: &[u8]) -> Result<Update, Error> {
        if bytes.len() > MAX_BYTES + UPDATE_HEAD {
            return Err(Error::TooBig);
        }
        let mut b = Bytes(bytes);
        let (root, focus, count) = (b.u32()?, b.u32()?, b.u32()? as usize);
        if count > MAX_NODES || count * NODE_HEAD > b.0.len() {
            return Err(Error::Truncated);
        }
        let mut nodes = Vec::with_capacity(count);
        let mut ids = BTreeSet::new();
        for _ in 0..count {
            let id = b.u32()?;
            let head = b.take(4)?;
            let (role, flags, actions) = (head[0], head[1], u16::from_le_bytes([head[2], head[3]]));
            let (x, y, w, h) = (b.u32()? as i32, b.u32()? as i32, b.u32()?, b.u32()?);
            let (name_len, value_len) = (usize::from(b.u16()?), usize::from(b.u16()?));
            let (children, zero) = (usize::from(b.u16()?), b.u16()?);
            if id == 0 || !ids.insert(id) {
                return Err(Error::BadId);
            }
            if zero != 0 {
                return Err(Error::Trailing);
            }
            if flags & !flag::ALL != 0 {
                return Err(Error::BadFlags);
            }
            if actions & !action::ALL != 0 {
                return Err(Error::BadActions);
            }
            let name = b.text(name_len, MAX_NAME)?;
            let value = b.text(value_len, MAX_VALUE)?;
            let pad = (name_len + value_len).next_multiple_of(4) - (name_len + value_len);
            if b.take(pad)?.iter().any(|&p| p != 0) {
                return Err(Error::Trailing);
            }
            let kids = (0..children)
                .map(|_| b.u32())
                .collect::<Result<Vec<_>, _>>()?;
            nodes.push(Node {
                id,
                role: Role::from_raw(role).ok_or(Error::BadRole)?,
                flags,
                actions,
                x,
                y,
                w,
                h,
                name,
                value,
                children: kids,
            });
        }
        if !b.0.is_empty() {
            return Err(Error::Trailing);
        }
        Ok(Update { root, focus, nodes })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [self.root, self.focus, self.nodes.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for n in &self.nodes {
            out.extend_from_slice(&n.id.to_le_bytes());
            out.extend_from_slice(&[n.role as u8, n.flags]);
            out.extend_from_slice(&n.actions.to_le_bytes());
            for v in [n.x as u32, n.y as u32, n.w, n.h] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for v in [n.name.len(), n.value.len(), n.children.len(), 0] {
                out.extend_from_slice(&(v as u16).to_le_bytes());
            }
            out.extend_from_slice(n.name.as_bytes());
            out.extend_from_slice(n.value.as_bytes());
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            for child in &n.children {
                out.extend_from_slice(&child.to_le_bytes());
            }
        }
        out
    }
}

/// A window's tree as `desktop` keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub root: u32,
    pub focus: u32,
    pub nodes: Vec<Node>,
}

impl Tree {
    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Applies `update`, or leaves the tree as it was and says why. Nodes no
    /// longer reachable from the root are dropped.
    pub fn apply(&mut self, update: Update) -> Result<(), Error> {
        let mut next = self.clone();
        next.root = update.root;
        next.focus = update.focus;
        for node in update.nodes {
            match next.nodes.iter_mut().find(|n| n.id == node.id) {
                Some(old) => *old = node,
                None => next.nodes.push(node),
            }
        }
        next.prune()?;
        if next.nodes.len() > MAX_NODES {
            return Err(Error::TooBig);
        }
        if next.nodes.iter().map(Node::encoded_len).sum::<usize>() > MAX_BYTES {
            return Err(Error::TooBig);
        }
        *self = next;
        Ok(())
    }

    /// Keeps the nodes reachable from the root, in walk order, and checks every
    /// edge: a child names a node, and no node is reached twice.
    fn prune(&mut self) -> Result<(), Error> {
        let by_id: BTreeMap<u32, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();
        let index = |id: u32| by_id.get(&id).copied();
        let mut seen = alloc::vec![false; self.nodes.len()];
        let mut order = Vec::new();
        let mut stack = alloc::vec![index(self.root).ok_or(Error::Dangling)?];
        while let Some(at) = stack.pop() {
            if seen[at] {
                return Err(Error::Cycle);
            }
            seen[at] = true;
            order.push(at);
            for &child in self.nodes[at].children.iter().rev() {
                stack.push(index(child).ok_or(Error::Dangling)?);
            }
        }
        if !order.iter().any(|&at| self.nodes[at].id == self.focus) {
            return Err(Error::Dangling);
        }
        let mut kept = Vec::with_capacity(order.len());
        for at in order {
            kept.push(core::mem::replace(&mut self.nodes[at], placeholder()));
        }
        self.nodes = kept;
        Ok(())
    }
}

fn placeholder() -> Node {
    Node {
        id: 0,
        role: Role::Group,
        flags: 0,
        actions: 0,
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        name: String::new(),
        value: String::new(),
        children: Vec::new(),
    }
}
