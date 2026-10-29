//! What session code may import (ADR 0012 D5): kairt's `fd_write` for
//! output, kai.h's `__show_*` calls, and the effects the service allows, each
//! of which suspends the run with [`Need`]. Nothing else links.

use super::{MAX_OUTPUT, MAX_SHOWN};
use alloc::format;
use alloc::string::{String, ToString};
#[cfg(test)]
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use kailink::{FuncType as LinkType, ValType as LinkVal};
use promptcore::wire::ValueType;
use wasmi::{Caller, Error, FuncType, Linker, Memory, TrapCode, Val, ValType};

/// The error an effect import raises to suspend the run.
#[derive(Debug)]
pub(super) struct Need {
    pub(super) name: String,
    pub(super) args: Vec<u64>,
}

impl fmt::Display for Need {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "need {}", self.name)
    }
}

impl wasmi::errors::HostError for Need {}

/// What session code leaves for the reply.
#[derive(Default)]
pub(super) struct State {
    pub(super) out: Vec<u8>,
    pub(super) cut: bool,
    pub(super) value: Option<(ValueType, Vec<u8>)>,
}

/// The built-in imports and their types: kai.h's `__show_*`.
const SHOWS: [(&str, LinkVal); 5] = [
    ("__show_i64", LinkVal::I64),
    ("__show_u64", LinkVal::I64),
    ("__show_f64", LinkVal::F64),
    ("__show_str", LinkVal::I32),
    ("__show_ptr", LinkVal::I32),
];

/// Whether session code may import `kai.field` with type `ty`.
pub(super) fn allowed(effects: &[(String, LinkType)], field: &str, ty: &LinkType) -> bool {
    let show = SHOWS
        .iter()
        .any(|(name, param)| *name == field && ty.params == [*param] && ty.results.is_empty());
    show || effects.iter().any(|(name, t)| name == field && t == ty)
}

fn val_type(ty: LinkVal) -> ValType {
    match ty {
        LinkVal::I32 => ValType::I32,
        LinkVal::I64 => ValType::I64,
        LinkVal::F32 => ValType::F32,
        LinkVal::F64 => ValType::F64,
    }
}

pub(super) fn define_effect(
    linker: &mut Linker<State>,
    name: &str,
    ty: &LinkType,
) -> Result<(), Error> {
    let wasm_ty = FuncType::new(
        ty.params.iter().copied().map(val_type),
        ty.results.iter().copied().map(val_type),
    );
    let owned = name.to_string();
    linker.func_new(kailink::KAI, name, wasm_ty, move |_caller, args, _out| {
        let args = args.iter().map(bits).collect();
        Err(Error::host(Need {
            name: owned.clone(),
            args,
        }))
    })?;
    Ok(())
}

fn bits(val: &Val) -> u64 {
    match val {
        Val::I32(x) => *x as u32 as u64,
        Val::I64(x) => *x as u64,
        Val::F32(x) => u64::from(x.to_bits()),
        Val::F64(x) => x.to_bits(),
        _ => 0,
    }
}

pub(super) fn define_builtins(linker: &mut Linker<State>, memory: Memory) -> Result<(), Error> {
    let shown = |ty: ValueType, text: String| {
        move |mut caller: Caller<State>| caller.data_mut().value = Some((ty, text.into_bytes()))
    };
    linker.func_wrap(
        kailink::KAI,
        "__show_i64",
        move |c: Caller<State>, v: i64| shown(ValueType::I64, format!("{v}"))(c),
    )?;
    linker.func_wrap(
        kailink::KAI,
        "__show_u64",
        move |c: Caller<State>, v: i64| shown(ValueType::U64, format!("{}", v as u64))(c),
    )?;
    linker.func_wrap(
        kailink::KAI,
        "__show_f64",
        move |c: Caller<State>, v: f64| shown(ValueType::F64, format!("{v}"))(c),
    )?;
    linker.func_wrap(
        kailink::KAI,
        "__show_ptr",
        move |c: Caller<State>, p: i32| shown(ValueType::Ptr, format!("0x{:x}", p as u32))(c),
    )?;
    linker.func_wrap(
        kailink::KAI,
        "__show_str",
        move |mut caller: Caller<State>, p: i32| {
            let (mem, state) = memory.data_and_store_mut(&mut caller);
            let start = (p as u32 as usize).min(mem.len());
            let text: Vec<u8> = mem[start..]
                .iter()
                .take(MAX_SHOWN)
                .take_while(|&&b| b != 0)
                .copied()
                .collect();
            state.value = Some((ValueType::Str, text));
        },
    )?;
    linker.func_wrap(
        kailink::WASI,
        kailink::FD_WRITE,
        move |mut caller: Caller<State>, fd: i32, iovs: i32, count: i32, written: i32| -> i32 {
            let (mem, state) = memory.data_and_store_mut(&mut caller);
            fd_write(mem, state, fd, iovs as u32, count as u32, written as u32)
        },
    )?;
    Ok(())
}

