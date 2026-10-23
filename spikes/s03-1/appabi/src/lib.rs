//! Spike `appabi` (ADR 0003 section 2, spike S03-1): the synchronous canonical
//! ABI, lifting and lowering over a [`Guest`] that gives memory, `cabi_realloc` and
//! core calls. No engine types appear here; the oracle tests bind it to wasmi.
//!
//! Covered: bool, every integer, f32, f64, char, string (UTF-8), list, record,
//! tuple, variant, enum, option, result, flags, and resources the guest exports,
//! with own and borrow. Flattening follows CanonicalABI.md: at most 16 flat
//! parameters (more go through memory) and 1 flat result (more come back by
//! pointer), then `cabi_post_<export>` once the result is copied out.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
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
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap>;
    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Trap>;
    fn realloc(&mut self, old: u32, old_size: u32, align: u32, size: u32) -> Result<u32, Trap>;
    fn call(&mut self, export: &str, args: &[Core]) -> Result<Vec<Core>, Trap>;
    /// The resource representation behind handle `h`, for a borrow.
    fn rep(&mut self, h: u32) -> Result<u32, Trap>;
}

/// One exported function: its core name and its component type.
#[derive(Clone, Debug)]
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
    let value = if flatten(ty).len() > MAX_FLAT_RESULTS {
        let at = match results.as_slice() {
            [Core::I32(at)] => *at as u32,
            _ => return Err(trap("expected a result pointer")),
        };
        load(guest, ty, at)?
    } else {
        lift(guest, ty, &mut results.into_iter())?
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

fn discriminant(cases: usize) -> Ty {
    match cases {
        0..=0x100 => Ty::U8,
        0x101..=0x1_0000 => Ty::U16,
        _ => Ty::U32,
    }
}

fn flags_ty(n: usize) -> Ty {
    match n {
        0..=8 => Ty::U8,
        9..=16 => Ty::U16,
        _ => Ty::U32,
    }
}

/// Alignment in linear memory.
pub fn align(ty: &Ty) -> u32 {
    match ty {
        Ty::Bool | Ty::U8 | Ty::S8 => 1,
        Ty::U16 | Ty::S16 => 2,
        Ty::U32 | Ty::S32 | Ty::F32 | Ty::Char | Ty::Own | Ty::Borrow => 4,
        Ty::U64 | Ty::S64 | Ty::F64 => 8,
        Ty::String | Ty::List(_) => 4,
        Ty::Record(fields) => fields.iter().map(|(_, t)| align(t)).max().unwrap_or(1),
        Ty::Tuple(types) => types.iter().map(align).max().unwrap_or(1),
        Ty::Variant(cases) => {
            let payload = cases.iter().filter_map(|(_, t)| t.as_ref()).map(align);
            payload.fold(align(&discriminant(cases.len())), u32::max)
        }
        Ty::Enum(cases) => align(&discriminant(cases.len())),
        Ty::Option(t) => align(&option_variant(t)),
        Ty::Result(ok, err) => align(&result_variant(ok, err)),
        Ty::Flags(names) => align(&flags_ty(names.len())),
    }
}

fn align_to(at: u32, align: u32) -> u32 {
    at.div_ceil(align) * align
}

/// Size in linear memory.
pub fn size(ty: &Ty) -> u32 {
    match ty {
        Ty::Bool | Ty::U8 | Ty::S8 => 1,
        Ty::U16 | Ty::S16 => 2,
        Ty::U32 | Ty::S32 | Ty::F32 | Ty::Char | Ty::Own | Ty::Borrow => 4,
        Ty::U64 | Ty::S64 | Ty::F64 => 8,
        Ty::String | Ty::List(_) => 8,
        Ty::Record(fields) => fields_size(fields.iter().map(|(_, t)| t), align(ty)),
        Ty::Tuple(types) => fields_size(types.iter(), align(ty)),
        Ty::Variant(cases) => {
            let disc = size(&discriminant(cases.len()));
            let payload = cases.iter().filter_map(|(_, t)| t.as_ref());
            let max_align = payload.clone().map(align).max().unwrap_or(1);
            let max_size = payload.map(size).max().unwrap_or(0);
            align_to(align_to(disc, max_align) + max_size, align(ty))
        }
        Ty::Enum(cases) => size(&discriminant(cases.len())),
        Ty::Option(t) => size(&option_variant(t)),
        Ty::Result(ok, err) => size(&result_variant(ok, err)),
        Ty::Flags(names) => size(&flags_ty(names.len())),
    }
}

fn fields_size<'a>(types: impl Iterator<Item = &'a Ty>, whole: u32) -> u32 {
    let end = types.fold(0, |at, t| align_to(at, align(t)) + size(t));
    align_to(end, whole)
}

