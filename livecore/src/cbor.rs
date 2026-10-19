//! Any CBOR item as indented lines of printable ASCII, for `:state`, read without
//! a CBOR library: the kernel has none. Arrays and maps that fit the width stay on
//! one line; the rest put one element per line. A length is checked against the
//! bytes left before anything is allocated, nesting and the number of items are
//! capped, and text is escaped, so the lines are printable ASCII no wider than
//! asked, whatever the bytes. Floats are shown as their bits, which keeps float
//! formatting out of the kernel.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// Deepest nesting read, counting arrays, maps and tags.
pub const MAX_DEPTH: usize = 32;
/// Items read; the rest are left out and shown as `...`.
pub const MAX_ITEMS: usize = 4096;
/// The narrowest width rendered.
const MIN_WIDTH: usize = 8;

const PAST_END: &str = "an item runs past the end";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Unsigned(u64),
    /// The value -1 - n.
    Negative(u64),
    Bytes(Vec<u8>),
    /// Kept as bytes, so text that is not UTF-8 can still be shown.
    Text(Vec<u8>),
    Array(Vec<Item>),
    Map(Vec<(Item, Item)>),
    Tag(u64, Box<Item>),
    /// false (20), true (21), null (22), undefined (23) or another simple value.
    Simple(u8),
    /// A float's bits and its width in bytes: 2, 4 or 8.
    Float(u64, u8),
    /// Where reading stopped at [`MAX_ITEMS`].
    More,
}

struct Reader<'a> {
    rest: &'a [u8],
    items: usize,
    stopped: bool,
}

/// One CBOR item, which must be all of `bytes`.
pub fn parse(bytes: &[u8]) -> Result<Item, &'static str> {
    let mut reader = Reader {
        rest: bytes,
        items: 0,
        stopped: false,
    };
    let item = reader.item(0)?;
    if !reader.stopped && !reader.rest.is_empty() {
        return Err("bytes follow the item");
    }
    Ok(item)
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, &'static str> {
        let (&byte, rest) = self.rest.split_first().ok_or(PAST_END)?;
        self.rest = rest;
        Ok(byte)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], &'static str> {
        if len > self.rest.len() {
            return Err(PAST_END);
        }
        let (head, rest) = self.rest.split_at(len);
        self.rest = rest;
        Ok(head)
    }

    /// The argument of a head: `info` itself up to 23, then the next 1, 2, 4 or 8
    /// bytes. `None` for an indefinite length (31).
    fn argument(&mut self, info: u8) -> Result<Option<u64>, &'static str> {
        let width = match info {
            0..=23 => return Ok(Some(u64::from(info))),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            31 => return Ok(None),
            _ => return Err("a head uses a reserved value (28 to 30)"),
        };
        let bytes = self.take(width)?;
        Ok(Some(bytes.iter().fold(0, |v, &b| (v << 8) | u64::from(b))))
    }

    /// `count` as a length, refused unless `count` elements of at least `size`
    /// bytes each could fit in what is left.
    fn count(&self, count: u64, size: usize) -> Result<usize, &'static str> {
        let count = usize::try_from(count).map_err(|_| PAST_END)?;
        match count.checked_mul(size) {
            Some(need) if need <= self.rest.len() => Ok(count),
            _ => Err(PAST_END),
        }
    }

    fn item(&mut self, depth: usize) -> Result<Item, &'static str> {
        if depth > MAX_DEPTH {
            return Err("items are nested too deeply");
        }
        if self.items == MAX_ITEMS {
            self.stopped = true;
            return Ok(Item::More);
        }
        self.items += 1;
        let head = self.byte()?;
        let (major, info) = (head >> 5, head & 0x1f);
        if major == 7 {
            return self.simple(info);
        }
        match (major, self.argument(info)?) {
            (0, Some(n)) => Ok(Item::Unsigned(n)),
            (1, Some(n)) => Ok(Item::Negative(n)),
            (2 | 3, Some(len)) => {
                let len = self.count(len, 1)?;
                Ok(string(major, self.take(len)?.to_vec()))
            }
            (2 | 3, None) => self.chunks(major),
            (4, count) => self.array(count, depth),
            (5, count) => self.map(count, depth),
            (6, Some(tag)) => Ok(Item::Tag(tag, Box::new(self.item(depth + 1)?))),
            _ => Err("an integer or a tag has no value"),
        }
    }

    fn simple(&mut self, info: u8) -> Result<Item, &'static str> {
        let float = |reader: &mut Self, width: u8| {
            let bytes = reader.take(usize::from(width))?;
            let bits = bytes.iter().fold(0, |v, &b| (v << 8) | u64::from(b));
            Ok(Item::Float(bits, width))
        };
        match info {
            0..=23 => Ok(Item::Simple(info)),
            24 => match self.byte()? {
                value @ 32.. => Ok(Item::Simple(value)),
                _ => Err("a two-byte simple value is below 32"),
            },
            25 => float(self, 2),
            26 => float(self, 4),
            27 => float(self, 8),
            31 => Err("a break stands outside an indefinite item"),
            _ => Err("a head uses a reserved value (28 to 30)"),
        }
    }

    /// Whether the next byte is the break that ends an indefinite item; it is
    /// read if so.
    fn at_break(&mut self) -> Result<bool, &'static str> {
        match self.rest.first() {
            Some(0xff) => {
                self.rest = &self.rest[1..];
                Ok(true)
            }
            Some(_) => Ok(false),
            None => Err(PAST_END),
        }
    }

    /// An indefinite byte or text string: definite chunks of the same type.
    fn chunks(&mut self, major: u8) -> Result<Item, &'static str> {
        let mut bytes = Vec::new();
        while !self.at_break()? {
            let head = self.byte()?;
            if head >> 5 != major {
                return Err("an indefinite string holds another type");
            }
            let len = self
                .argument(head & 0x1f)?
                .ok_or("an indefinite string holds another indefinite one")?;
            let len = self.count(len, 1)?;
            bytes.extend_from_slice(self.take(len)?);
        }
        Ok(string(major, bytes))
    }

    fn array(&mut self, count: Option<u64>, depth: usize) -> Result<Item, &'static str> {
        let mut items = Vec::new();
        match count {
            Some(count) => {
                for _ in 0..self.count(count, 1)? {
                    if self.stopped {
                        break;
                    }
                    items.push(self.item(depth + 1)?);
                }
            }
            None => {
                while !self.stopped && !self.at_break()? {
                    items.push(self.item(depth + 1)?);
                }
            }
        }
        Ok(Item::Array(items))
    }

    fn map(&mut self, count: Option<u64>, depth: usize) -> Result<Item, &'static str> {
        let mut entries = Vec::new();
        let mut entry = |reader: &mut Self| -> Result<(), &'static str> {
            let key = reader.item(depth + 1)?;
            let value = if reader.stopped {
                Item::More
            } else {
                reader.item(depth + 1)?
            };
            entries.push((key, value));
            Ok(())
        };
        match count {
            Some(count) => {
                for _ in 0..self.count(count, 2)? {
                    if self.stopped {
                        break;
                    }
                    entry(self)?;
                }
            }
            None => {
                while !self.stopped && !self.at_break()? {
                    entry(self)?;
                }
            }
        }
        Ok(Item::Map(entries))
    }
}

