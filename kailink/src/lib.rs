//! Links one relocatable Wasm object (tool-conventions Linking.md, as `wcc -c`
//! writes it) into a session (ADR 0012 D3-D4).
//!
//! A session is one memory, one funcref table and one `__stack_pointer`, shared by
//! every module linked into it. [`Session::link`] turns an object into a module
//! that imports those three and the host calls it may use, and plans where its
//! data and functions go; [`Session::commit`] records the plan once the entry has
//! run. Nothing changes in the session until then, so a compile error or a trap
//! commits no definitions.
//!
//! - **Data** goes at the next free address, each segment at its alignment.
//! - **Functions** each get a table slot. A global function redefined with the
//!   same type keeps its slot, so every caller and stored pointer switches; with a
//!   different type it gets a new slot and old callers keep the old body.
//! - **Calls to earlier entries** go through a thunk, `call_indirect` on the
//!   callee's slot, so they follow redefinition. Calls within one object stay
//!   direct.
//! - **Imports** are only module `kai` fields the caller allows, and WASI
//!   `fd_write`; anything else that no earlier entry defines fails to link,
//!   naming the symbol.

#![no_std]

extern crate alloc;

mod leb;
mod link;
mod object;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// The module the session's memory, table and stack pointer are imported from.
pub const ENV: &str = "env";
pub const MEMORY: &str = "memory";
pub const TABLE: &str = "table";
pub const STACK_POINTER: &str = "__stack_pointer";
/// The only WASI call session code may import, for output on fds 1 and 2.
pub const WASI: &str = "wasi_snapshot_preview1";
pub const FD_WRITE: &str = "fd_write";
/// The module of the OS's actions (ADR 0012 D6).
pub const KAI: &str = "kai";

/// A number type; vectors and references are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

/// Where a session's pieces live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The first address data may use; the stack lies below it.
    pub data_base: u32,
    /// The end of memory data may reach.
    pub memory_limit: u32,
    /// Most table slots. Slot 0 is never used, so a null function pointer traps.
    pub table_limit: u32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            data_base: 64 << 10,
            memory_limit: 64 << 20,
            table_limit: 4096,
        }
    }
}

/// A name the session defines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Symbol {
    Func { slot: u32, ty: FuncType },
    Data { addr: u32, size: u32 },
}

/// What one linked entry defines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    pub name: String,
    pub symbol: Symbol,
    /// Whether it replaces a definition of the same name.
    pub replaces: bool,
}

/// One object, linked and ready to instantiate in the session's Store.
#[derive(Clone, Debug)]
pub struct Linked {
    pub wasm: Vec<u8>,
    /// Table slots the module's elements need: grow the table to this first.
    pub table_size: u32,
    /// The end of the module's data: grow memory to hold it first.
    pub data_end: u32,
    /// The global definitions, to commit after the entry runs.
    pub defs: Vec<Def>,
    /// The functions the module exports, by symbol name.
    pub exports: Vec<String>,
    /// Old definitions this one replaces with a different type; their callers
    /// keep the old body.
    pub retyped: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a well-formed object.
    Parse(String),
    /// Well formed, but uses something the session does not offer.
    Unsupported(String),
    /// A symbol no earlier entry defines and no import provides.
    Undefined(String),
    /// A symbol used with a different type than its definition.
    TypeMismatch(String),
    /// An import the session does not allow.
    Import { module: String, field: String },
    /// The table or memory would pass its limit.
    Full(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(why) => write!(f, "bad object: {why}"),
            Error::Unsupported(what) => write!(f, "not supported: {what}"),
            Error::Undefined(name) => write!(f, "undefined symbol {name}"),
            Error::TypeMismatch(name) => write!(f, "{name} is used with a different type"),
            Error::Import { module, field } => write!(f, "import {module}.{field} is not allowed"),
            Error::Full(what) => write!(f, "the session's {what} is full"),
        }
    }
}

/// Which `kai` imports session code may use: field name and type.
pub type Allow<'a> = &'a dyn Fn(&str, &FuncType) -> bool;

/// The definitions of one session, and where the next ones go.
#[derive(Clone, Debug)]
pub struct Session {
    layout: Layout,
    symbols: BTreeMap<String, Symbol>,
    next_slot: u32,
    data_end: u32,
}

impl Session {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            symbols: BTreeMap::new(),
            next_slot: 1,
            data_end: layout.data_base,
        }
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.get(name)
    }

    pub fn symbols(&self) -> impl Iterator<Item = (&str, &Symbol)> {
        self.symbols
            .iter()
            .map(|(name, symbol)| (name.as_str(), symbol))
    }

    /// The table slots in use, slot 0 included.
    pub fn table_size(&self) -> u32 {
        self.next_slot
    }

    pub fn data_end(&self) -> u32 {
        self.data_end
    }

    /// Links `object`; `allow` decides which `kai` imports it may use.
    pub fn link(&self, object: &[u8], allow: Allow) -> Result<Linked, Error> {
        link::link(self, &object::Object::parse(object)?, allow)
    }

    /// Records what `linked` defined, once its entry has run.
    pub fn commit(&mut self, linked: &Linked) {
        for def in &linked.defs {
            self.symbols.insert(def.name.clone(), def.symbol.clone());
        }
        self.next_slot = self.next_slot.max(linked.table_size);
        self.data_end = self.data_end.max(linked.data_end);
    }
}
