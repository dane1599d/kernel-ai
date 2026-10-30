//! The app nap's driver (P3 task 12b): [`crate::nap::Nap`]'s policy against a
//! running [`Host`], which comphost calls around each request, each window
//! focus and each wait. Every entry first [`Napper::sync`]s with the host, so
//! a resume from any side (a window's close or the agent's action through
//! `Host::wake`, a kernel `ctl resume`) ends the nap, and a suspend that is not
//! the nap's own (a standing instruction's) is never woken by focus.

use crate::host::{Env, Exit, Host};
use crate::nap::Nap;
use alloc::format;
use alloc::vec::Vec;

/// The keep-running switch, from the kernel only: `ctl keep-running on|off`.
pub const KEEP_RUNNING: &str = "ctl keep-running ";

/// What happened, for the host's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Napped {
    /// It napped, unused.
    Napped,
    /// It woke from its nap; the resume's reply.
    Woke(Vec<u8>),
}

pub struct Napper {
    pub nap: Nap,
}

/// Control requests that answer for a napping app as it is, without waking
/// it: its status, and the suspend and resume that a rule or the agent sends.
fn as_it_is(request: &[u8]) -> bool {
    [b"ctl status".as_slice(), b"ctl suspend", b"ctl resume"].contains(&request)
}

impl Napper {
    pub fn new(nap: Nap) -> Self {
        Self { nap }
    }

    /// The nap ends whenever the app is running again, whoever resumed it.
    pub fn sync<E: Env>(&mut self, host: &Host<E>, now_ms: u64) {
        if self.nap.is_napping() && !host.suspended() {
            self.nap.woke(now_ms);
        }
    }

    /// How long to wait, at most `wait` (0 forever), so a nap comes on time.
    pub fn wait<E: Env>(&self, host: &Host<E>, wait: u64, now_ms: u64) -> u64 {
        let nap = (!host.suspended())
            .then(|| self.nap.wait_ms(now_ms))
            .flatten()
            .map(|ms| ms.max(1));
        match (wait, nap) {
            (0, Some(nap)) => nap,
            (wait, Some(nap)) => wait.min(nap),
            (wait, None) => wait,
        }
    }

    /// Before a request from `caller`: the keep-running switch is answered
    /// here, and turning it on wakes a napping app; a rule's `ctl suspend` takes a napping app over (so focus no
    /// longer wakes it) and is answered here; status, suspend and resume pass
    /// as they are; anything else is use and wakes a napping app first, so even
    /// `ctl swap` finds it running. `Some` is the reply, `None` hands the
    /// request to the host.
    pub fn request<E: Env>(
        &mut self,
        host: &mut Host<E>,
        caller: u64,
        request: &[u8],
        now_ms: u64,
    ) -> Result<(Option<Vec<u8>>, Option<Napped>), Exit> {
        self.sync(host, now_ms);
        if caller == 0
            && let Some(on) = request.strip_prefix(KEEP_RUNNING.as_bytes())
        {
            let reply: &[u8] = match on {
                b"on" => b"ok keep-running on",
                b"off" => b"ok keep-running off",
                _ => b"err usage: ctl keep-running on|off",
            };
            if on == b"on" || on == b"off" {
                self.nap.keep_running(on == b"on");
            }
            // Kept running from now: a napping app wakes; a rule's suspend
            // stays the rule's (`wake` ends only the nap's own).
            let woke = match on {
                b"on" => self.wake(host, now_ms)?,
                _ => None,
            };
            return Ok((Some(reply.to_vec()), woke));
        }
        if caller == 0 && self.nap.is_napping() && request == b"ctl suspend" {
            // Suspended already: the rule's suspend now holds it.
            self.nap.woke(now_ms);
            let name = host.name().as_bytes().to_vec();
            let mut reply = b"ok suspended ".to_vec();
            reply.extend_from_slice(&name);
            reply.extend_from_slice(b" (it was napping)");
            return Ok((Some(reply), None));
        }
        if caller == 0 && as_it_is(request) {
            return Ok((None, None));
        }
        self.nap.used(now_ms);
        Ok((None, self.wake(host, now_ms)?))
    }

    /// After a request: whatever it did, the nap follows the host.
    pub fn after<E: Env>(&mut self, host: &Host<E>, now_ms: u64) {
        self.sync(host, now_ms);
    }

    /// One of its windows took focus: use, which wakes only its own nap.
    pub fn focus<E: Env>(
        &mut self,
        host: &mut Host<E>,
        focused: bool,
        now_ms: u64,
    ) -> Result<Option<Napped>, Exit> {
        self.sync(host, now_ms);
        if !focused {
            return Ok(None);
        }
        self.nap.used(now_ms);
        self.wake(host, now_ms)
    }

    fn wake<E: Env>(&mut self, host: &mut Host<E>, now_ms: u64) -> Result<Option<Napped>, Exit> {
        if !self.nap.is_napping() {
            return Ok(None);
        }
        let reply = host.wake()?;
        self.sync(host, now_ms);
        Ok(reply.map(Napped::Woke))
    }

    /// When the wait ends: nap if it is due and the app serves alone.
    pub fn tick<E: Env>(
        &mut self,
        host: &mut Host<E>,
        now_ms: u64,
    ) -> Result<Option<Napped>, Exit> {
        self.sync(host, now_ms);
        if host.suspended() || !self.nap.due(now_ms) {
            return Ok(None);
        }
        let request = format!("{}suspend", kabi::live::CONTROL_PREFIX);
        let reply = host.handle(0, request.as_bytes())?;
        if reply.starts_with(b"ok suspended") {
            self.nap.napped();
            Ok(Some(Napped::Napped))
        } else {
            // On probation or unsaved: try again a whole nap later.
            self.nap.used(now_ms);
            Ok(None)
        }
    }
}
