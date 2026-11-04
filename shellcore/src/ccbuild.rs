//! `cc build <component>` (ADR 0012 D7; P4 task 13), at `kai>` or `c>`:
//!
//! 1. `pkg`'s `local-next` proposes the version.
//! 2. The host reads the `.c` and `.h` files in `/home/user/c/<component>/`,
//!    `cc` builds them, and the host writes the artifact to
//!    `/home/user/c/<component>.kai` ([`crate::Host::cc_build`]).
//! 3. An app goes to `pkg`'s `local` as a read-only File, which publishes it,
//!    signs it with the device key and installs it as a new generation
//!    ([`crate::Host::pkg_local`]). A component is an artifact only.
//!
//! The install is logged with `pkg`'s `switch-back` as its undo, and the
//! entry's script holds D7's provenance line. The component's name is checked
//! here, before it names a folder.

use crate::actlog::{Change, Entry, Form, State};
use crate::undo::Inverse;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};

/// Where components' folders are: `<component>/` holds its sources, and
/// `<component>.kai` the artifact a build writes.
pub const DIR: &str = "/home/user/c";
/// Where Kai's apps' folders are (`app_build`): `<name>/main.c`, kept after the
/// app is removed, and `<name>.kai`.
pub const APPS_DIR: &str = "/home/user/apps";
/// How the host's `pkg_local` begins its error when `pkg` gave no answer at
/// all, so the install may have happened.
pub const NO_ANSWER: &str = "pkg did not answer";
/// Most characters of a text from `cc` or `pkg` that a reply or the log shows.
const MAX_SHOWN: usize = 300;
/// Most characters of `cc`'s diagnostics the editor's pager gets, lines kept.
const MAX_DIAGNOSTICS: usize = 4_000;

/// The artifact a build wrote, as the host holds it: what an install card
/// shows, and the bytes `pkg` must install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    /// The SHA-256 of the artifact's bytes, in hex.
    pub sha256: String,
    /// The interfaces it imports (`kai:ui/view@0.1.0`), each once.
    pub imports: alloc::vec::Vec<String>,
}

/// What `cc` built, from its `build` reply: `<kind> <bytes> <provenance>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CcBuilt {
    /// An app, which `local` installs; else a component.
    pub app: bool,
    pub bytes: usize,
    /// D7's ledger line: `built-by cc <xcc commit> source <sha256>`.
    pub provenance: String,
    /// Where the host wrote the artifact.
    pub path: String,
}

impl CcBuilt {
    /// `text`, cc's reply to `build`, for the artifact written at `path`.
    pub fn from_reply(text: &str, path: String) -> Result<Self, String> {
        let bad = || format!("cc's build reply is not one: {}", shown(text));
        let mut words = text.splitn(3, ' ');
        let app = match words.next() {
            Some("app" | "window") => true,
            Some("component") => false,
            _ => return Err(bad()),
        };
        let bytes = words.next().and_then(|n| n.parse().ok()).ok_or_else(bad)?;
        let provenance = words
            .next()
            .filter(|p| source_of(p).is_some())
            .ok_or_else(bad)?;
        Ok(Self {
            app,
            bytes,
            provenance: provenance.to_string(),
            path,
        })
    }

    /// The SHA-256 of the sources, from the provenance line.
    pub fn source(&self) -> &str {
        source_of(&self.provenance).unwrap_or("")
    }
}

/// `line`'s source hash if it is exactly D7's `built-by cc <40 hex> source
/// <64 hex>`, in lowercase.
fn source_of(line: &str) -> Option<&str> {
    let hex = |text: &str, len: usize| {
        text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    };
    let rest = line.strip_prefix("built-by cc ")?;
    let (commit, source) = rest.split_once(" source ")?;
    (hex(commit, 40) && hex(source, 64)).then_some(source)
}