const EBADF: i32 = 8;
const EFAULT: i32 = 21;
const EINVAL: i32 = 28;
/// Most iovecs one `fd_write` may pass, as wasihost allows.
const MAX_IOVS: u32 = 1024;

/// WASI `fd_write` for fds 1 and 2: keeps up to [`MAX_OUTPUT`] bytes, drops
/// the rest (reported as written, so kairt does not retry) and notes the cut.
fn fd_write(
    mem: &mut [u8],
    state: &mut State,
    fd: i32,
    iovs: u32,
    count: u32,
    written: u32,
) -> i32 {
    if fd != 1 && fd != 2 {
        return EBADF;
    }
    if count > MAX_IOVS {
        return EINVAL;
    }
    let read_u32 = |mem: &[u8], at: u64| -> Option<u32> {
        let at = usize::try_from(at).ok()?;
        let bytes = mem.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    };
    let mut total: u32 = 0;
    for i in 0..u64::from(count) {
        let at = u64::from(iovs) + i * 8;
        let (Some(base), Some(len)) = (read_u32(mem, at), read_u32(mem, at + 4)) else {
            return EFAULT;
        };
        let Some(bytes) = (base as usize)
            .checked_add(len as usize)
            .and_then(|end| mem.get(base as usize..end))
        else {
            return EFAULT;
        };
        let room = MAX_OUTPUT - state.out.len();
        state.out.extend_from_slice(&bytes[..bytes.len().min(room)]);
        state.cut |= bytes.len() > room;
        total = total.saturating_add(len);
    }
    let at = written as usize;
    match at.checked_add(4).and_then(|end| mem.get_mut(at..end)) {
        Some(slot) => {
            slot.copy_from_slice(&total.to_le_bytes());
            0
        }
        None => EFAULT,
    }
}

/// A trap's reason word for the `trap` record.
pub(super) fn reason(error: &Error) -> &'static str {
    match error.as_trap_code() {
        Some(TrapCode::UnreachableCodeReached) => "unreachable",
        Some(TrapCode::MemoryOutOfBounds) => "memory",
        Some(TrapCode::TableOutOfBounds) => "table",
        Some(TrapCode::IndirectCallToNull) => "null-call",
        Some(TrapCode::IntegerDivisionByZero) => "div-zero",
        Some(TrapCode::IntegerOverflow) => "overflow",
        Some(TrapCode::BadConversionToInteger) => "conversion",
        Some(TrapCode::StackOverflow) => "stack",
        Some(TrapCode::BadSignature) => "bad-call",
        Some(TrapCode::OutOfFuel) => "fuel",
        _ => "host",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Memory with two iovecs at 0 ("hello" at 64, " world" at 80) and the
    /// written count at 32.
    fn memory() -> Vec<u8> {
        let mut mem = vec![0u8; 128];
        for (at, value) in [(0, 64u32), (4, 5), (8, 80), (12, 6)] {
            mem[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        mem[64..69].copy_from_slice(b"hello");
        mem[80..86].copy_from_slice(b" world");
        mem
    }

    #[test]
    fn fd_write_keeps_output_and_counts_it() {
        let mut mem = memory();
        let mut state = State::default();
        assert_eq!(fd_write(&mut mem, &mut state, 1, 0, 2, 32), 0);
        assert_eq!(state.out, b"hello world");
        assert_eq!(u32::from_le_bytes(mem[32..36].try_into().unwrap()), 11);
        assert!(!state.cut);
    }

    #[test]
    fn fd_write_refuses_bad_fds_and_pointers() {
        let mut mem = memory();
        let mut state = State::default();
        assert_eq!(fd_write(&mut mem, &mut state, 3, 0, 2, 32), EBADF);
        assert_eq!(
            fd_write(&mut mem, &mut state, 1, 0, MAX_IOVS + 1, 32),
            EINVAL
        );
        assert_eq!(fd_write(&mut mem, &mut state, 1, 124, 1, 32), EFAULT);
        assert_eq!(fd_write(&mut mem, &mut state, 1, 0, 2, 126), EFAULT);
        mem[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(fd_write(&mut mem, &mut state, 1, 0, 1, 32), EFAULT);
    }

    #[test]
    fn fd_write_cuts_at_the_output_limit() {
        let mut mem = memory();
        let mut state = State {
            out: vec![b'x'; MAX_OUTPUT - 3],
            ..State::default()
        };
        assert_eq!(fd_write(&mut mem, &mut state, 2, 0, 2, 32), 0);
        assert_eq!(state.out.len(), MAX_OUTPUT);
        assert!(state.out.ends_with(b"xxxhel"));
        assert!(state.cut);
        assert_eq!(u32::from_le_bytes(mem[32..36].try_into().unwrap()), 11);
    }
}