fn option_variant(t: &Ty) -> Ty {
    Ty::Variant(vec![
        ("none".into(), None),
        ("some".into(), Some(t.clone())),
    ])
}

fn result_variant(ok: &Option<Box<Ty>>, err: &Option<Box<Ty>>) -> Ty {
    Ty::Variant(vec![
        ("ok".into(), ok.as_deref().cloned()),
        ("error".into(), err.as_deref().cloned()),
    ])
}

fn flatten(ty: &Ty) -> Vec<Flat> {
    match ty {
        Ty::Bool
        | Ty::U8
        | Ty::U16
        | Ty::U32
        | Ty::S8
        | Ty::S16
        | Ty::S32
        | Ty::Char
        | Ty::Enum(_)
        | Ty::Flags(_)
        | Ty::Own
        | Ty::Borrow => vec![Flat::I32],
        Ty::U64 | Ty::S64 => vec![Flat::I64],
        Ty::F32 => vec![Flat::F32],
        Ty::F64 => vec![Flat::F64],
        Ty::String | Ty::List(_) => vec![Flat::I32, Flat::I32],
        Ty::Record(fields) => fields.iter().flat_map(|(_, t)| flatten(t)).collect(),
        Ty::Tuple(types) => types.iter().flat_map(flatten).collect(),
        Ty::Variant(cases) => {
            let mut joined: Vec<Flat> = Vec::new();
            for payload in cases.iter().filter_map(|(_, t)| t.as_ref()) {
                for (i, f) in flatten(payload).into_iter().enumerate() {
                    match joined.get_mut(i) {
                        Some(slot) => *slot = join(*slot, f),
                        None => joined.push(f),
                    }
                }
            }
            let mut flat = vec![Flat::I32];
            flat.extend(joined);
            flat
        }
        Ty::Option(t) => flatten(&option_variant(t)),
        Ty::Result(ok, err) => flatten(&result_variant(ok, err)),
    }
}

fn join(a: Flat, b: Flat) -> Flat {
    match (a, b) {
        (a, b) if a == b => a,
        (Flat::I32, Flat::F32) | (Flat::F32, Flat::I32) => Flat::I32,
        _ => Flat::I64,
    }
}

