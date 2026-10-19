//! Building: the edited source goes to the host's build service, and what comes
//! back is checked and uploaded, or the compiler's first errors are shown with the
//! cursor on the first.

use super::{BUILD_TIMEOUT_MS, Env, Service, err, ok};
use crate::wire::{self, BuildRequest, BuildResult, Origin};
use crate::{editable, rules};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use livecore::artifact;

/// Diagnostic lines a failed build shows.
const DIAGNOSTICS: usize = 8;
/// How a failed build that moved the cursor begins, after `err `.
pub(crate) const MOVED: &str = "build failed; the cursor is on the first error";

impl Service {
    pub(super) fn build(&mut self, env: &mut dyn Env) -> String {
        let Some(session) = self.session.as_ref() else {
            return err("no file is open");
        };
        let component = session.component.clone();
        let base = session.label.clone();
        if let Some(built) = &session.built {
            return err(format!(
                "{built} was built from this text already; :apply runs it"
            ));
        }
        if !session.changed() {
            return err(format!("nothing changed since {component} {base}"));
        }
        let bundle = session.to_bundle();
        let Some(editable) = editable(&component) else {
            return err(format!("{component} cannot be edited"));
        };
        if let Err(why) = rules::check(editable, session.original(), &bundle) {
            return err(format!("not built: {why}"));
        }
        if bundle.len() > crate::MAX_BUNDLE {
            return err(format!(
                "not built: the source is over {} KiB",
                crate::MAX_BUNDLE / 1024
            ));
        }
        if let Err(Error::NotFound) = env.artifact(&component, &base) {
            return err(format!(
                "not built: {component} {base}, which these edits start from, is no longer stored; diff shows them, and :edit {component} starts again from the active version"
            ));
        }
        let next = match env.info(&component) {
            Ok(info) => info.next().to_string(),
            Err(error) => return err(format!("cannot read {component}: {error:?}")),
        };
        let body = wire::encode_build(&BuildRequest {
            component: &component,
            label: &next,
            base: &base,
            // Founder until the session can hold a model draft (M9c step 4).
            origin: Origin::Founder,
            bundle: &bundle,
        });
        env.log(&format!(
            "building {component} {next} from {base} on the host"
        ));
        let body = match self.post(env, "/build", &body, BUILD_TIMEOUT_MS) {
            Ok(body) => body,
            Err(why) => return err(format!("build service: {why}")),
        };
        let built = match wire::decode_built(&body) {
            Ok(built) => built,
            Err(why) => return err(format!("build service: a malformed reply ({why})")),
        };
        let log = String::from_utf8_lossy(built.log);
        match built.result {
            BuildResult::Refused => err(format!("build refused: {}", log.trim())),
            BuildResult::Failed => self.failed(&log),
            BuildResult::Ok => {
                let checked = check_artifact(built.artifact, &component, &next, &bundle);
                if let Err(why) = checked {
                    return err(format!(
                        "the build service returned the wrong artifact: {why}"
                    ));
                }
                match env.put(&component, built.artifact, &base, built.commit) {
                    Ok(()) => {}
                    Err(Error::Invalid) => return err(refusal(env, &component, &next, &base)),
                    Err(Error::NoMemory) => {
                        return err("the store has no room for another upload");
                    }
                    Err(error) => return err(format!("the store refused {next}: {error:?}")),
                }
                if let Some(session) = self.session.as_mut() {
                    session.built = Some(next.clone());
                }
                ok(format!(
                    "built {next} from {base} (commit {}); :apply runs it",
                    short(built.commit)
                ))
            }
        }
    }

    /// A failed build: its first errors, with the cursor on the first.
    fn failed(&mut self, log: &str) -> String {
        let lines: Vec<&str> = log
            .lines()
            .filter(|line| line.contains("error"))
            .take(DIAGNOSTICS)
            .collect();
        if let Some(session) = self.session.as_mut()
            && let Some((path, line)) = lines.iter().find_map(|line| location(line))
            && session.goto(path, line)
        {
            let mut reply = err(MOVED);
            for line in &lines {
                reply.push('\n');
                reply.push_str(line);
            }
            return reply;
        }
        let mut reply = err("build failed");
        for line in if lines.is_empty() {
            log.lines().take(DIAGNOSTICS).collect()
        } else {
            lines
        } {
            reply.push('\n');
            reply.push_str(line);
        }
        reply
    }
}

/// The built artifact must parse, name this component and the next label, and
/// carry exactly the source that was sent.
pub(super) fn check_artifact(
    bytes: &[u8],
    component: &str,
    label: &str,
    bundle: &[u8],
) -> Result<(), String> {
    let parsed = artifact::parse(bytes)?;
    let manifest = &parsed.manifest;
    if manifest.component != component || manifest.version != label {
        return Err(format!(
            "it is {} {}, not {component} {label}",
            manifest.component, manifest.version
        ));
    }
    if parsed.source != bundle {
        return Err("its source is not what was sent".into());
    }
    Ok(())
}

/// `path:line:col: ...` from a compiler message.
pub(crate) fn location(line: &str) -> Option<(&str, usize)> {
    let mut parts = line.splitn(4, ':');
    let path = parts.next()?;
    let line = parts.next()?.parse().ok()?;
    parts.next()?.parse::<usize>().ok()?;
    Some((path.trim(), line))
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// Why the store refused an upload it would have taken when the build began.
fn refusal(env: &mut dyn Env, component: &str, next: &str, base: &str) -> String {
    let now = env.info(component).map(|info| info.next().to_string());
    if env.artifact(component, base).is_err() {
        return format!(
            "the store refused {next}: {base}, which it was built from, is no longer stored; :edit {component} starts again"
        );
    }
    match now {
        Ok(now) if now != next => {
            format!("the store refused {next}: the next upload is now {now}; build again")
        }
        _ => format!(
            "the store refused {next}: the active version changed, or the build service's reply did not match; build again"
        ),
    }
}
