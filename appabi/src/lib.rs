//! The app ABI (ADR 0003 section 2): the synchronous canonical ABI, lifting and
//! lowering over a [`Guest`] that gives linear memory, `cabi_realloc` and core
//! calls. No engine types appear here; wasmhost stays the only crate that names
//! wasmi. Spike S03-1 checked this code against Wasmtime 50's component runtime:
//! 0 differences over 10,000 generated calls (`spikes/s03-1`, docs/decisions.md).
//!
//! Covered: bool, every integer, f32, f64, char, string (UTF-8), list, record,
//! tuple, variant, enum, option, result, flags, and resources the guest exports,
//! with own and borrow. Flattening follows CanonicalABI.md: at most 16 flat
//! parameters (more go through memory) and 1 flat result (more come back by
//! pointer), then `cabi_post_<export>` once the result is copied out.
//!
//! Everything lifted was written by the guest, so lifting never panics: a bad value
//! is a [`Trap`], and one result may cost the host at most [`MAX_LIFT_UNITS`]
//! (about 4 MiB), however its lists alias or nest.

#![no_std]

extern crate alloc;

mod layout;
mod lift;
mod lower;
pub mod wave;

use layout::flatten;
pub use layout::{align, size};
pub use lift::{Lift, MAX_LIFT_UNITS, VALUE_UNITS};
use lift::{lift, load};
use lower::{lower, store};

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const MAX_FLAT_PARAMS: usize = 16;
pub const MAX_FLAT_RESULTS: usize = 1;

/// A component-level type.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,
    List(Box<Ty>),
    Record(Vec<(String, Ty)>),
    Tuple(Vec<Ty>),
    Variant(Vec<(String, Option<Ty>)>),
    Enum(Vec<String>),
    Option(Box<Ty>),
    Result(Option<Box<Ty>>, Option<Box<Ty>>),
    Flags(Vec<String>),
    /// An owned handle to a resource the guest exports.
    Own,
    /// A borrowed handle to a resource the guest exports.
    Borrow,
}

/// A component-level value.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    S8(i8),
    S16(i16),
    S32(i32),
    S64(i64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),
    List(Vec<Val>),
    /// A `list<u8>`, kept as bytes: a checkpoint is one value, not 64K.
    Bytes(Vec<u8>),
    Record(Vec<Val>),
    Tuple(Vec<Val>),
    Variant(u32, Option<Box<Val>>),
    Enum(u32),
    Option(Option<Box<Val>>),
    Result(Result<Option<Box<Val>>, Option<Box<Val>>>),
    /// Which flags are set, by index.
    Flags(Vec<u32>),
    /// A handle in the host's table.
    Own(u32),
    Borrow(u32),
}

/// A core WebAssembly value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Core {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flat {
    I32,
    I64,
    F32,
    F64,
}

/// Why a call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trap(pub String);

fn trap(why: impl Into<String>) -> Trap {
    Trap(why.into())
}

/// What `appabi` needs from an engine: the guest's linear memory, its
/// `cabi_realloc`, and calls to its core exports.
pub trait Guest {
    /// Exactly `len` bytes at `at`, or a trap when any is outside memory; checked
    /// before anything is allocated.
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap>;
    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Trap>;
    /// The guest's `cabi_realloc`.
    fn realloc(&mut self, old: u32, old_size: u32, align: u32, size: u32) -> Result<u32, Trap>;
    /// Calls the core export `export`.
    fn call(&mut self, export: &str, args: &[Core]) -> Result<Vec<Core>, Trap>;
    /// The resource representation behind handle `h`, for a borrow.
    fn rep(&mut self, h: u32) -> Result<u32, Trap>;
}

/// One exported function: its core name and its component type.
#[derive(Clone, Debug, PartialEq)]
pub struct Func {
    pub export: String,
    pub params: Vec<Ty>,
    pub result: Option<Ty>,
}

/// Calls `func` with `args`, lowered into the guest, and lifts its result.
pub fn call(guest: &mut dyn Guest, func: &Func, args: &[Val]) -> Result<Option<Val>, Trap> {
    if args.len() != func.params.len() {
        return Err(trap("wrong number of arguments"));
    }
    let flat_params: usize = func.params.iter().map(|t| flatten(t).len()).sum();
    let mut core = Vec::new();
    if flat_params > MAX_FLAT_PARAMS {
        let tuple = Ty::Tuple(func.params.clone());
        let at = guest.realloc(0, 0, align(&tuple), size(&tuple))?;
        store(guest, &Val::Tuple(args.to_vec()), &tuple, at)?;
        core.push(Core::I32(at as i32));
    } else {
        for (arg, ty) in args.iter().zip(&func.params) {
            lower(guest, arg, ty, &mut core)?;
        }
    }
    let results = guest.call(&func.export, &core)?;
    let returned = results.clone();
    let Some(ty) = &func.result else {
        return Ok(None);
    };
    let mut cx = Lift::new(guest);
    let value = if flatten(ty).len() > MAX_FLAT_RESULTS {
        let at = match results.as_slice() {
            [Core::I32(at)] => *at as u32,
            _ => return Err(trap("expected a result pointer")),
        };
        load(&mut cx, ty, at)?
    } else {
        lift(&mut cx, ty, &mut results.into_iter())?
    };
    let post = post_name(&func.export);
    // `cabi_post_*` gets the export's own results; a guest without one has
    // nothing to free.
    let _ = guest.call(&post, &returned).ok();
    Ok(Some(value))
}

fn post_name(export: &str) -> String {
    format!("cabi_post_{export}")
}

#[cfg(test)]
mod tests;