fn string(major: u8, bytes: Vec<u8>) -> Item {
    match major {
        2 => Item::Bytes(bytes),
        _ => Item::Text(bytes),
    }
}

/// `item` as lines no wider than `width` (at least 8), at most `rows` of them
/// (at least 1). When more would follow, the last line is `...`.
pub fn render(item: &Item, width: usize, rows: usize) -> Vec<String> {
    let mut out = Lines {
        lines: Vec::new(),
        width: width.max(MIN_WIDTH),
        rows: rows.max(1),
        full: false,
    };
    out.block(item, 0, "", "");
    if out.full {
        out.lines.truncate(out.rows - 1);
        out.lines.push(String::from("..."));
    }
    out.lines
}

struct Lines {
    lines: Vec<String>,
    width: usize,
    rows: usize,
    full: bool,
}

impl Lines {
    fn push(&mut self, indent: usize, text: &str) {
        if self.lines.len() == self.rows {
            self.full = true;
            return;
        }
        let mut line = format!("{:indent$}{text}", "");
        if line.len() > self.width {
            line.truncate(self.width - 3);
            line.push_str("...");
        }
        self.lines.push(line);
    }

    /// `item` at `indent`, its first line after `prefix` and its last before
    /// `suffix`: on one line if it fits, else one element per line.
    fn block(&mut self, item: &Item, indent: usize, prefix: &str, suffix: &str) {
        if self.full {
            return;
        }
        let room = self
            .width
            .saturating_sub(indent + prefix.len() + suffix.len());
        if let Some(text) = inline(item, room) {
            self.push(indent, &format!("{prefix}{text}{suffix}"));
            return;
        }
        let last = |n: usize, len: usize| if n + 1 < len { "," } else { "" };
        match item {
            Item::Array(items) => {
                self.push(indent, &format!("{prefix}["));
                for (n, child) in items.iter().enumerate() {
                    self.block(child, indent + 2, "", last(n, items.len()));
                }
                self.push(indent, &format!("]{suffix}"));
            }
            Item::Map(entries) => {
                self.push(indent, &format!("{prefix}{{"));
                for (n, (key, value)) in entries.iter().enumerate() {
                    let key = short(key, self.width / 2);
                    let key = format!("{key}: ");
                    self.block(value, indent + 2, &key, last(n, entries.len()));
                }
                self.push(indent, &format!("}}{suffix}"));
            }
            Item::Tag(tag, inner) => {
                self.block(
                    inner,
                    indent,
                    &format!("{prefix}{tag}("),
                    &format!("){suffix}"),
                );
            }
            leaf => self.push(indent, &format!("{prefix}{}{suffix}", short(leaf, room))),
        }
    }
}

