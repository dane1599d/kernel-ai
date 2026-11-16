//! The hosts the person allowed for apps, as the agent's grounding reads them
//! (ADR 0002 D3; founder decision 2026-11-16, which reverses #751's "no app
//! grants"): a `kai-grants 1` host grant, made only by the person's Allow on
//! the trusted path's card, grounds a fetch from that one exact host.
//!
//! - **Lazy.** The grants file is read only when the rule reaches the grant
//!   check (the host is neither typed nor a table word, and no clause negates
//!   it), and at most once for each [`AppGrants`]: one decision pass. A
//!   Settings revoke is seen on the next pass.
//! - **Installed apps only.** A grant counts while `pkg` says its package is
//!   installed (`actions <package>`; a window's service `<x>-window` runs
//!   package `<x>`). Any other answer from `pkg`, or none, counts the grant
//!   for nothing.
//! - **Fails closed.** A file out of shape grounds nothing; the audit says so
//!   once for each new reason, not on every line.
//! - **Plain hosts only.** An object with no dot, or the `NetDest` words `any`
//!   and `none`, grounds nothing, whatever the file says.
//!
//! What a grant grounds is decided in `agentabi::consent`: only a fetch the
//! line alone decided, never one read content steered or whose path the line
//! does not hold, and never a destination for data (P-F).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{OnceCell, RefCell};

use crate::Host;

/// The `NetDest` words that are not hosts.
const NOT_HOSTS: [&str; 2] = ["any", "none"];

/// One decision pass's view of the app grants.
pub(crate) struct AppGrants<'a> {
    host: &'a dyn Host,
    /// The last reason the file was not read, so the audit says each once.
    said: &'a RefCell<Option<String>>,
    /// Each grant as (package, host), once read.
    held: OnceCell<Vec<(String, nethosts::Host)>>,
    /// What `pkg` said of each package asked about in this pass.
    installed: RefCell<Vec<(String, bool)>>,
}

impl<'a> AppGrants<'a> {
    pub(crate) fn new(host: &'a dyn Host, said: &'a RefCell<Option<String>>) -> Self {
        Self {
            host,
            said,
            held: OnceCell::new(),
            installed: RefCell::new(Vec::new()),
        }
    }

    fn held(&self) -> &[(String, nethosts::Host)] {
        self.held
            .get_or_init(|| match crate::netask::read_store(self.host) {
                Ok(grants) => {
                    *self.said.borrow_mut() = None;
                    agent_hosts(&grants)
                }
                Err(why) => {
                    let mut said = self.said.borrow_mut();
                    if said.as_deref() != Some(why.as_str()) {
                        self.host
                            .audit(&format!("agent grounding: {why}; no grant grounds"));
                        *said = Some(why);
                    }
                    Vec::new()
                }
            })
    }

    fn installed(&self, package: &str) -> bool {
        if let Some(&(_, known)) = self.installed.borrow().iter().find(|(p, _)| p == package) {
            return known;
        }
        let known = match self.host.installed_rows(package) {
            Ok(_) => true,
            Err(why) => {
                if !why.ends_with(" is not installed") {
                    self.host.audit(&format!(
                        "agent grounding: {package}'s grants count for nothing: {why}"
                    ));
                }
                false
            }
        };
        self.installed
            .borrow_mut()
            .push((package.to_string(), known));
        known
    }
}

impl nethosts::Granted for AppGrants<'_> {
    fn granted(&self, host: &nethosts::Host) -> bool {
        self.held()
            .iter()
            .filter(|(_, granted)| granted == host)
            .any(|(package, _)| self.installed(package))
    }
}

/// Each grant of `grants` that could ground a host, as (package, host).
fn agent_hosts(grants: &crate::netgrant::NetGrants) -> Vec<(String, nethosts::Host)> {
    grants
        .all()
        .iter()
        .filter(|grant| grant.object.contains('.') && !NOT_HOSTS.contains(&grant.object.as_str()))
        .filter_map(|grant| {
            let host = nethosts::Host::parse(&grant.object).ok()?;
            let package = grant.app.strip_suffix("-window").unwrap_or(&grant.app);
            Some((package.to_string(), host))
        })
        .collect()
}
