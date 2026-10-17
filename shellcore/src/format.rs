//! Showing values without recursing without bound: Rhai's own `Display` follows
//! nesting as deep as it goes, which a line can make deeper than the stack.

use alloc::string::String;
use core::fmt::Write;
use rhai::{Array, Dynamic, Map};

const MAX_DEPTH: usize = 4;
const MAX_CHARS: usize = 2048;
const MAX_ITEMS: usize = 32;

/// The value as the shell echoes it: strings bare, everything else as Rhai would
/// write it, cut off past a few levels or a couple of thousand characters.
pub fn value(value: &Dynamic) -> String {
    if value.is_string() {
        return truncate(&value.clone().into_string().unwrap_or_default(), MAX_CHARS);
    }
    let mut out = String::new();
    write_value(&mut out, value, 0);
    truncate(&out, MAX_CHARS)
}

fn write_value(out: &mut String, value: &Dynamic, depth: usize) {
    if out.len() > MAX_CHARS {
        return;
    }
    if let Some(array) = value.read_lock::<Array>() {
        if depth >= MAX_DEPTH {
            out.push_str("[...]");
            return;
        }
        out.push('[');
        for (index, item) in array.iter().enumerate() {
            if index == MAX_ITEMS {
                let _ = write!(out, ", ... {} more", array.len() - MAX_ITEMS);
                break;
            }
            if index > 0 {
                out.push_str(", ");
            }
            write_value(out, item, depth + 1);
        }
        out.push(']');
    } else if let Some(map) = value.read_lock::<Map>() {
        if depth >= MAX_DEPTH {
            out.push_str("#{...}");
            return;
        }
        out.push_str("#{");
        for (index, (key, item)) in map.iter().enumerate() {
            if index == MAX_ITEMS {
                let _ = write!(out, ", ... {} more", map.len() - MAX_ITEMS);
                break;
            }
            if index > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "{key}: ");
            write_value(out, item, depth + 1);
        }
        out.push('}');
    } else if value.is_string() {
        let _ = write!(out, "{:?}", value.clone().into_string().unwrap_or_default());
    } else {
        let _ = write!(out, "{value}");
    }
}

/// `text` cut to at most `max` bytes at a character boundary, marked if cut.
pub fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return String::from(text);
    }
    let cut = text.floor_char_boundary(max);
    let mut out = String::from(&text[..cut]);
    out.push_str(" ...");
    out
}