/// `value` of `ty` as core values, appended to `out`.
fn lower(g: &mut dyn Guest, value: &Val, ty: &Ty, out: &mut Vec<Core>) -> Result<(), Trap> {
    match (value, ty) {
        (Val::Bool(b), Ty::Bool) => out.push(Core::I32(i32::from(*b))),
        (Val::U8(n), Ty::U8) => out.push(Core::I32(i32::from(*n))),
        (Val::U16(n), Ty::U16) => out.push(Core::I32(i32::from(*n))),
        (Val::U32(n), Ty::U32) => out.push(Core::I32(*n as i32)),
        (Val::S8(n), Ty::S8) => out.push(Core::I32(i32::from(*n))),
        (Val::S16(n), Ty::S16) => out.push(Core::I32(i32::from(*n))),
        (Val::S32(n), Ty::S32) => out.push(Core::I32(*n)),
        (Val::U64(n), Ty::U64) => out.push(Core::I64(*n as i64)),
        (Val::S64(n), Ty::S64) => out.push(Core::I64(*n)),
        (Val::F32(n), Ty::F32) => out.push(Core::F32(canon_f32(*n))),
        (Val::F64(n), Ty::F64) => out.push(Core::F64(canon_f64(*n))),
        (Val::Char(c), Ty::Char) => out.push(Core::I32(*c as i32)),
        (Val::Enum(i), Ty::Enum(_)) => out.push(Core::I32(*i as i32)),
        (Val::Flags(set), Ty::Flags(_)) => out.push(Core::I32(flag_bits(set) as i32)),
        (Val::Own(h), Ty::Own) => out.push(Core::I32(*h as i32)),
        // The guest defines the resource, so a borrow passes its representation.
        (Val::Borrow(h), Ty::Borrow) => out.push(Core::I32(g.rep(*h)? as i32)),
        (Val::String(s), Ty::String) => {
            let at = write_bytes(g, s.as_bytes(), 1)?;
            out.extend([Core::I32(at as i32), Core::I32(s.len() as i32)]);
        }
        (Val::List(items), Ty::List(elem)) => {
            let at = store_list(g, items, elem)?;
            out.extend([Core::I32(at as i32), Core::I32(items.len() as i32)]);
        }
        (Val::Record(vals), Ty::Record(fields)) if vals.len() == fields.len() => {
            for (v, (_, t)) in vals.iter().zip(fields) {
                lower(g, v, t, out)?;
            }
        }
        (Val::Tuple(vals), Ty::Tuple(types)) if vals.len() == types.len() => {
            for (v, t) in vals.iter().zip(types) {
                lower(g, v, t, out)?;
            }
        }
        (Val::Variant(case, payload), Ty::Variant(_)) => {
            lower_variant(g, *case, payload.as_deref(), ty, out)?;
        }
        (Val::Option(o), Ty::Option(t)) => {
            let (case, payload) = match o {
                None => (0, None),
                Some(v) => (1, Some(v.as_ref())),
            };
            lower_variant(g, case, payload, &option_variant(t), out)?;
        }
        (Val::Result(r), Ty::Result(ok, err)) => {
            let (case, payload) = match r {
                Ok(v) => (0, v.as_deref()),
                Err(v) => (1, v.as_deref()),
            };
            lower_variant(g, case, payload, &result_variant(ok, err), out)?;
        }
        _ => return Err(trap(format!("value {value:?} is not a {ty:?}"))),
    }
    Ok(())
}

fn lower_variant(
    g: &mut dyn Guest,
    case: u32,
    payload: Option<&Val>,
    ty: &Ty,
    out: &mut Vec<Core>,
) -> Result<(), Trap> {
    let Ty::Variant(cases) = ty else {
        return Err(trap("not a variant"));
    };
    let slots = flatten(ty);
    out.push(Core::I32(case as i32));
    let (_, case_ty) = cases
        .get(case as usize)
        .ok_or_else(|| trap("no such case"))?;
    let mut written = Vec::new();
    match (payload, case_ty) {
        (Some(v), Some(t)) => lower(g, v, t, &mut written)?,
        (None, None) => {}
        _ => return Err(trap("a case payload does not match its type")),
    }
    for (i, slot) in slots[1..].iter().enumerate() {
        out.push(match written.get(i) {
            Some(v) => coerce(*v, *slot),
            None => zero(*slot),
        });
    }
    Ok(())
}

fn zero(slot: Flat) -> Core {
    match slot {
        Flat::I32 => Core::I32(0),
        Flat::I64 => Core::I64(0),
        Flat::F32 => Core::F32(0.0),
        Flat::F64 => Core::F64(0.0),
    }
}

/// A case's core value in the joined slot's type.
fn coerce(v: Core, slot: Flat) -> Core {
    match (v, slot) {
        (Core::F32(f), Flat::I32) => Core::I32(f.to_bits() as i32),
        (Core::I32(i), Flat::I64) => Core::I64(i64::from(i as u32)),
        (Core::F32(f), Flat::I64) => Core::I64(i64::from(f.to_bits())),
        (Core::F64(f), Flat::I64) => Core::I64(f.to_bits() as i64),
        (v, _) => v,
    }
}

/// A joined slot's value back in the case's own type.
fn uncoerce(v: Core, want: Flat) -> Result<Core, Trap> {
    Ok(match (v, want) {
        (Core::I32(i), Flat::F32) => Core::F32(f32::from_bits(i as u32)),
        (Core::I64(i), Flat::I32) => Core::I32(i as i32),
        (Core::I64(i), Flat::F32) => Core::F32(f32::from_bits(i as u32)),
        (Core::I64(i), Flat::F64) => Core::F64(f64::from_bits(i as u64)),
        (Core::I32(_), Flat::I32)
        | (Core::I64(_), Flat::I64)
        | (Core::F32(_), Flat::F32)
        | (Core::F64(_), Flat::F64) => v,
        _ => return Err(trap("a flat value has the wrong type")),
    })
}

