//! The hosts the person allowed for apps, as the agent's grounding reads them
//! (ADR 0002 D3; founder decision 2026-11-16, which reverses #751's "no app
//! grants"): a `kai-grants 1` host grant, made only by the person's Allow on
//! the trusted path's card, grounds a read from that one exact host.
//!
//! - **Lazy.** The grants file is read only when the rule reaches the grant
//!   check (the host is neither typed nor a table word, and no clause negates
//!   it), and at most once for each [`AppGrants`]: one decision pass. A
//!   Settings revoke is seen on the next pass.
//! - **Installed apps only.** A grant counts while `pkg` says its package is
//!   installed (`Host::package_installed`, a short wait; a window's service
//!   `<x>-window` runs package `<x>`). The answer, or its absence, is kept for
//!   [`INSTALLED_KEEP_NS`]. Any answer but "installed" counts the grant for
//!   nothing.
//! - **Fails closed.** A file out of shape grounds nothing. Each audit line is
//!   said once for its reason, not on every pass.
//! - **Plain hosts only.** An object with no dot, or the `NetDest` words `any`
//!   and `none`, grounds nothing, whatever the file says.
//! - **This session's grants** count only once the boot dropped last
//!   session's: if that write failed, only `always` grants count.
//!
//! What a grant grounds is decided in `agentabi::consent::consent_grant`: a
//! read the line asked for, with a path of at most three of its words, once a
//! plan, never one read content or the OS's state decided, and never a
//! destination for data (P-F).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{Cell, OnceCell, RefCell};
use kaigrants::Life;

use crate::Host;

/// The `NetDest` words that are not hosts.
const NOT_HOSTS: [&str; 2] = ["any", "none"];
/// How long `pkg`'s answer about a package is kept: about one line's work.
pub(crate) const INSTALLED_KEEP_NS: u64 = 5_000_000_000;
/// Most audit reasons remembered as said.
const SAID_MAX: usize = 16;

/// `pkg`'s answer about a package: installed, or why it is not known, and
/// when it was asked.
type Asked = (String, Result<bool, String>, u64);

/// What the shell keeps between decision passes.
#[derive(Default)]
pub(crate) struct Memo {
    /// Audit lines already said, so each reason is said once.
    said: RefCell<Vec<String>>,
    /// `pkg`'s answer about each package: installed, or why not known, and
    /// when it was asked.
    installed: RefCell<Vec<Asked>>,
    /// Set when the boot could not drop last session's grants.
    pub(crate) session_stale: Cell<bool>,
}

impl Memo {
    /// Audits `line` unless it was said already.
    fn say(&self, host: &dyn Host, line: String) {
        let mut said = self.said.borrow_mut();
        if said.contains(&line) {
            return;
        }
        host.audit(&line);
        if said.len() >= SAID_MAX {
            said.remove(0);
        }
        said.push(line);
    }

    /// Forgets the lines said that start with `lead`: their trouble is over.
    fn unsay(&self, lead: &str) {
        self.said
            .borrow_mut()
            .retain(|line| !line.starts_with(lead));
    }

    /// Forgets `pkg`'s answers: a package was installed or removed.
    pub(crate) fn forget_installed(&self) {
        self.installed.borrow_mut().clear();
    }

    fn installed(&self, host: &dyn Host, package: &str) -> Result<bool, String> {
        let now = host.now_ns();
        let fresh = |at: u64| now.saturating_sub(at) < INSTALLED_KEEP_NS;
        let kept = self
            .installed
            .borrow()
            .iter()
            .find(|(p, _, at)| p == package && fresh(*at))
            .map(|(_, answer, _)| answer.clone());
        if let Some(answer) = kept {
            return answer;
        }
        let answer = host.package_installed(package);
        let mut kept = self.installed.borrow_mut();
        kept.retain(|(p, _, at)| p != package && fresh(*at));
        kept.push((package.to_string(), answer.clone(), now));
        answer
    }
}

/// One decision pass's view of the app grants.
pub(crate) struct AppGrants<'a> {
    host: &'a dyn Host,
    memo: &'a Memo,
    /// Each grant as (package, host), once read.
    held: OnceCell<Vec<(String, nethosts::Host)>>,
}

/// The lead of the audit line for a grants file not read.
const FILE_LEAD: &str = "agent grounding: the network grants";

impl<'a> AppGrants<'a> {
    pub(crate) fn new(host: &'a dyn Host, memo: &'a Memo) -> Self {
        Self {
            host,
            memo,
            held: OnceCell::new(),
        }
    }

    fn held(&self) -> &[(String, nethosts::Host)] {
        self.held
            .get_or_init(|| match crate::netask::read_store(self.host) {
                Ok(grants) => {
                    self.memo.unsay(FILE_LEAD);
                    agent_hosts(&grants, self.memo.session_stale.get())
                }
                Err(why) => {
                    let why = why.trim_start_matches("the network grants ");
                    let line = format!("{FILE_LEAD} {why}; no grant grounds");
                    self.memo.say(self.host, line);
                    Vec::new()
                }
            })
    }

    fn installed(&self, package: &str) -> bool {
        let lead = format!("agent grounding: {package}'s grants count for nothing");
        match self.memo.installed(self.host, package) {
            Ok(installed) => {
                self.memo.unsay(&lead);
                installed
            }
            Err(why) => {
                self.memo.say(self.host, format!("{lead}: {why}"));
                false
            }
        }
    }
}

impl nethosts::Granted for AppGrants<'_> {
    fn granted(&self, host: &nethosts::Host) -> bool {
        !self.holders(host).is_empty()
    }

    fn holders(&self, host: &nethosts::Host) -> Vec<String> {
        let mut holders: Vec<String> = Vec::new();
        for (package, granted) in self.held() {
            if granted == host && !holders.contains(package) && self.installed(package) {
                holders.push(package.clone());
            }
        }
        holders
    }
}

/// Each grant of `grants` that could ground a host, as (package, host): only
/// `always` ones when last session's were not dropped at boot.
fn agent_hosts(
    grants: &crate::netgrant::NetGrants,
    session_stale: bool,
) -> Vec<(String, nethosts::Host)> {
    grants
        .all()
        .iter()
        .filter(|grant| !session_stale || grant.life == Life::Persistent)
        .filter(|grant| grant.object.contains('.') && !NOT_HOSTS.contains(&grant.object.as_str()))
        .filter_map(|grant| {
            let host = nethosts::Host::parse(&grant.object).ok()?;
            let package = grant.app.strip_suffix("-window").unwrap_or(&grant.app);
            Some((package.to_string(), host))
        })
        .collect()
}
