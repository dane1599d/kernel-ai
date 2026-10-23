//! Where each type sits: alignment and size in linear memory, and its flat core
//! values (CanonicalABI.md, "Despecialization", "Alignment", "Element Size" and
//! "Flattening").

use crate::{Flat, Ty};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

pub(crate) fn discriminant(cases: usize) -> Ty {
    match cases {
        0..=0x100 => Ty::U8,
        0x101..=0x1_0000 => Ty::U16,
        _ => Ty::U32,
    }
}

pub(crate) fn flags_ty(n: usize) -> Ty {
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

pub(crate) fn align_to(at: u32, align: u32) -> u32 {
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

pub(crate) fn fields_size<'a>(types: impl Iterator<Item = &'a Ty>, whole: u32) -> u32 {
    let end = types.fold(0, |at, t| align_to(at, align(t)) + size(t));
    align_to(end, whole)
}

pub(crate) fn option_variant(t: &Ty) -> Ty {
    Ty::Variant(vec![
        ("none".into(), None),
        ("some".into(), Some(t.clone())),
    ])
}

pub(crate) fn result_variant(ok: &Option<Box<Ty>>, err: &Option<Box<Ty>>) -> Ty {
    Ty::Variant(vec![
        ("ok".into(), ok.as_deref().cloned()),
        ("error".into(), err.as_deref().cloned()),
    ])
}

pub(crate) fn flatten(ty: &Ty) -> Vec<Flat> {
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

pub(crate) fn join(a: Flat, b: Flat) -> Flat {
    match (a, b) {
        (a, b) if a == b => a,
        (Flat::I32, Flat::F32) | (Flat::F32, Flat::I32) => Flat::I32,
        _ => Flat::I64,
    }
}

pub(crate) fn payload_offset(ty: &Ty) -> u32 {
    let Ty::Variant(cases) = ty else { return 0 };
    let max_align = cases
        .iter()
        .filter_map(|(_, t)| t.as_ref())
        .map(align)
        .max()
        .unwrap_or(1);
    align_to(size(&discriminant(cases.len())), max_align)
}

/// `at + by`, or a trap if it runs past the 32-bit address space.
pub(crate) fn offset(at: u32, by: u32) -> Result<u32, crate::Trap> {
    at.checked_add(by)
        .ok_or_else(|| crate::trap("an address overflows"))
}