/// NaNs are canonicalised, as the canonical ABI's lowering allows.
fn canon_f32(f: f32) -> f32 {
    if f.is_nan() {
        f32::from_bits(0x7fc0_0000)
    } else {
        f
    }
}

fn canon_f64(f: f64) -> f64 {
    if f.is_nan() {
        f64::from_bits(0x7ff8_0000_0000_0000)
    } else {
        f
    }
}

fn flag_bits(set: &[u32]) -> u32 {
    set.iter().fold(0, |bits, i| bits | (1 << i))
}

fn flag_set(bits: u32, n: usize) -> Result<Vec<u32>, Trap> {
    if n < 32 && bits >> n != 0 {
        return Err(trap("a flag beyond the declared ones is set"));
    }
    Ok((0..n as u32).filter(|i| bits & (1 << i) != 0).collect())
}

fn write_bytes(g: &mut dyn Guest, bytes: &[u8], align: u32) -> Result<u32, Trap> {
    let len = u32::try_from(bytes.len()).map_err(|_| trap("too long"))?;
    let at = g.realloc(0, 0, align, len)?;
    if at % align != 0 {
        return Err(trap("realloc returned a misaligned pointer"));
    }
    g.write(at, bytes)?;
    Ok(at)
}

fn store_list(g: &mut dyn Guest, items: &[Val], elem: &Ty) -> Result<u32, Trap> {
    let (each, len) = (size(elem), items.len() as u32);
    let total = each.checked_mul(len).ok_or_else(|| trap("list too long"))?;
    let at = g.realloc(0, 0, align(elem), total)?;
    for (i, item) in items.iter().enumerate() {
        store(g, item, elem, at + i as u32 * each)?;
    }
    Ok(at)
}

/// Writes `value` of `ty` at `at`.
fn store(g: &mut dyn Guest, value: &Val, ty: &Ty, at: u32) -> Result<(), Trap> {
    match (value, ty) {
        (Val::Record(vals), Ty::Record(fields)) if vals.len() == fields.len() => {
            store_fields(g, vals, fields.iter().map(|(_, t)| t), at)
        }
        (Val::Tuple(vals), Ty::Tuple(types)) if vals.len() == types.len() => {
            store_fields(g, vals, types.iter(), at)
        }
        (Val::Variant(case, payload), Ty::Variant(_)) => {
            store_variant(g, *case, payload.as_deref(), ty, at)
        }
        (Val::Option(o), Ty::Option(t)) => {
            let (case, payload) = match o {
                None => (0, None),
                Some(v) => (1, Some(v.as_ref())),
            };
            store_variant(g, case, payload, &option_variant(t), at)
        }
        (Val::Result(r), Ty::Result(ok, err)) => {
            let (case, payload) = match r {
                Ok(v) => (0, v.as_deref()),
                Err(v) => (1, v.as_deref()),
            };
            store_variant(g, case, payload, &result_variant(ok, err), at)
        }
        (Val::Enum(i), Ty::Enum(cases)) => {
            store_int(g, u64::from(*i), &discriminant(cases.len()), at)
        }
        (Val::Flags(set), Ty::Flags(names)) => {
            store_int(g, u64::from(flag_bits(set)), &flags_ty(names.len()), at)
        }
        (Val::String(s), Ty::String) => {
            let ptr = write_bytes(g, s.as_bytes(), 1)?;
            g.write(at, &ptr.to_le_bytes())?;
            g.write(at + 4, &(s.len() as u32).to_le_bytes())
        }
        (Val::List(items), Ty::List(elem)) => {
            let ptr = store_list(g, items, elem)?;
            g.write(at, &ptr.to_le_bytes())?;
            g.write(at + 4, &(items.len() as u32).to_le_bytes())
        }
        _ => {
            // Scalars: lower to one core value and write it at its width.
            let mut flat = Vec::new();
            lower(g, value, ty, &mut flat)?;
            let bytes: Vec<u8> = match (flat.as_slice(), size(ty)) {
                ([Core::I32(v)], 1) => vec![*v as u8],
                ([Core::I32(v)], 2) => (*v as u16).to_le_bytes().to_vec(),
                ([Core::I32(v)], 4) => v.to_le_bytes().to_vec(),
                ([Core::I64(v)], 8) => v.to_le_bytes().to_vec(),
                ([Core::F32(v)], 4) => v.to_bits().to_le_bytes().to_vec(),
                ([Core::F64(v)], 8) => v.to_bits().to_le_bytes().to_vec(),
                _ => return Err(trap(format!("cannot store {value:?} as {ty:?}"))),
            };
            g.write(at, &bytes)
        }
    }
}

