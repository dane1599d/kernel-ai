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
/// How the host's `pkg_local` begins its error when `pkg` gave no answer at
/// all, so the install may have happened.
pub const NO_ANSWER: &str = "pkg did not answer";

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
        let bad = || format!("cc's build reply is not one: {text}");
        let mut words = text.splitn(3, ' ');
        let app = match words.next() {
            Some("app") => true,
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
fn installed(text: &str, name: &str, version: &str) -> Option<(u64, u64)> {
    match text.split(' ').collect::<alloc::vec::Vec<_>>()[..] {
        [got, v, "generation", n, "from", parent] if got == name && v == version => {
            Some((n.parse().ok()?, parent.parse().ok()?))
        }
        _ => None,
    }
}

impl Shell {
    pub(crate) fn cc_build(&mut self, line: &str, name: &str) -> Reply {
        if !valid_name(name) {
            return Reply::err(format!(
                "cc build takes a component's name: 2-12 of a-z, 0-9 and -, starting with a letter, not {name:?}"
            ));
        }
        let version = match self.host.pkg(&format!("local-next {name}")) {
            Ok(version) => version,
            Err(why) => return Reply::err(format!("cc build {name}: pkg: {why}")),
        };
        let built = match self.host.cc_build(name, &version) {
            Ok(built) => built,
            Err(why) => return Reply::err(format!("cc build {name}: {why}")),
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
        let answer = match self.host.pkg_local(name, &built.path) {
            Ok(answer) => answer,
            // No answer at all: pkg may have installed it, so the attempt is
            // logged, with nothing `undo` could switch back.
            Err(why) if why.starts_with(NO_ANSWER) => {
                self.log_build(line, script, &format!("local: {why}"), State::Nothing, None);
                return Reply::err(format!(
                    "built {name} {version} at {}, but {why}: it may have installed it, and undo cannot switch that back",
                    built.path
                ));
            }
            Err(why) => {
                return Reply::err(format!(
                    "built {name} {version} at {}, but local did not install it: {why}",
                    built.path
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
        self.log_build(line, script, "ran", State::Done, Some(change));
        match made {
            Some((n, _)) => Reply::ok(format!(
                "built {name} {version} ({} bytes) and installed it as generation {n}. undo",
                built.bytes
            )),
            None => Reply::ok(format!(
                "built {name} {version} and installed it, but pkg's answer gives no undo: {answer}"
            )),
        }
    }

    /// Logs a build that reached `local`, as one the user typed.
    fn log_build(
        &mut self,
        line: &str,
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
            consent: String::from("typed"),
            state,
            request: String::from(line),
            script,
            outcome: String::from(outcome),
            changes: change.into_iter().collect(),
        });
    }
}
