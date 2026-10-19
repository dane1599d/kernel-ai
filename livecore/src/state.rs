//! The header of a state, CBOR `[schema, body]`, read without a CBOR library so
//! the kernel and the host can check which schema a state is in; and the whole
//! state as text for `:state`.

use crate::cbor::{self, Item};
use crate::ledger::Ledger;
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
    let (&array, rest) = state.split_first()?;
    if array != 0x82 {
        return None;
    }
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