fn store_fields<'a>(
    g: &mut dyn Guest,
    vals: &[Val],
    types: impl Iterator<Item = &'a Ty>,
    at: u32,
) -> Result<(), Trap> {
    let mut offset = 0;
    for (v, t) in vals.iter().zip(types) {
        offset = align_to(offset, align(t));
        store(g, v, t, at + offset)?;
        offset += size(t);
    }
    Ok(())
}

fn store_int(g: &mut dyn Guest, v: u64, ty: &Ty, at: u32) -> Result<(), Trap> {
    let bytes = v.to_le_bytes();
    g.write(at, &bytes[..size(ty) as usize])
}

fn payload_offset(ty: &Ty) -> u32 {
    let Ty::Variant(cases) = ty else { return 0 };
    let max_align = cases
        .iter()
        .filter_map(|(_, t)| t.as_ref())
        .map(align)
        .max()
        .unwrap_or(1);
    align_to(size(&discriminant(cases.len())), max_align)
}

fn store_variant(
    g: &mut dyn Guest,
    case: u32,
    payload: Option<&Val>,
    ty: &Ty,
    at: u32,
) -> Result<(), Trap> {
    let Ty::Variant(cases) = ty else {
        return Err(trap("not a variant"));
    };
    let (_, case_ty) = cases
        .get(case as usize)
        .ok_or_else(|| trap("no such case"))?;
    store_int(g, u64::from(case), &discriminant(cases.len()), at)?;
    match (payload, case_ty) {
        (Some(v), Some(t)) => store(g, v, t, at + payload_offset(ty)),
        (None, None) => Ok(()),
        _ => Err(trap("a case payload does not match its type")),
    }
}

/// The next flat value, in the type `want` of the value being lifted.
fn take(slots: &mut dyn Iterator<Item = Core>, want: Flat) -> Result<Core, Trap> {
    let v = slots.next().ok_or_else(|| trap("too few flat values"))?;
    uncoerce(v, want)
}

