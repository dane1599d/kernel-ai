//! The header of a state, CBOR `[schema, body]`, read without a CBOR library so
//! the kernel and the host can check which schema a state is in; and the whole
//! state as text for `:state`.
//!
//! What the kernel saves for an app with storage is `[schema, body, storage]`:
//! the version's state with the app's own scope, a map of text keys to byte
//! values, after it ([`join`]). An app with nothing stored saves its state as it
//! is, so a component without storage saves what it always has.

use crate::cbor::{self, Item};
use crate::ledger::Ledger;
use crate::txn::Storage;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A state's schema and its body as lines no wider than `width`, at most `rows`
/// of them (see [`cbor::render`]).
pub fn view(state: &[u8], width: usize, rows: usize) -> Result<(u64, Vec<String>), &'static str> {
    match cbor::parse(state)? {
        Item::Array(parts) => match parts.as_slice() {
            [Item::Unsigned(schema), body] => Ok((*schema, cbor::render(body, width, rows))),
            [Item::Unsigned(schema), body, storage] => {
                let mut lines = cbor::render(body, width, rows.saturating_sub(1));
                if let Item::Map(entries) = storage {
                    lines.push(format!("and {} stored keys", entries.len()));
                }
                Ok((*schema, lines))
            }
            _ => Err("it is not a schema and a body"),
        },
        _ => Err("it is not a schema and a body"),
    }
}

/// What `:state <name>` prints, at most `rows` lines: a header, then the state
/// the ledger last saved, its lines at most `width` long after the shell's `= `
/// lead, since the component wrote them.
pub fn lines(name: &str, ledger: &Ledger, width: usize, rows: usize) -> Vec<String> {
    if ledger.checkpoint.is_empty() {
        return vec![format!("{name} has no saved state")];
    }
    let saved_by = ledger.active().unwrap_or("none");
    match view(&ledger.checkpoint, width, rows.saturating_sub(1)) {
        Ok((schema, body)) => {
            let mut lines = vec![format!(
                "{name} state, saved by {saved_by}: schema {schema}, {} bytes",
                ledger.checkpoint.len()
            )];
            lines.extend(body.iter().map(|line| format!("= {line}")));
            lines
        }
        Err(why) => vec![format!("{name} state does not read as CBOR: {why}")],
    }
}

/// Whether `lines` from [`lines`] show a state in `schema` with a body, for the
/// checks after the live scenario.
pub fn shows_schema(lines: &[String], schema: u64) -> bool {
    let header = format!(": schema {schema}, ");
    lines.first().is_some_and(|first| first.contains(&header))
        && lines.len() > 1
        && lines[1..].iter().all(|line| line.starts_with("= "))
}

/// The schema in a state's header: a two-element array (`0x82`), then the schema
/// as an unsigned integer. `None` for anything else.
pub fn schema_of(state: &[u8]) -> Option<u64> {
    match state.first() {
        Some(0x82) => schema_after_header(state),
        _ => None,
    }
}

/// The schema of what the kernel saves: a state, or a state joined with storage
/// (`0x83`).
pub fn saved_schema_of(saved: &[u8]) -> Option<u64> {
    match saved.first() {
        Some(0x82 | 0x83) => schema_after_header(saved),
        _ => None,
    }
}

/// `state` with `storage` joined after it, as the kernel saves it; the state as it
/// is when nothing is stored.
pub fn join(state: &[u8], storage: &Storage) -> Result<Vec<u8>, &'static str> {
    if schema_of(state).is_none() {
        return Err("the state is not a schema and a body");
    }
    if storage.is_empty() {
        return Ok(state.to_vec());
    }
    let mut saved = Vec::with_capacity(state.len() + 64);
    saved.push(0x83);
    saved.extend_from_slice(&state[1..]);
    head(&mut saved, 5, storage.len());
    for (key, value) in storage {
        head(&mut saved, 3, key.len());
        saved.extend_from_slice(key.as_bytes());
        head(&mut saved, 2, value.len());
        saved.extend_from_slice(value);
    }
    Ok(saved)
}

