//! `kai:app/storage` and `kai:app/effects` (ADR 0003 section 4) for `lifecycle 2`.
//! Between `begin` and `finish` a call reads the storage as it was when the call
//! began, plus its own writes; its writes and effects are kept for the host to
//! commit or drop. Outside a call they trap: `init`, `migrate` and the rest of
//! the lifecycle have no storage or effects.

use crate::instance::HostState;
use alloc::string::String;
use alloc::vec::Vec;
use livecore::txn::{self, Effect, Storage, Txn};
use wasmi::{Caller, Error, Extern, Memory};

/// The call in progress.
pub(crate) struct OpenCall {
    pub storage: Storage,
    pub txn: Txn,
    pub first_ticket: u32,
}

fn memory(caller: &Caller<'_, HostState>) -> Result<Memory, Error> {
    caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| Error::new("no memory"))
}

fn read(caller: &Caller<'_, HostState>, ptr: i32, len: i32, max: usize) -> Result<Vec<u8>, Error> {
    let (start, len) = (ptr as u32 as usize, len as u32 as usize);
    if len > max {
        return Err(Error::new(alloc::format!("{len} bytes; at most {max}")));
    }
    memory(caller)?
        .data(caller)
        .get(start..start.saturating_add(len))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| Error::new("bytes outside memory"))
}

fn text(caller: &Caller<'_, HostState>, ptr: i32, len: i32, max: usize) -> Result<String, Error> {
    String::from_utf8(read(caller, ptr, len, max)?).map_err(|_| Error::new("text is not UTF-8"))
}

fn key(caller: &Caller<'_, HostState>, ptr: i32, len: i32) -> Result<String, Error> {
    let key = text(caller, ptr, len, txn::MAX_KEY)?;
    txn::check_key(&key).map_err(Error::new)?;
    Ok(key)
}

fn open<'a>(caller: &'a mut Caller<'_, HostState>) -> Result<&'a mut OpenCall, Error> {
    caller
        .data_mut()
        .call
        .as_mut()
        .ok_or_else(|| Error::new("storage and effects are only for action calls"))
}

/// `storage.get(key) -> option<list<u8>>`, written at `ret`.
pub(crate) fn get(
    mut caller: Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
    ret: i32,
) -> Result<(), Error> {
    let key = key(&caller, key_ptr, key_len)?;
    // Outside an action call, a lifecycle call reads the host's view.
    let value = match caller.data_mut().call.as_mut() {
        Some(call) => match call.txn.writes.get(&key) {
            Some(written) => written.clone(),
            None => call.storage.get(&key).cloned(),
        },
        None => match caller.data().view.as_ref() {
            Some(view) => view.get(&key).cloned(),
            None => return Err(Error::new("storage is only for action calls")),
        },
    };
    let mut area = [0u8; 12];
    if let Some(value) = value {
        let at = if value.is_empty() {
            1
        } else {
            let realloc = caller
                .get_export("cabi_realloc")
                .and_then(Extern::into_func)
                .ok_or_else(|| Error::new("no cabi_realloc export"))?
                .typed::<(i32, i32, i32, i32), i32>(&caller)?;
            let at = realloc.call(&mut caller, (0, 0, 1, value.len() as i32))?;
            memory(&caller)?
                .write(&mut caller, at as u32 as usize, &value)
                .map_err(|_| Error::new("the value does not fit in memory"))?;
            at
        };
        area[0] = 1;
        area[4..8].copy_from_slice(&at.to_le_bytes());
        area[8..12].copy_from_slice(&(value.len() as u32).to_le_bytes());
    }
    memory(&caller)?
        .write(&mut caller, ret as u32 as usize, &area)
        .map_err(|_| Error::new("the result area is outside memory"))
}

/// `storage.put(key, value)`.
pub(crate) fn put(
    mut caller: Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
    value_ptr: i32,
    value_len: i32,
) -> Result<(), Error> {
    let key = key(&caller, key_ptr, key_len)?;
    let value = read(&caller, value_ptr, value_len, txn::MAX_VALUE)?;
    open(&mut caller)?.txn.writes.insert(key, Some(value));
    Ok(())
}

/// `storage.delete(key)`.
pub(crate) fn delete(
    mut caller: Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
) -> Result<(), Error> {
    let key = key(&caller, key_ptr, key_len)?;
    open(&mut caller)?.txn.writes.insert(key, None);
    Ok(())
}

/// `effects.request(e: effect) -> u32`: the effect's ticket.
pub(crate) fn request(
    mut caller: Caller<'_, HostState>,
    case: i32,
    a: i32,
    b: i32,
    c: i32,
    d: i32,
) -> Result<i32, Error> {
    let effect = match case {
        0 => Effect::Call {
            name: text(&caller, a, b, txn::MAX_EFFECT_NAME)?,
            args: text(&caller, c, d, txn::MAX_EFFECT_ARGS)?,
        },
        1 => Effect::Notify(text(&caller, a, b, txn::MAX_NOTIFY)?),
        _ => return Err(Error::new("an effect of no known case")),
    };
    effect.check().map_err(Error::new)?;
    let call = open(&mut caller)?;
    if call.txn.effects.len() == txn::MAX_EFFECTS {
        return Err(Error::new("too many effects in one call"));
    }
    let ticket = call
        .first_ticket
        .wrapping_add(call.txn.effects.len() as u32);
    call.txn.effects.push(effect);
    Ok(ticket as i32)
}