/// Reads a value of `ty` from flat core values.
fn lift(g: &mut dyn Guest, ty: &Ty, slots: &mut dyn Iterator<Item = Core>) -> Result<Val, Trap> {
    let i32_of = |v: Core| match v {
        Core::I32(i) => i,
        _ => 0,
    };
    Ok(match ty {
        Ty::Bool => Val::Bool(i32_of(take(slots, Flat::I32)?) != 0),
        Ty::U8 => Val::U8(i32_of(take(slots, Flat::I32)?) as u8),
        Ty::U16 => Val::U16(i32_of(take(slots, Flat::I32)?) as u16),
        Ty::U32 => Val::U32(i32_of(take(slots, Flat::I32)?) as u32),
        Ty::S8 => Val::S8(i32_of(take(slots, Flat::I32)?) as i8),
        Ty::S16 => Val::S16(i32_of(take(slots, Flat::I32)?) as i16),
        Ty::S32 => Val::S32(i32_of(take(slots, Flat::I32)?)),
        Ty::U64 | Ty::S64 => match take(slots, Flat::I64)? {
            Core::I64(v) if *ty == Ty::U64 => Val::U64(v as u64),
            Core::I64(v) => Val::S64(v),
            _ => unreachable!(),
        },
        Ty::F32 => match take(slots, Flat::F32)? {
            Core::F32(f) => Val::F32(canon_f32(f)),
            _ => unreachable!(),
        },
        Ty::F64 => match take(slots, Flat::F64)? {
            Core::F64(f) => Val::F64(canon_f64(f)),
            _ => unreachable!(),
        },
        Ty::Char => Val::Char(char_of(i32_of(take(slots, Flat::I32)?) as u32)?),
        Ty::Enum(cases) => {
            let i = i32_of(take(slots, Flat::I32)?) as u32;
            if i as usize >= cases.len() {
                return Err(trap("an enum case out of range"));
            }
            Val::Enum(i)
        }
        Ty::Flags(names) => Val::Flags(flag_set(
            i32_of(take(slots, Flat::I32)?) as u32,
            names.len(),
        )?),
        Ty::Own => Val::Own(i32_of(take(slots, Flat::I32)?) as u32),
        Ty::Borrow => Val::Borrow(i32_of(take(slots, Flat::I32)?) as u32),
        Ty::String => {
            let (at, len) = (
                i32_of(take(slots, Flat::I32)?) as u32,
                i32_of(take(slots, Flat::I32)?) as u32,
            );
            Val::String(read_string(g, at, len)?)
        }
        Ty::List(elem) => {
            let (at, len) = (
                i32_of(take(slots, Flat::I32)?) as u32,
                i32_of(take(slots, Flat::I32)?) as u32,
            );
            Val::List(load_list(g, elem, at, len)?)
        }
        Ty::Record(fields) => Val::Record(
            fields
                .iter()
                .map(|(_, t)| lift(g, t, slots))
                .collect::<Result<_, _>>()?,
        ),
        Ty::Tuple(types) => Val::Tuple(
            types
                .iter()
                .map(|t| lift(g, t, slots))
                .collect::<Result<_, _>>()?,
        ),
        Ty::Variant(_) | Ty::Option(_) | Ty::Result(..) => {
            let variant = as_variant(ty);
            let Ty::Variant(cases) = &variant else {
                unreachable!()
            };
            let flat = flatten(&variant);
            let case = i32_of(take(slots, Flat::I32)?) as u32;
            let joined: Vec<Core> = flat[1..]
                .iter()
                .map(|_| slots.next().ok_or_else(|| trap("too few flat values")))
                .collect::<Result<_, _>>()?;
            let (_, case_ty) = cases
                .get(case as usize)
                .ok_or_else(|| trap("a variant case out of range"))?;
            let payload = match case_ty {
                Some(t) => {
                    let want = flatten(t);
                    let own: Vec<Core> = joined
                        .iter()
                        .zip(&want)
                        .map(|(v, w)| uncoerce(*v, *w))
                        .collect::<Result<_, _>>()?;
                    Some(Box::new(lift(g, t, &mut own.into_iter())?))
                }
                None => None,
            };
            from_variant(ty, case, payload)
        }
    })
}

fn as_variant(ty: &Ty) -> Ty {
    match ty {
        Ty::Option(t) => option_variant(t),
        Ty::Result(ok, err) => result_variant(ok, err),
        other => other.clone(),
    }
}

fn from_variant(ty: &Ty, case: u32, payload: Option<Box<Val>>) -> Val {
    match ty {
        Ty::Option(_) if case == 0 => Val::Option(None),
        Ty::Option(_) => Val::Option(payload),
        Ty::Result(..) if case == 0 => Val::Result(Ok(payload)),
        Ty::Result(..) => Val::Result(Err(payload)),
        _ => Val::Variant(case, payload),
    }
}

fn char_of(n: u32) -> Result<char, Trap> {
    char::from_u32(n).ok_or_else(|| trap("not a Unicode scalar value"))
}

fn read_string(g: &mut dyn Guest, at: u32, len: u32) -> Result<String, Trap> {
    let bytes = g.read(at, len)?;
    String::from_utf8(bytes).map_err(|_| trap("a string is not UTF-8"))
}

fn load_list(g: &mut dyn Guest, elem: &Ty, at: u32, len: u32) -> Result<Vec<Val>, Trap> {
    let each = size(elem);
    if at % align(elem) != 0 {
        return Err(trap("a list is misaligned"));
    }
    (0..len).map(|i| load(g, elem, at + i * each)).collect()
}