/// What [`join`] made: the state, and the storage.
pub fn split(saved: &[u8]) -> Result<(Vec<u8>, Storage), &'static str> {
    match saved.first() {
        Some(0x82) => return Ok((saved.to_vec(), Storage::new())),
        Some(0x83) => {}
        _ => return Err("it is not a state"),
    }
    let schema = cbor::item_len(&saved[1..])?;
    let body = cbor::item_len(&saved[1 + schema..])?;
    let end = 1 + schema + body;
    let mut state = Vec::with_capacity(end);
    state.push(0x82);
    state.extend_from_slice(&saved[1..end]);
    let Item::Map(entries) = cbor::parse(&saved[end..])? else {
        return Err("the storage is not a map");
    };
    let mut storage = Storage::new();
    for (key, value) in entries {
        let (Item::Text(key), Item::Bytes(value)) = (key, value) else {
            return Err("a stored entry is not text to bytes");
        };
        let key = String::from_utf8(key).map_err(|_| "a stored key is not UTF-8")?;
        if storage.insert(key, value).is_some() {
            return Err("a key is stored twice");
        }
    }
    Ok((state, storage))
}

/// A CBOR head: major type and length.
fn head(out: &mut Vec<u8>, major: u8, len: usize) {
    let major = major << 5;
    let len = len as u64;
    match len {
        0..=23 => out.push(major | len as u8),
        24..=0xff => out.extend_from_slice(&[major | 24, len as u8]),
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ => {
            out.push(major | 26);
            out.extend_from_slice(&(len as u32).to_be_bytes());
        }
    }
}

fn schema_after_header(state: &[u8]) -> Option<u64> {
    let (_, rest) = state.split_first()?;
    let (&head, rest) = rest.split_first()?;
    let (major, info) = (head >> 5, head & 0x1f);
    if major != 0 {
        return None;
    }
    let width = match info {
        0..=23 => return Some(u64::from(info)),
        24 => 1,
        25 => 2,
        26 => 4,
        27 => 8,
        _ => return None,
    };
    let bytes = rest.get(..width)?;
    Some(
        bytes
            .iter()
            .fold(0, |value, &b| (value << 8) | u64::from(b)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unsigned_form_is_read() {
        assert_eq!(schema_of(&[0x82, 0x02, 0x80]), Some(2));
        assert_eq!(schema_of(&[0x82, 0x17]), Some(23));
        assert_eq!(schema_of(&[0x82, 0x18, 24]), Some(24));
        assert_eq!(schema_of(&[0x82, 0x19, 0x01, 0x2c]), Some(300));
        assert_eq!(schema_of(&[0x82, 0x1a, 0, 1, 0, 0]), Some(65536));
        assert_eq!(
            schema_of(&[0x82, 0x1b, 0, 0, 0, 1, 0, 0, 0, 0]),
            Some(1 << 32)
        );
    }

    #[test]
    fn storage_joins_after_the_state_and_splits_back() {
        let state = [0x82, 0x02, 0x83, 0x01, 0x02, 0x03];
        let mut storage = Storage::new();
        assert_eq!(join(&state, &storage).unwrap(), state);
        storage.insert("k".into(), vec![9; 30]);
        storage.insert("notes/1".into(), b"buy milk".to_vec());
        let saved = join(&state, &storage).unwrap();
        assert_eq!(saved[0], 0x83);
        assert_eq!(saved_schema_of(&saved), Some(2));
        assert_eq!(schema_of(&saved), None);
        assert!(matches!(cbor::parse(&saved), Ok(Item::Array(parts)) if parts.len() == 3));
        assert_eq!(split(&saved).unwrap(), (state.to_vec(), storage));
        assert_eq!(split(&state).unwrap(), (state.to_vec(), Storage::new()));
        assert!(join(b"junk", &Storage::new()).is_err());
        assert!(split(&saved[..saved.len() - 1]).is_err());
        assert!(split(&[0x83, 0x02]).is_err());
    }

    #[test]
    fn anything_else_is_refused() {
        for bad in [
            &[][..],
            &[0x82],
            &[0x81, 0x02],
            &[0x83, 0x02],
            &[0x82, 0x22],
            &[0x82, 0x19, 0x01],
            &[0x82, 0x1c],
            b"hello",
        ] {
            assert_eq!(schema_of(bad), None, "{bad:?}");
        }
    }
}
