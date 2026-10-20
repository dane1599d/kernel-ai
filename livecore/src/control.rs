//! Control requests, `ctl <verb>`, which only the kernel (caller 0) may send: how
//! they parse, and how the host answers them.

use crate::artifact;
use crate::host::{EXIT_NO_VERSION, Env, Exit, Host, Mode, Report, Version, exit, text};
use crate::settings::Settings;
use crate::{Fault, Reason};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::live::{kind, valid_label};

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

impl<E: Env> Host<E> {
    pub(crate) fn control(
        &mut self,
        mode: Mode<E::Instance>,
        control: Control,
    ) -> Result<(Mode<E::Instance>, Vec<u8>), Exit> {
        let name = self.core.name.clone();
        Ok(match (control, mode) {
            (Control::Status, mode) => {
                let status = status(&name, &mode, &self.core.settings);
                (mode, text(status))
            }
            (Control::Source { file }, mode) => {
                let reply = match active(&mode) {
                    Some(version) => source(version, file.as_deref()),
                    None => String::from("err no active version"),
                };
                (mode, text(reply))
            }
            (Control::Swap { label, force }, Mode::Serving(version)) => {
                crate::swap::swap(&mut self.core, version, &label, force)?
            }
            (Control::Swap { .. } | Control::Try { .. }, mode @ Mode::Probation(_)) => {
                (mode, text("err busy: a version is on probation"))
            }
            (Control::Try { label }, Mode::Serving(version)) => {
                let reply = crate::try_run::rehearse(&mut self.core, &version, &label);
                (Mode::Serving(version), text(reply))
            }
            (Control::Rollback, Mode::Probation(probation)) => crate::probation::roll_back(
                &mut self.core,
                *probation,
                &Fault::new(Reason::Manual, ""),
                0,
            )
            .map(|(mode, label)| (mode, text(format!("ok rolled back {label}"))))?,
            (Control::Rollback, mode) => (mode, text("err nothing is on probation")),
            (Control::Reset, Mode::Disabled { version, .. }) => self.reset(version)?,
            (Control::Reset, mode) => (mode, text(format!("err {name} is not disabled"))),
            (_, mode @ Mode::Disabled { .. }) => (
                mode,
                text(format!("err {name} is disabled; ctl reset starts fresh")),
            ),
            (_, Mode::Moving) => unreachable!("a request found the host between modes"),
        })
    }

    /// `ctl reset`: forget the state and start the active version empty.
    fn reset(
        &mut self,
        version: Version<E::Instance>,
    ) -> Result<(Mode<E::Instance>, Vec<u8>), Exit> {
        self.core.report(&Report::new(kind::RESET))?;
        let label = version.label.clone();
        let fresh = self
            .core
            .stage(&label, version.artifact, None, false)
            .map_err(|fault| exit(EXIT_NO_VERSION, fault.detail))?;
        let mode = Self::fresh(&mut self.core, fresh)?;
        Ok((
            mode,
            text(format!(
                "ok {} started fresh on version {label}",
                self.core.name
            )),
        ))
    }
}

fn active<I>(mode: &Mode<I>) -> Option<&Version<I>> {
    match mode {
        Mode::Serving(version) | Mode::Disabled { version, .. } => Some(version),
        Mode::Probation(probation) => Some(&probation.standby),
        Mode::Moving => None,
    }
}

fn status<I>(name: &str, mode: &Mode<I>, settings: &Settings) -> String {
    match mode {
        Mode::Serving(version) => format!("ok {name} {} schema {}", version.label, version.schema),
        Mode::Probation(p) => format!(
            "ok {name} {} schema {}; probation {} schema {}: {} of {} calls",
            p.standby.label,
            p.standby.schema,
            p.candidate.label,
            p.candidate.schema,
            p.served,
            settings.probation_calls
        ),
        Mode::Disabled { version, why } => format!("ok {name} {} disabled ({why})", version.label),
        Mode::Moving => String::from("err busy"),
    }
}

/// `ctl source`: the files and sizes, or one file cut to fit a reply.
fn source<I>(version: &Version<I>, file: Option<&str>) -> String {
    let Ok(parsed) = artifact::parse(&version.artifact) else {
        return String::from("err the artifact has no readable source");
    };
    let Ok(files) = artifact::source_files(parsed.source) else {
        return String::from("err the source bundle is damaged");
    };
    let limit = kabi::MAX_MESSAGE - 64;
    match file {
        None => {
            let mut list = format!(
                "ok {} {} ({}):",
                version.manifest.component,
                version.label,
                version.manifest.build.get(..12).unwrap_or("")
            );
            for (path, bytes) in files {
                list.push_str(&format!("\n{path} {}", bytes.len()));
            }
            list
        }
        Some(wanted) => match files.iter().find(|(path, _)| *path == wanted) {
            Some((_, bytes)) => {
                let text = String::from_utf8_lossy(bytes);
                let cut = text.floor_char_boundary(limit);
                format!("ok {}", &text[..cut])
            }
            None => format!("err no file {wanted} in version {}", version.label),
        },
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