/// `item` on one line, if that is at most `room` long.
fn inline(item: &Item, room: usize) -> Option<String> {
    let mut out = String::new();
    write_inline(item, &mut out, room).then_some(out)
}

/// Appends `item` to `out`; false once `out` is longer than `room`.
fn write_inline(item: &Item, out: &mut String, room: usize) -> bool {
    match item {
        Item::Array(items) => {
            out.push('[');
            for (n, child) in items.iter().enumerate() {
                if n > 0 {
                    out.push_str(", ");
                }
                if !write_inline(child, out, room) {
                    return false;
                }
            }
            out.push(']');
        }
        Item::Map(entries) => {
            out.push('{');
            for (n, (key, value)) in entries.iter().enumerate() {
                if n > 0 {
                    out.push_str(", ");
                }
                if !write_inline(key, out, room) {
                    return false;
                }
                out.push_str(": ");
                if !write_inline(value, out, room) {
                    return false;
                }
            }
            out.push('}');
        }
        Item::Tag(tag, inner) => {
            let _ = write!(out, "{tag}(");
            if !write_inline(inner, out, room) {
                return false;
            }
            out.push(')');
        }
        leaf => leaf_text(leaf, out, room),
    }
    out.len() <= room
}

/// `item` cut to `room`, ending in `...` if it was cut; a container stands as
/// `[...]`, `{...}` or `tag(...)` when it does not fit.
fn short(item: &Item, room: usize) -> String {
    if let Some(text) = inline(item, room) {
        return text;
    }
    let mut text = match item {
        Item::Array(_) => String::from("[...]"),
        Item::Map(_) => String::from("{...}"),
        Item::Tag(tag, _) => format!("{tag}(...)"),
        leaf => {
            let mut text = String::new();
            leaf_text(leaf, &mut text, room);
            text
        }
    };
    if text.len() > room {
        text.truncate(room.saturating_sub(3));
        text.push_str("...");
    }
    text
}

/// Appends a scalar, stopping soon after `out` grows past `limit`.
fn leaf_text(item: &Item, out: &mut String, limit: usize) {
    let _ = match item {
        Item::Unsigned(n) => write!(out, "{n}"),
        Item::Negative(n) => write!(out, "-{}", u128::from(*n) + 1),
        Item::Bytes(bytes) => {
            out.push_str("h'");
            for byte in bytes {
                if out.len() > limit {
                    return;
                }
                let _ = write!(out, "{byte:02x}");
            }
            write!(out, "'")
        }
        Item::Text(bytes) => {
            quoted(bytes, out, limit);
            Ok(())
        }
        Item::Simple(20) => write!(out, "false"),
        Item::Simple(21) => write!(out, "true"),
        Item::Simple(22) => write!(out, "null"),
        Item::Simple(23) => write!(out, "undefined"),
        Item::Simple(value) => write!(out, "simple({value})"),
        Item::Float(bits, 2) => write!(out, "f16({bits:#06x})"),
        Item::Float(bits, 4) => write!(out, "f32({bits:#010x})"),
        Item::Float(bits, _) => write!(out, "f64({bits:#018x})"),
        Item::More | Item::Array(_) | Item::Map(_) | Item::Tag(..) => write!(out, "..."),
    };
}

/// Text in quotes: printable ASCII as it is, `"` and `\` escaped, other
/// characters as `\u{..}`, and bytes that are not UTF-8 as `\x..`.
fn quoted(bytes: &[u8], out: &mut String, limit: usize) {
    out.push('"');
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if out.len() > limit {
                return;
            }
            let _ = match c {
                '"' => write!(out, "\\\""),
                '\\' => write!(out, "\\\\"),
                '\n' => write!(out, "\\n"),
                '\t' => write!(out, "\\t"),
                '\r' => write!(out, "\\r"),
                ' '..='~' => write!(out, "{c}"),
                _ => write!(out, "\\u{{{:x}}}", u32::from(c)),
            };
        }
        for byte in chunk.invalid() {
            if out.len() > limit {
                return;
            }
            let _ = write!(out, "\\x{byte:02x}");
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests;
