//! Lifting: guest values back to the host, from flat core values or from linear
//! memory. Everything here reads what the guest wrote, so nothing may panic on
//! any bytes it finds; bad values are traps.

use crate::layout::{
    align, align_to, discriminant, flatten, offset, option_variant, payload_offset, result_variant,
    size,
};
use crate::lower::{canon_f32, canon_f64};
use crate::{Core, Flat, Guest, Trap, Ty, Val, trap};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// A joined slot's value back in the case's own type.
pub(crate) fn uncoerce(v: Core, want: Flat) -> Result<Core, Trap> {
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

pub(crate) fn flag_set(bits: u32, n: usize) -> Result<Vec<u32>, Trap> {
    if n < 32 && bits >> n != 0 {
        return Err(trap("a flag beyond the declared ones is set"));
    }
    Ok((0..n as u32).filter(|i| bits & (1 << i) != 0).collect())
}

/// What lifting one result may cost: [`VALUE_UNITS`] per value and 1 per byte
/// read. A guest can point many list entries at the same bytes, or list values
/// that take no memory, so its memory size alone does not bound the host's;
/// this does, to about 4 MiB.
pub const MAX_LIFT_UNITS: u64 = 4 << 20;
/// What one lifted value costs, about its size in host memory.
pub const VALUE_UNITS: u64 = 32;

/// A guest being read, and what is left of the budget for this result.
pub struct Lift<'a> {
    g: &'a mut dyn Guest,
    left: u64,
}

impl<'a> Lift<'a> {
    pub fn new(g: &'a mut dyn Guest) -> Self {
        Self {
            g,
            left: MAX_LIFT_UNITS,
        }
    }

    fn charge(&mut self, units: u64) -> Result<(), Trap> {
        self.left = self
            .left
            .checked_sub(units)
            .ok_or_else(|| trap("a result is too large to lift"))?;
        Ok(())
    }

    /// Exactly `len` bytes at `at`, charged before they are read.
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap> {
        self.charge(u64::from(len))?;
        offset(at, len)?;
        let bytes = self.g.read(at, len)?;
        if bytes.len() != len as usize {
            return Err(trap("a read returned the wrong length"));
        }
        Ok(bytes)
    }
}

/// The next flat value, in the type `want` of the value being lifted.
pub(crate) fn take(slots: &mut dyn Iterator<Item = Core>, want: Flat) -> Result<Core, Trap> {
    let v = slots.next().ok_or_else(|| trap("too few flat values"))?;
    uncoerce(v, want)
}

/// Reads a value of `ty` from flat core values.
pub(crate) fn lift(
    g: &mut Lift<'_>,
    ty: &Ty,
    slots: &mut dyn Iterator<Item = Core>,
) -> Result<Val, Trap> {
    g.charge(VALUE_UNITS)?;
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
            _ => return Err(trap("not an i64")),
        },
        Ty::F32 => match take(slots, Flat::F32)? {
            Core::F32(f) => Val::F32(canon_f32(f)),
            _ => return Err(trap("not an f32")),
        },
        Ty::F64 => match take(slots, Flat::F64)? {
            Core::F64(f) => Val::F64(canon_f64(f)),
            _ => return Err(trap("not an f64")),
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
            list(g, elem, at, len)?
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
                return Err(trap("not a variant"));
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

pub(crate) fn as_variant(ty: &Ty) -> Ty {
    match ty {
        Ty::Option(t) => option_variant(t),
        Ty::Result(ok, err) => result_variant(ok, err),
        other => other.clone(),
    }
}

pub(crate) fn from_variant(ty: &Ty, case: u32, payload: Option<Box<Val>>) -> Val {
    match ty {
        Ty::Option(_) if case == 0 => Val::Option(None),
        Ty::Option(_) => Val::Option(payload),
        Ty::Result(..) if case == 0 => Val::Result(Ok(payload)),
        Ty::Result(..) => Val::Result(Err(payload)),
        _ => Val::Variant(case, payload),
    }
}

pub(crate) fn char_of(n: u32) -> Result<char, Trap> {
    char::from_u32(n).ok_or_else(|| trap("not a Unicode scalar value"))
}

pub(crate) fn read_string(g: &mut Lift<'_>, at: u32, len: u32) -> Result<String, Trap> {
    let bytes = g.read(at, len)?;
    String::from_utf8(bytes).map_err(|_| trap("a string is not UTF-8"))
}

/// A list: `list<u8>` as one [`Val::Bytes`], anything else element by element.
fn list(g: &mut Lift<'_>, elem: &Ty, at: u32, len: u32) -> Result<Val, Trap> {
    match elem {
        Ty::U8 => Ok(Val::Bytes(g.read(at, len)?)),
        _ => Ok(Val::List(load_list(g, elem, at, len)?)),
    }
}

pub(crate) fn load_list(g: &mut Lift<'_>, elem: &Ty, at: u32, len: u32) -> Result<Vec<Val>, Trap> {
    let each = size(elem);
    if !at.is_multiple_of(align(elem)) {
        return Err(trap("a list is misaligned"));
    }
    let total = each
        .checked_mul(len)
        .ok_or_else(|| trap("a list is too long"))?;
    at.checked_add(total)
        .ok_or_else(|| trap("a list runs past memory"))?;
    (0..len).map(|i| load(g, elem, at + i * each)).collect()
}

/// Reads a value of `ty` from `at`.
pub(crate) fn load(g: &mut Lift<'_>, ty: &Ty, at: u32) -> Result<Val, Trap> {
    g.charge(VALUE_UNITS)?;
    let int = |g: &mut Lift<'_>, n: u32| -> Result<u64, Trap> {
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
            let (ptr, len) = (int(g, 4)? as u32, load_u32(g, offset(at, 4)?)?);
            Val::String(read_string(g, ptr, len)?)
        }
        Ty::List(elem) => {
            let (ptr, len) = (int(g, 4)? as u32, load_u32(g, offset(at, 4)?)?);
            list(g, elem, ptr, len)?
        }
        Ty::Record(fields) => Val::Record(load_fields(g, fields.iter().map(|(_, t)| t), at)?),
        Ty::Tuple(types) => Val::Tuple(load_fields(g, types.iter(), at)?),
        Ty::Variant(_) | Ty::Option(_) | Ty::Result(..) => {
            let variant = as_variant(ty);
            let Ty::Variant(cases) = &variant else {
                return Err(trap("not a variant"));
            };
            let case = int(g, size(&discriminant(cases.len())))? as u32;
            let (_, case_ty) = cases
                .get(case as usize)
                .ok_or_else(|| trap("a variant case out of range"))?;
            let payload = match case_ty {
                Some(t) => Some(Box::new(load(g, t, offset(at, payload_offset(&variant))?)?)),
                None => None,
            };
            from_variant(ty, case, payload)
        }
    })
}

pub(crate) fn load_u32(g: &mut Lift<'_>, at: u32) -> Result<u32, Trap> {
    let bytes: [u8; 4] = g
        .read(at, 4)?
        .try_into()
        .map_err(|_| trap("a read returned the wrong length"))?;
    Ok(u32::from_le_bytes(bytes))
}

pub(crate) fn load_fields<'a>(
    g: &mut Lift<'_>,
    types: impl Iterator<Item = &'a Ty>,
    at: u32,
) -> Result<Vec<Val>, Trap> {
    let mut field = 0;
    let mut out = Vec::new();
    for t in types {
        field = align_to(field, align(t));
        out.push(load(g, t, offset(at, field)?)?);
        field += size(t);
    }
    Ok(out)
}
