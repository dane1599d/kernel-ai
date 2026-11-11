//! `kai-offers 1`: what `updated` answers `models offers` with (P5 task 9),
//! the signed role's targets this OS may download and whether each fits this
//! machine ([`crate::fetch::offers`]), largest first. The shell reads it back
//! to say which model to download and why another does not fit. One line a
//! target, every field a number or a word:
//!
//! ```text
//! kai-offers 1
//! <id> <recommended: 1 or 0> fits 0 0
//! <id> 0 stored 0 0
//! <id> 0 memory <need MiB> <have MiB>
//! <id> 0 disk <need bytes> <free bytes>
//! ```
//!
//! An id is the role's, which `kai-models 1` lists too. What is read back is
//! held as hostile: a line that does not parse refuses the whole answer.

use crate::fetch::{Fit, Offer};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const MAGIC: &str = "kai-offers 1";
/// The most lines written and read back: more than a role names.
pub const MAX_LINES: usize = 64;
/// The longest id read back, as a `model_*` row takes one.
const MAX_ID: usize = 64;

/// One target as the shell reads it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    pub id: String,
    pub fit: Fit,
    pub recommended: bool,
}

/// `offers` as `kai-offers 1`: the first [`MAX_LINES`] whose id a line can
/// carry, so the answer always parses and fits one message (64 lines of at
/// most 117 bytes).
pub fn encode(offers: &[Offer<'_>]) -> String {
    let mut out = format!("{MAGIC}\n");
    let named = offers.iter().filter(|o| id_ok(&o.target.id));
    for o in named.take(MAX_LINES) {
        let (word, a, b) = match o.fit {
            Fit::Fits => ("fits", 0, 0),
            Fit::Stored => ("stored", 0, 0),
            Fit::Memory { need_mib, have_mib } => ("memory", need_mib, have_mib),
            Fit::Disk { need, free } => ("disk", need, free),
        };
        let recommended = u8::from(o.recommended);
        out.push_str(&format!("{} {recommended} {word} {a} {b}\n", o.target.id));
    }
    out
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_graphic())
}

fn line(text: &str) -> Result<Offered, String> {
    let words: Vec<&str> = text.split(' ').collect();
    let [id, recommended, word, a, b] = words.as_slice() else {
        return Err(format!("not five fields: {text}"));
    };
    if !id_ok(id) {
        return Err(String::from("an id that is not one"));
    }
    let number = |n: &str| n.parse::<u64>().map_err(|_| format!("not a number: {n}"));
    let (a, b) = (number(a)?, number(b)?);
    let fit = match (*word, a, b) {
        ("fits", 0, 0) => Fit::Fits,
        ("stored", 0, 0) => Fit::Stored,
        ("memory", need_mib, have_mib) => Fit::Memory { need_mib, have_mib },
        ("disk", need, free) => Fit::Disk { need, free },
        _ => return Err(format!("not a fit: {word} {a} {b}")),
    };
    let recommended = match *recommended {
        "1" if fit == Fit::Fits => true,
        "0" => false,
        _ => {
            return Err(String::from(
                "a recommendation that is not 0, or 1 on one that fits",
            ));
        }
    };
    Ok(Offered {
        id: String::from(*id),
        fit,
        recommended,
    })
}

/// Reads `kai-offers 1` back: every line, or why not. At most one target is
/// recommended, as [`crate::fetch::offers`] gives it.
pub fn parse(text: &str) -> Result<Vec<Offered>, String> {
    let mut lines = text.lines();
    if lines.next() != Some(MAGIC) {
        return Err(format!("not {MAGIC}"));
    }
    let mut out = Vec::new();
    for text in lines {
        if out.len() == MAX_LINES {
            return Err(format!("more than {MAX_LINES} targets"));
        }
        out.push(line(text)?);
    }
    if out.iter().filter(|o| o.recommended).count() > 1 {
        return Err(String::from("more than one target recommended"));
    }
    Ok(out)
}
