//! Replaying the kernel's journal (M9d) into a restarted service: the open and
//! the commands run again under an environment that only reads the store. The
//! first reply whose digest differs stops it and drops the session, so the kernel
//! can replay the entries before it again.

use super::building::check_artifact;
use super::{Env, Service, err, fit, ok};
use crate::ipc::{self, Ipc};
use crate::journal::{self, NO_SESSION, quoted};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::Info;

/// A replay reads the store and logs; it never builds, uploads or reaches a
/// component, so neither `build` nor `try` can run in one. It never looks at a
/// build either (`handle` polls before every message but a replay).
struct Replaying<'a>(&'a mut dyn Env);

impl Env for Replaying<'_> {
    fn info(&mut self, component: &str) -> Result<Info, Error> {
        self.0.info(component)
    }

    fn artifact(&mut self, component: &str, label: &str) -> Result<Vec<u8>, Error> {
        self.0.artifact(component, label)
    }

    fn put(&mut self, _: &str, _: &[u8], _: &str, _: &str) -> Result<(), Error> {
        Err(Error::Permission)
    }

    fn build(&mut self, _: &[u8], _: u64) -> Result<Vec<u8>, Error> {
        Err(Error::Permission)
    }

    fn build_start(&mut self, _: &[u8], _: u64) -> Result<(), Error> {
        Err(Error::Permission)
    }

    fn build_poll(&mut self, _: bool) -> Result<Vec<u8>, Error> {
        Err(Error::Permission)
    }

    fn control(&mut self, _: &str, _: &str) -> Result<Vec<u8>, Error> {
        Err(Error::Permission)
    }

    fn log(&mut self, text: &str) {
        self.0.log(text);
    }
}

impl Service {
    /// `replay <from>`: runs journal entries `from` on, one a line. From the
    /// open it replaces any session, since the journal is what was answered.
    pub(super) fn replay(&mut self, env: &mut dyn Env, from: usize, entries: &str) -> String {
        if from == 0 {
            self.session = None;
            self.builds.moved = None;
            // The session is replaced: what was decided about a build is not.
            self.builds.decided = false;
        } else if self.session.is_none() {
            return err(format!("replay stopped at {from}: {NO_SESSION}"));
        }
        let mut env = Replaying(env);
        let mut done = from;
        for entry in entries.split('\n') {
            if let Err(why) = self.replay_entry(&mut env, entry) {
                self.session = None;
                return err(format!("replay stopped at {done}: {why}"));
            }
            done += 1;
        }
        env.log(&format!("replayed journal entries {from} to {}", done - 1));
        ok(format!("replayed {done}"))
    }

    fn replay_entry(&mut self, env: &mut dyn Env, entry: &str) -> Result<(), String> {
        let (digest, message) = journal::parse_entry(entry).ok_or("a malformed entry")?;
        if let Some(label) = message.strip_prefix("#built ") {
            self.rebuilt(env, label);
            return Ok(());
        }
        if let Some(place) = message.strip_prefix("#at ") {
            let session = self.session.as_mut().ok_or("no session")?;
            let (path, line) = place.rsplit_once(':').ok_or("a place with no line")?;
            let line = line.parse().map_err(|_| "a bad line")?;
            return match session.goto(path, line) {
                true => Ok(()),
                false => Err(format!("{path} is not in the session")),
            };
        }
        let reply = match ipc::decode_ipc(message.as_bytes()) {
            Ok(Ipc::Open {
                component,
                label,
                at,
            }) => self.open(env, component, label, at),
            Ok(Ipc::Cmd(line)) => self.command(env, line),
            _ => {
                return Err(String::from(
                    "an entry that is neither an open nor a command",
                ));
            }
        };
        let reply = fit(reply);
        if journal::reply_digest(message.as_bytes(), reply.as_bytes()) != digest {
            return Err(format!("it now answers {:?}", quoted(&reply)));
        }
        Ok(())
    }

    /// `#built <label>`: the session was built as that upload, which counts again
    /// only if the store has it and its source is this text.
    fn rebuilt(&mut self, env: &mut dyn Env, label: &str) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let bundle = session.to_bundle();
        let same = env
            .artifact(&session.component, label)
            .is_ok_and(|bytes| check_artifact(&bytes, &session.component, label, &bundle).is_ok());
        if same {
            session.built = Some(label.to_string());
        }
    }
}
