//! Lowering: host values into the guest, as flat core values or written into
//! linear memory through `cabi_realloc`.

use crate::layout::{
    align, align_to, discriminant, flags_ty, flatten, offset, option_variant, payload_offset,
    result_variant, size,
};
use crate::{Core, Flat, Guest, Trap, Ty, Val, trap};
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

/// `value` of `ty` as core values, appended to `out`.
pub(crate) fn lower(
    g: &mut dyn Guest,
    value: &Val,
    ty: &Ty,
    out: &mut Vec<Core>,
) -> Result<(), Trap> {
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
        (Val::Bytes(b), Ty::List(elem)) if **elem == Ty::U8 => {
            let at = write_bytes(g, b, 1)?;
            out.extend([Core::I32(at as i32), Core::I32(b.len() as i32)]);
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

pub(crate) fn lower_variant(
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

pub(crate) fn zero(slot: Flat) -> Core {
    match slot {
        Flat::I32 => Core::I32(0),
        Flat::I64 => Core::I64(0),
        Flat::F32 => Core::F32(0.0),
        Flat::F64 => Core::F64(0.0),
    }
}

/// A case's core value in the joined slot's type.
pub(crate) fn coerce(v: Core, slot: Flat) -> Core {
    match (v, slot) {
        (Core::F32(f), Flat::I32) => Core::I32(f.to_bits() as i32),
        (Core::I32(i), Flat::I64) => Core::I64(i64::from(i as u32)),
        (Core::F32(f), Flat::I64) => Core::I64(i64::from(f.to_bits())),
        (Core::F64(f), Flat::I64) => Core::I64(f.to_bits() as i64),
        (v, _) => v,
    }
}

/// NaNs are canonicalised, as the canonical ABI's lowering allows.
pub(crate) fn canon_f32(f: f32) -> f32 {
    if f.is_nan() {
        f32::from_bits(0x7fc0_0000)
    } else {
        f
    }
}

pub(crate) fn canon_f64(f: f64) -> f64 {
    if f.is_nan() {
        f64::from_bits(0x7ff8_0000_0000_0000)
    } else {
        f
    }
}

pub(crate) fn flag_bits(set: &[u32]) -> u32 {
    set.iter().fold(0, |bits, i| bits | (1 << i))
}

pub(crate) fn write_bytes(g: &mut dyn Guest, bytes: &[u8], align: u32) -> Result<u32, Trap> {
    let len = u32::try_from(bytes.len()).map_err(|_| trap("too long"))?;
    let at = g.realloc(0, 0, align, len)?;
    if !at.is_multiple_of(align) {
        return Err(trap("realloc returned a misaligned pointer"));
    }
    g.write(at, bytes)?;
    Ok(at)
}

pub(crate) fn store_list(g: &mut dyn Guest, items: &[Val], elem: &Ty) -> Result<u32, Trap> {
    let (each, len) = (size(elem), items.len() as u32);
    let total = each.checked_mul(len).ok_or_else(|| trap("list too long"))?;
    let at = g.realloc(0, 0, align(elem), total)?;
    offset(at, total)?;
    for (i, item) in items.iter().enumerate() {
        store(g, item, elem, at + i as u32 * each)?;
    }
    Ok(at)
}

/// Writes `value` of `ty` at `at`.
pub(crate) fn store(g: &mut dyn Guest, value: &Val, ty: &Ty, at: u32) -> Result<(), Trap> {
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
            g.write(offset(at, 4)?, &(s.len() as u32).to_le_bytes())
        }
        (Val::Bytes(b), Ty::List(elem)) if **elem == Ty::U8 => {
            let ptr = write_bytes(g, b, 1)?;
            g.write(at, &ptr.to_le_bytes())?;
            g.write(offset(at, 4)?, &(b.len() as u32).to_le_bytes())
        }
        (Val::List(items), Ty::List(elem)) => {
            let ptr = store_list(g, items, elem)?;
            g.write(at, &ptr.to_le_bytes())?;
            g.write(offset(at, 4)?, &(items.len() as u32).to_le_bytes())
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

pub(crate) fn store_fields<'a>(
    g: &mut dyn Guest,
    vals: &[Val],
    types: impl Iterator<Item = &'a Ty>,
    at: u32,
) -> Result<(), Trap> {
    let mut field = 0;
    for (v, t) in vals.iter().zip(types) {
        field = align_to(field, align(t));
        store(g, v, t, offset(at, field)?)?;
        field += size(t);
    }
    Ok(())
}

pub(crate) fn store_int(g: &mut dyn Guest, v: u64, ty: &Ty, at: u32) -> Result<(), Trap> {
    let bytes = v.to_le_bytes();
    g.write(at, &bytes[..size(ty) as usize])
}

pub(crate) fn store_variant(
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
        (Some(v), Some(t)) => store(g, v, t, offset(at, payload_offset(ty))?),
        (None, None) => Ok(()),
        _ => Err(trap("a case payload does not match its type")),
    }
}
