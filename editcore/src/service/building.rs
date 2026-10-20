//! Building: the edited source goes to the host's build service in the
//! background, and `build` answers at once (`job.rs` follows the job). This file
//! holds the checks before a build and what checking its result needs.

use super::job::{Job, Running, busy_reply};
use super::{BUILD_TIMEOUT_MS, Env, Service, err, ok};
use crate::wire::{self, BuildRequest, Origin};
use crate::{editable, rules};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use livecore::artifact;
use llmproto::http;

impl Service {
    /// A POST of `body` to the build service's `path`, with the token.
    pub(super) fn request(&self, path: &str, body: &[u8]) -> Vec<u8> {
        let token = [("X-Kai-Token", self.token.as_str())];
        http::post(&self.host, path, "application/octet-stream", &token, body)
    }

    /// `build`: starts building the session's text as the next upload, or says
    /// why not; `ok building <next> from <base>` when it started.
    pub(super) fn build(&mut self, env: &mut dyn Env) -> String {
        if let Some(label) = self.busy(env) {
            return busy_reply(&label);
        }
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
        let request = self.request("/build", &body);
        // A build that starts makes the last one's answer stale.
        self.builds.last = None;
        match env.build_start(&request, BUILD_TIMEOUT_MS) {
            Ok(()) => {
                let reply = ok(format!("building {next} from {base}"));
                self.builds.job = Some(Job::Running(Running {
                    component,
                    next,
                    base,
                    bundle,
                }));
                reply
            }
            Err(Error::Busy) => busy_reply(&next),
            Err(error) => err(format!("build service: {error:?}")),
        }
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
pub(super) fn location(line: &str) -> Option<(&str, usize)> {
    let mut parts = line.splitn(4, ':');
    let path = parts.next()?;
    let line = parts.next()?.parse().ok()?;
    parts.next()?.parse::<usize>().ok()?;
    Some((path.trim(), line))
}

pub(super) fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// Why the store refused an upload it would have taken when the build began.
pub(super) fn refusal(env: &mut dyn Env, component: &str, next: &str, base: &str) -> String {
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

/// The body of the build service's `200` response, or what went wrong.
pub(super) fn body_of(response: &[u8]) -> Result<Vec<u8>, String> {
    let response = http::parse_response(response, true).map_err(|error| format!("{error:?}"))?;
    if response.status != 200 {
        let body = String::from_utf8_lossy(&response.body);
        let first = body.lines().next().unwrap_or("");
        return Err(format!("HTTP {} {}", response.status, first));
    }
    Ok(response.body)
}
