//! The OS's actions as `kai` imports (ADR 0012 D6): one import per registry
//! row the one door runs, module `kai`, field = the row's name, and its
//! prototype in kai.h. A golden test (`tests/golden/kai_rows.h`) fixes the
//! lowering:
//!
//! - `Int` is `I64`; an optional one passes `KAI_NONE` when it is absent.
//! - `Enum`, `Text`, `Path` and `Host` are NUL-terminated `const char *`; an
//!   optional one passes `NULL`. The host finds the length, within the
//!   argument's own limit, so D6's (pointer, length) pair is one C pointer.
//! - Every import returns an `I64` status: a negative `KAI_E*` on failure,
//!   otherwise 0. A value comes back through out parameters after the
//!   arguments:
//!   - `I64 *out` for an `Int`;
//!   - `char *out, I64 cap` for text, filled as `snprintf` fills, with the
//!     full length as the status;
//!   - `struct kai_<row> *out` for a record;
//!   - `struct kai_<row>_item *out, I64 cap` for a list, with the full count
//!     as the status.
//! - Text in a record is a `char` array of the field's limit plus its NUL,
//!   cut at a character boundary.
//! - Rows with a shape not lowered yet (a `List` argument, a record inside a
//!   record) are left out, as are never-tier rows.

mod effects;

pub use effects::{Layout, Rows};

use agentabi::{Action, ArgType, RetType, Tier};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;
use kailink::{FuncType, ValType};

/// Refused by consent, or the user said no.
pub const KAI_EREFUSED: i64 = -1;
/// The action ran and failed.
pub const KAI_EFAILED: i64 = -5;
/// Text over its limit, or a call too long to ask about.
pub const KAI_E2BIG: i64 = -7;
/// A pointer out of the session's memory.
pub const KAI_EFAULT: i64 = -14;
/// An argument out of its range or not one of its words.
pub const KAI_EINVAL: i64 = -22;
/// An optional `Int` argument left out.
pub const KAI_NONE: i64 = i64::MIN;

/// Longest path or host, in bytes.
const PATH_MAX: usize = 1024;
const HOST_MAX: usize = 253;

/// One argument or result value.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Scalar {
    Int {
        min: i64,
        max: i64,
    },
    /// Text of at most `bytes` bytes, and `chars` characters if the row says;
    /// `words` for an `Enum`.
    Text {
        bytes: usize,
        chars: Option<usize>,
        words: Option<&'static [&'static str]>,
    },
}

fn scalar(ty: &ArgType) -> Option<Scalar> {
    let text = |bytes, chars, words| Scalar::Text {
        bytes,
        chars,
        words,
    };
    match *ty {
        ArgType::Int { min, max } => Some(Scalar::Int { min, max }),
        ArgType::Enum(words) => {
            let longest = words.iter().map(|w| w.len()).max().unwrap_or(0);
            Some(text(longest, None, Some(words)))
        }
        ArgType::Text { max } => {
            let chars = usize::from(max);
            Some(text(4 * chars, Some(chars), None))
        }
        ArgType::Path => Some(text(PATH_MAX, None, None)),
        ArgType::Host => Some(text(HOST_MAX, None, None)),
        ArgType::List(..) | ArgType::Opt(_) => None,
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Param {
    pub(crate) name: &'static str,
    pub(crate) scalar: Scalar,
    pub(crate) optional: bool,
}

/// A field of a result struct, at its offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Field {
    pub(crate) name: &'static str,
    /// `None` for an `I64`, else the `char` array's size, its NUL included.
    pub(crate) text: Option<u32>,
    pub(crate) offset: u32,
}

/// How a row's result comes back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Out {
    Unit,
    Int,
    Text,
    Record { fields: Vec<Field>, size: u32 },
    List { fields: Vec<Field>, size: u32 },
}

/// One row, lowered.
#[derive(Clone, Debug)]
pub(crate) struct Lowered {
    pub(crate) row: &'static Action,
    pub(crate) params: Vec<Param>,
    pub(crate) out: Out,
}

/// `row` as an import; `None` when it is never-tier or has a shape with no
/// lowering yet.
pub(crate) fn lower(row: &'static Action) -> Option<Lowered> {
    if agentabi::derive(row) == Tier::Never {
        return None;
    }
    let mut params = Vec::with_capacity(row.args.len());
    for arg in row.args {
        let (ty, optional) = match &arg.ty {
            ArgType::Opt(inner) => (*inner, true),
            ty => (ty, false),
        };
        params.push(Param {
            name: arg.name,
            scalar: scalar(ty)?,
            optional,
        });
    }
    let out = match &row.ret {
        RetType::Unit => Out::Unit,
        RetType::Of(ArgType::Int { .. }) => Out::Int,
        RetType::Of(_) | RetType::Data => {
            scalar_out(&row.ret)?;
            Out::Text
        }
        RetType::Record(fields) => {
            let (fields, size) = layout(fields)?;
            Out::Record { fields, size }
        }
        RetType::List(item) => {
            let (fields, size) = match item {
                RetType::Record(fields) => layout(fields)?,
                scalar => layout(&[("value", **scalar)])?,
            };
            Out::List { fields, size }
        }
    };
    Some(Lowered { row, params, out })
}