/// `text`, from `cc` or `pkg`, as a reply or the log shows it: printable
/// ASCII, anything else as `?`, cut at [`MAX_SHOWN`] characters.
pub(crate) fn shown(text: &str) -> String {
    text.chars()
        .take(MAX_SHOWN)
        .map(|c| {
            if c == ' ' || c.is_ascii_graphic() {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// `text` as [`shown`] has it, but with its lines kept and more of it: `cc`'s
/// diagnostics, one to a line, for the editor's pager.
fn shown_lines(text: &str) -> String {
    text.chars()
        .take(MAX_DIAGNOSTICS)
        .map(|c| {
            if c == '\n' || c == ' ' || c.is_ascii_graphic() {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// Whether `version`, `local-next`'s answer, is `x.y.z` in digits, as cc
/// takes it.
pub(crate) fn valid_version(version: &str) -> bool {
    let parts: alloc::vec::Vec<&str> = version.split('.').collect();
    version.len() <= 32
        && parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether `name` may be built: a package's name, 2-12 of a-z, 0-9 and `-`,
/// starting with a letter. `cc` and `pkg` check it again.
pub fn valid_name(name: &str) -> bool {
    (2..=12).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// `pkg`'s `local` answer, `<name> <version> generation <n> from <parent>`,
/// for the build of `name` at `version`: the generation it made and its parent.
pub(crate) fn installed(text: &str, name: &str, version: &str) -> Option<(u64, u64)> {
    match text.split(' ').collect::<alloc::vec::Vec<_>>()[..] {
        [got, v, "generation", n, "from", parent] if got == name && v == version => {
            Some((n.parse().ok()?, parent.parse().ok()?))
        }
        _ => None,
    }
}

/// Who a build is for, as its log entry records it.
pub(crate) struct Asker<'a> {
    /// The entry's request: the line typed, or the app's account of it.
    pub request: &'a str,
    /// The entry's consent: `typed`, or `app:<name>` for an app's door.
    pub consent: &'a str,
    /// Whether `cc`'s diagnostics keep their lines, for the editor's pager.
    pub lines: bool,
}

impl Shell {
    /// `cc build <name>` as the person typed `line`.
    pub(crate) fn cc_build(&mut self, line: &str, name: &str) -> Reply {
        let asker = Asker {
            request: line,
            consent: "typed",
            lines: false,
        };
        self.cc_build_as(&asker, name)
    }

    /// `cc trial <name>`: the last build, which must be `name`'s, tried once on
    /// stub imports as `app_build` tries each ([`crate::Host::cc_trial`]). It
    /// runs nothing for real and changes nothing, so nothing is logged.
    pub(crate) fn cc_trial_line(&mut self, name: &str) -> Reply {
        if !valid_name(name) {
            return Reply::err(format!(
                "cc trial takes a component's name: 2-12 of a-z, 0-9 and -, starting with a letter, not {name:?}"
            ));
        }
        match self.host.cc_trial(name) {
            Ok(report) => Reply::ok(format!("{name}: {}", shown(&report))),
            Err(why) => Reply::err(format!("cc trial {name}: {}", shown(&why))),
        }
    }

    pub(crate) fn cc_build_as(&mut self, asker: &Asker<'_>, name: &str) -> Reply {
        if !valid_name(name) {
            return Reply::err(format!(
                "cc build takes a component's name: 2-12 of a-z, 0-9 and -, starting with a letter, not {name:?}"
            ));
        }
        let version = match self.host.pkg(&format!("local-next {name}")) {
            Ok(version) if valid_version(&version) => version,
            Ok(other) => {
                return Reply::err(format!(
                    "cc build {name}: pkg's next version is not one: {}",
                    shown(&other)
                ));
            }
            Err(why) => return Reply::err(format!("cc build {name}: pkg: {}", shown(&why))),
        };
        let built = match self.host.cc_build(DIR, name, &version) {
            Ok(built) => built,
            Err(why) => {
                let why = if asker.lines {
                    shown_lines(&why)
                } else {
                    shown(&why)
                };
                return Reply::err(format!("cc build {name}: {why}"));
            }
        };
        let script = format!("cc build {name} {version}\n{}", built.provenance);
        self.host
            .audit(&format!("cc build {name} {version}: {}", built.provenance));
        if !built.app {
            return Reply::ok(format!(
                "built {name} {version}, a component of {} bytes: {}. Only an app installs from local.",
                built.bytes, built.path
            ));
        }
        let answer = match self.host.pkg_local(name, &built.path, None) {
            Ok(answer) => answer,
            // No answer at all: pkg may have installed it, so the attempt is
            // logged, with nothing `undo` could switch back.
            Err(why) if why.starts_with(NO_ANSWER) => {
                let why = shown(&why);
                let outcome = format!("local: {why}");
                self.log_build(asker, script, &outcome, State::Nothing, None);
                return Reply::err(format!(
                    "built {name} {version} at {}, but {why}: it may have installed it, and undo cannot switch that back",
                    built.path
                ));
            }
            Err(why) => {
                return Reply::err(format!(
                    "built {name} {version} at {}, but local did not install it: {}",
                    built.path,
                    shown(&why)
                ));
            }
        };
        let made = installed(&answer, name, &version);
        let change = Change {
            action: String::from("cc_build"),
            owner: String::from("pkg"),
            res: String::from("pkg-generation"),
            before: made.map_or_else(String::new, |(_, parent)| format!("{parent}")),
            after: made.map_or_else(String::new, |(n, _)| format!("{n}")),
            inverse: made.map(|(n, parent)| Inverse::Local {
                from: n,
                to: parent,
            }),
        };
        self.log_build(asker, script, "ran", State::Done, Some(change));
        match made {
            Some((n, _)) => Reply::ok(format!(
                "built {name} {version} ({} bytes) and installed it as generation {n}. undo",
                built.bytes
            )),
            None => Reply::ok(format!(
                "built {name} {version} and installed it, but pkg's answer gives no undo: {}",
                shown(&answer)
            )),
        }
    }

    /// Logs a build that reached `local`, as `asker` asked it.
    fn log_build(
        &mut self,
        asker: &Asker<'_>,
        script: String,
        outcome: &str,
        state: State,
        change: Option<Change>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: None,
            pack: None,
            consent: String::from(asker.consent),
            state,
            request: String::from(asker.request),
            script,
            outcome: String::from(outcome),
            changes: change.into_iter().collect(),
        });
    }
}