/// Reads a value of `ty` from `at`.
fn load(g: &mut dyn Guest, ty: &Ty, at: u32) -> Result<Val, Trap> {
    let int = |g: &mut dyn Guest, n: u32| -> Result<u64, Trap> {
        let bytes = g.read(at, n)?;
        let mut word = [0u8; 8];
        word[..bytes.len()].copy_from_slice(&bytes);
        Ok(u64::from_le_bytes(word))
    };
    Ok(match ty {
        Ty::Bool => Val::Bool(int(g, 1)? != 0),
        Ty::U8 => Val::U8(int(g, 1)? as u8),
        Ty::S8 => Val::S8(int(g, 1)? as i8),
        Ty::U16 => Val::U16(int(g, 2)? as u16),
        Ty::S16 => Val::S16(int(g, 2)? as i16),
        Ty::U32 => Val::U32(int(g, 4)? as u32),
        Ty::S32 => Val::S32(int(g, 4)? as i32),
        Ty::U64 => Val::U64(int(g, 8)?),
        Ty::S64 => Val::S64(int(g, 8)? as i64),
        Ty::F32 => Val::F32(canon_f32(f32::from_bits(int(g, 4)? as u32))),
        Ty::F64 => Val::F64(canon_f64(f64::from_bits(int(g, 8)?))),
        Ty::Char => Val::Char(char_of(int(g, 4)? as u32)?),
        Ty::Own => Val::Own(int(g, 4)? as u32),
        Ty::Borrow => Val::Borrow(int(g, 4)? as u32),
        Ty::Enum(cases) => {
            let i = int(g, size(ty))? as u32;
            if i as usize >= cases.len() {
                return Err(trap("an enum case out of range"));
            }
            Val::Enum(i)
        }
        Ty::Flags(names) => Val::Flags(flag_set(int(g, size(ty))? as u32, names.len())?),
        Ty::String => {
            let (ptr, len) = (int(g, 4)? as u32, load_u32(g, at + 4)?);
            Val::String(read_string(g, ptr, len)?)
        }
        Ty::List(elem) => {
            let (ptr, len) = (int(g, 4)? as u32, load_u32(g, at + 4)?);
            Val::List(load_list(g, elem, ptr, len)?)
        }
        Ty::Record(fields) => Val::Record(load_fields(g, fields.iter().map(|(_, t)| t), at)?),
        Ty::Tuple(types) => Val::Tuple(load_fields(g, types.iter(), at)?),
        Ty::Variant(_) | Ty::Option(_) | Ty::Result(..) => {
            let variant = as_variant(ty);
            let Ty::Variant(cases) = &variant else {
                unreachable!()
            };
            let case = int(g, size(&discriminant(cases.len())))? as u32;
            let (_, case_ty) = cases
                .get(case as usize)
                .ok_or_else(|| trap("a variant case out of range"))?;
            let payload = match case_ty {
                Some(t) => Some(Box::new(load(g, t, at + payload_offset(&variant))?)),
                None => None,
            };
            from_variant(ty, case, payload)
        }
    })
}

fn load_u32(g: &mut dyn Guest, at: u32) -> Result<u32, Trap> {
    let bytes = g.read(at, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn load_fields<'a>(
    g: &mut dyn Guest,
    types: impl Iterator<Item = &'a Ty>,
    at: u32,
) -> Result<Vec<Val>, Trap> {
    let mut offset = 0;
    let mut out = Vec::new();
    for t in types {
        offset = align_to(offset, align(t));
        out.push(load(g, t, at + offset)?);
        offset += size(t);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_alignments_follow_the_spec() {
        let point = Ty::Record(vec![("x".into(), Ty::S32), ("y".into(), Ty::S32)]);
        assert_eq!((size(&point), align(&point)), (8, 4));
        let tuple = Ty::Tuple(vec![Ty::U8, Ty::String, Ty::S64]);
        assert_eq!((size(&tuple), align(&tuple)), (24, 8));
        let opt = Ty::Option(Box::new(Ty::String));
        assert_eq!((size(&opt), align(&opt)), (12, 4));
        let res = Ty::Result(Some(Box::new(Ty::U32)), Some(Box::new(Ty::String)));
        assert_eq!(flatten(&res), vec![Flat::I32, Flat::I32, Flat::I32]);
        let mixed = Ty::Variant(vec![
            ("a".into(), Some(Ty::F64)),
            ("b".into(), Some(Ty::U32)),
            ("c".into(), None),
        ]);
        assert_eq!(flatten(&mixed), vec![Flat::I32, Flat::I64]);
        assert_eq!(
            size(&Ty::Flags((0..9).map(|i| format!("f{i}")).collect())),
            2
        );
    }
}