/// A scalar result's field: `None` for an `I64`, else its array size.
fn scalar_out(ret: &RetType) -> Option<Option<u32>> {
    match ret {
        RetType::Of(ArgType::Int { .. }) => Some(None),
        RetType::Of(ty) => match scalar(ty)? {
            Scalar::Text { bytes, chars, .. } => {
                // A record keeps a limit's worth of bytes, not of characters.
                let bytes = chars.unwrap_or(bytes);
                Some(Some(u32::try_from(bytes).ok()?.checked_add(1)?))
            }
            Scalar::Int { .. } => Some(None),
        },
        RetType::Data => Some(Some(u32::try_from(PATH_MAX).ok()? + 1)),
        RetType::Unit | RetType::Record(_) | RetType::List(_) => None,
    }
}

/// C's layout of a struct of these fields: each at its alignment (8 for an
/// `I64`, 1 for a `char` array), the size rounded up to the largest.
fn layout(fields: &[(&'static str, RetType)]) -> Option<(Vec<Field>, u32)> {
    let mut out = Vec::with_capacity(fields.len());
    let mut at: u32 = 0;
    let mut align = 1;
    for (name, ret) in fields {
        let text = scalar_out(ret)?;
        let (size, field_align) = match text {
            None => (8, 8),
            Some(bytes) => (bytes, 1),
        };
        at = at.checked_next_multiple_of(field_align)?;
        out.push(Field {
            name,
            text,
            offset: at,
        });
        at = at.checked_add(size)?;
        align = align.max(field_align);
    }
    Some((out, at.max(1).checked_next_multiple_of(align)?))
}

impl Lowered {
    /// The import's Wasm type.
    pub(crate) fn ty(&self) -> FuncType {
        let mut params: Vec<ValType> = self
            .params
            .iter()
            .map(|p| match p.scalar {
                Scalar::Int { .. } => ValType::I64,
                Scalar::Text { .. } => ValType::I32,
            })
            .collect();
        params.extend(match self.out {
            Out::Unit => &[][..],
            Out::Int | Out::Record { .. } => &[ValType::I32][..],
            Out::Text | Out::List { .. } => &[ValType::I32, ValType::I64][..],
        });
        FuncType {
            params,
            results: vec![ValType::I64],
        }
    }

    /// Its struct and prototype in kai.h.
    fn declare(&self, out: &mut String) {
        let name = self.row.name;
        let doc = self.row.doc.replace("*/", "* /");
        let _ = writeln!(out, "/* {doc} */");
        let item = match &self.out {
            Out::Record { fields, .. } => Some((format!("kai_{name}"), fields)),
            Out::List { fields, .. } => Some((format!("kai_{name}_item"), fields)),
            _ => None,
        };
        if let Some((tag, fields)) = &item {
            let _ = write!(out, "struct {tag} {{");
            for field in fields.iter() {
                match field.text {
                    None => {
                        let _ = write!(out, " I64 {};", c_name(field.name));
                    }
                    Some(bytes) => {
                        let _ = write!(out, " char {}[{bytes}];", c_name(field.name));
                    }
                }
            }
            let _ = writeln!(out, " }};");
        }
        let mut params: Vec<String> = self
            .params
            .iter()
            .map(|p| match p.scalar {
                Scalar::Int { .. } => format!("I64 {}", c_name(p.name)),
                Scalar::Text { .. } => format!("const char *{}", c_name(p.name)),
            })
            .collect();
        match (&self.out, &item) {
            (Out::Int, _) => params.push("I64 *out".into()),
            (Out::Text, _) => params.push("char *out, I64 cap".into()),
            (Out::Record { .. }, Some((tag, _))) => params.push(format!("struct {tag} *out")),
            (Out::List { .. }, Some((tag, _))) => {
                params.push(format!("struct {tag} *out, I64 cap"));
            }
            _ => {}
        }
        let params = match params.is_empty() {
            true => String::from("void"),
            false => params.join(", "),
        };
        let _ = writeln!(out, "__KAI_IMPORT(\"{name}\") I64 {name}({params});");
    }
}

/// A row's argument or field name as C may write it: a keyword gets a `_`.
fn c_name(name: &str) -> String {
    match promptcore::split::keyword(name) {
        true => format!("{name}_"),
        false => name.into(),
    }
}

/// kai.h's part from the rows: the status codes, then each row's struct and
/// prototype.
pub(crate) fn declarations(rows: &[Lowered]) -> String {
    let mut out = format!(
        "/* The OS's actions (ADR 0012 D6), registry version {}. Each returns a\n   \
         status: a negative KAI_E* on failure. */\n",
        agentabi::REGISTRY_VERSION
    );
    for (name, value) in [
        ("KAI_EREFUSED", KAI_EREFUSED),
        ("KAI_EFAILED", KAI_EFAILED),
        ("KAI_E2BIG", KAI_E2BIG),
        ("KAI_EFAULT", KAI_EFAULT),
        ("KAI_EINVAL", KAI_EINVAL),
    ] {
        let _ = writeln!(out, "#define {name} ({value})");
    }
    out.push_str("#define KAI_NONE (-9223372036854775807LL - 1)\n");
    for row in rows {
        row.declare(&mut out);
    }
    out
}
