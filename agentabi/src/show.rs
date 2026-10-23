//! Rows as the model reads them: `memory() -> #{free_mib: int, total_mib: int}`.

use crate::{Action, ArgType, RetType};
use core::fmt;

/// A row's signature: `name(arg: type, ...)`, then ` -> type` unless it returns
/// nothing.
pub struct Signature<'a>(pub &'a Action);

impl fmt::Display for Signature<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let row = self.0;
        write!(f, "{}(", row.name)?;
        for (i, arg) in row.args.iter().enumerate() {
            let comma = if i == 0 { "" } else { ", " };
            write!(f, "{comma}{}: {}", arg.name, Type::Arg(&arg.ty))?;
        }
        f.write_str(")")?;
        match row.ret {
            RetType::Unit => Ok(()),
            ret => write!(f, " -> {}", Type::Ret(&ret)),
        }
    }
}

/// A type as Rhai scripts see it: `int`, `string`, `[...]`, `#{...}`.
pub enum Type<'a> {
    Arg(&'a ArgType),
    Ret(&'a RetType),
}

impl fmt::Display for Type<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Arg(ArgType::Int { .. }) => f.write_str("int"),
            Self::Arg(ArgType::Enum(_) | ArgType::Text { .. } | ArgType::Path | ArgType::Host) => {
                f.write_str("string")
            }
            Self::Arg(ArgType::List(inner, _)) => write!(f, "[{}]", Type::Arg(inner)),
            Self::Arg(ArgType::Opt(inner)) => write!(f, "{}?", Type::Arg(inner)),
            Self::Ret(RetType::Unit) => f.write_str("()"),
            Self::Ret(RetType::Of(ty)) => Type::Arg(ty).fmt(f),
            Self::Ret(RetType::Data) => f.write_str("data"),
            Self::Ret(RetType::List(inner)) => write!(f, "[{}]", Type::Ret(inner)),
            Self::Ret(RetType::Record(fields)) => {
                f.write_str("#{")?;
                for (i, (name, ty)) in fields.iter().enumerate() {
                    let comma = if i == 0 { "" } else { ", " };
                    write!(f, "{comma}{name}: {}", Type::Ret(ty))?;
                }
                f.write_str("}")
            }
        }
    }
}
