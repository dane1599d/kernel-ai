//! Resource profiles in the shell (P3 task 12; the founder's decision of
//! 2026-10-27): `profile` lists them, `profile <name>` switches to one at once,
//! and `undo` puts every process's previous class and quota back. A standing
//! instruction can switch one too (`on focus(game) -> profile_set(gaming)`).
//!
//! Profiles live in RAM until `/system/settings` serves them. A switch resolves
//! the profile against the running processes and hands the settings to
//! [`Host::profile_apply`](crate::Host::profile_apply): the supervisor sets them
//! with PROC_CLASS_SET and PROC_QUOTA_SET, which need SUPERVISE, and the kernel's
//! floors keep the system from being starved whatever a profile says.

use crate::actlog::{Change, Entry, Form, State};
use crate::tiers::Tier;
use crate::undo::Inverse;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kaiprofile::{Profile, Setting};

/// Most switches whose previous settings `undo` keeps.
const MAX_BACK: usize = 16;

/// The profiles and what `undo` puts back.
pub(crate) struct Profiles {
    list: Vec<Profile>,
    /// The profile in force; a boot starts on `default`.
    now: String,
    /// (key, the profile it was, every process's settings before the switch).
    back: Vec<(u64, String, Vec<Setting>)>,
    next_key: u64,
}

impl Default for Profiles {
    fn default() -> Self {
        Self {
            list: kaiprofile::builtin(),
            now: String::from("default"),
            back: Vec::new(),
            next_key: 1,
        }
    }
}

/// How a switch is logged: the form, the consent and the tier of its entry.
pub(crate) struct Logged<'a> {
    pub form: Form,
    pub consent: String,
    pub request: &'a str,
}

impl Shell {
    /// `profile`: every profile, the one in force marked.
    pub(crate) fn list_profiles(&self) -> Reply {
        let lines: Vec<String> = self
            .profiles
            .list
            .iter()
            .map(|p| match p.name == self.profiles.now {
                true => format!("{} (now)", p.name),
                false => p.name.clone(),
            })
            .collect();
        Reply::ok(lines.join("\n"))
    }

    /// `profile <name>`: plainly asked, so it runs at once, with `undo`.
    pub(crate) fn profile_command(&mut self, request: &str, name: &str) -> Reply {
        let id = self.next_id;
        self.next_id += 1;
        let logged = Logged {
            form: Form::Call,
            consent: String::from("asked"),
            request,
        };
        match self.switch_profile(id, name.trim(), logged) {
            Ok((was, _)) => Reply::ok(format!("Done: profile {} (was {was}). undo", name.trim())),
            Err(why) => Reply::err(why),
        }
    }

    /// Switches to profile `name` and logs it as entry `id`; the profile it was,
    /// and the key `undo` puts back.
    pub(crate) fn switch_profile(
        &mut self,
        id: u64,
        name: &str,
        logged: Logged,
    ) -> Result<(String, u64), String> {
        let profile = self
            .profiles
            .list
            .iter()
            .find(|p| p.name == name)
            .cloned()
            .ok_or_else(|| format!("no profile {name}; `profile` lists them"))?;
        let metrics = self
            .host
            .metrics()
            .map_err(|e| format!("cannot read the processes ({e:?})"))?;
        let running: Vec<String> = metrics
            .processes
            .iter()
            .take(metrics.process_count as usize)
            .map(|p| String::from(p.name()))
            .collect();
        // A profile sets apps: a running service the supervisor keeps is the
        // system's, and is left alone.
        let services: Vec<String> = self
            .host
            .services()
            .map(|list| {
                list.into_iter()
                    .filter(|s| !s.stopped)
                    .map(|s| s.name)
                    .collect()
            })
            .unwrap_or_default();
        let running: Vec<&str> = running
            .iter()
            .map(String::as_str)
            .filter(|name| !services.iter().any(|s| s == name))
            .collect();
        let focused = self.host.focused_app();
        let settings = profile.resolve(&running, focused.as_deref());
        let before = self.host.profile_apply(&settings)?;
        let was = core::mem::replace(&mut self.profiles.now, String::from(name));
        let key = self.keep_back(was.clone(), before);
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: logged.form,
            of: None,
            tier: Some(Tier::Run),
            pack: None,
            consent: logged.consent,
            state: State::Done,
            request: String::from(logged.request),
            script: format!("profile_set(\"{name}\")"),
            outcome: format!("{} processes set", settings.len()),
            changes: alloc::vec![Change {
                action: String::from("profile_set"),
                owner: String::from("supervisor"),
                res: String::from("profile"),
                before: was.clone(),
                after: String::from(name),
                inverse: Some(Inverse::Profile { key }),
            }],
        });
        self.host.audit(&format!(
            "profile {name} (was {was}): {} processes set",
            settings.len()
        ));
        Ok((was, key))
    }

    /// `undo` of a switch: every process's settings from before it, and the
    /// profile it was; its text and the key that redoes it.
    pub(crate) fn revert_profile(&mut self, key: u64) -> Result<(String, u64), String> {
        let at = self
            .profiles
            .back
            .iter()
            .position(|(k, _, _)| *k == key)
            .ok_or_else(|| String::from("that profile switch is too old to undo"))?;
        let (_, name, settings) = self.profiles.back.remove(at);
        let current = self.host.profile_apply(&settings)?;
        let was = core::mem::replace(&mut self.profiles.now, name.clone());
        let redo = self.keep_back(was, current);
        Ok((format!("profile back to {name}"), redo))
    }

    fn keep_back(&mut self, name: String, settings: Vec<Setting>) -> u64 {
        let key = self.profiles.next_key;
        self.profiles.next_key += 1;
        if self.profiles.back.len() == MAX_BACK {
            self.profiles.back.remove(0);
        }
        self.profiles.back.push((key, name, settings));
        key
    }
}
