//! Control requests, `ctl <verb>`, which only the kernel (caller 0) may send.

use alloc::format;
use alloc::string::{String, ToString};
use kabi::live::valid_label;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    /// The active version, and the candidate if one is on probation.
    Status,
    /// Swap to version `label`; `force` retries one that failed before.
    Swap { label: String, force: bool },
    /// Drop the candidate on probation.
    Rollback,
    /// Start a disabled component afresh, dropping its state.
    Reset,
    /// The active version's source files, or one of them.
    Source { file: Option<String> },
    /// Rehearse a swap to version `label` on copies, changing nothing.
    Try { label: String },
}

pub const USAGE: &str =
    "usage: ctl status | swap <version> [force] | try <version> | rollback | reset | source [file]";

/// Reads what follows `ctl `.
pub fn parse(text: &str) -> Result<Control, String> {
    let words: alloc::vec::Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["status"] => Ok(Control::Status),
        ["swap", label] | ["swap", label, "force"] if valid_label(label) => Ok(Control::Swap {
            label: label.to_string(),
            force: words.len() == 3,
        }),
        ["swap", label, ..] if !valid_label(label) => Err(format!("bad version label {label:?}")),
        ["try", label] if valid_label(label) => Ok(Control::Try {
            label: label.to_string(),
        }),
        ["try", label] => Err(format!("bad version label {label:?}")),
        ["rollback"] => Ok(Control::Rollback),
        ["reset"] => Ok(Control::Reset),
        ["source"] => Ok(Control::Source { file: None }),
        ["source", file] => Ok(Control::Source {
            file: Some(file.to_string()),
        }),
        _ => Err(USAGE.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_parses() {
        assert_eq!(parse("status"), Ok(Control::Status));
        assert_eq!(
            parse("swap 3-trap force"),
            Ok(Control::Swap {
                label: "3-trap".into(),
                force: true
            })
        );
        assert_eq!(
            parse(" swap 2 "),
            Ok(Control::Swap {
                label: "2".into(),
                force: false
            })
        );
        assert_eq!(parse("try e1"), Ok(Control::Try { label: "e1".into() }));
        assert_eq!(parse("rollback"), Ok(Control::Rollback));
        assert_eq!(parse("reset"), Ok(Control::Reset));
        assert_eq!(
            parse("source src/lib.rs"),
            Ok(Control::Source {
                file: Some("src/lib.rs".into())
            })
        );
    }

    #[test]
    fn anything_else_gets_the_usage() {
        assert!(parse("swap ../x").unwrap_err().contains("bad version"));
        assert_eq!(parse("swap 2 now"), Err(USAGE.into()));
        assert!(parse("try ../x").unwrap_err().contains("bad version"));
        assert_eq!(parse("try"), Err(USAGE.into()));
        assert_eq!(parse("try e1 e2"), Err(USAGE.into()));
        assert_eq!(parse("explode"), Err(USAGE.into()));
        assert_eq!(parse(""), Err(USAGE.into()));
    }
}
