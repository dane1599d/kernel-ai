//! Per-app network grants in the shell (P5 task 13, the OS half of
//! [`crate::netgrant`]): what an app's connection to a host comes to, the ask
//! for a new host on the trusted path's card, the grants file, and Settings'
//! list and revoke, each with `undo`.
//!
//! - **The file is the one copy.** The host grants are kept as `kai-grants 1`
//!   host lines in `/system/state/netgrants`, read through the host on each
//!   decision and written before a change is told, as the withheld rows are
//!   (`crate::grants`). A file out of shape fails closed: no host is granted,
//!   nothing is asked, and nothing overwrites it until it is put right. At
//!   start, `session` grants are dropped.
//! - **The ask.** A host inside the app's package ceiling with no grant waits
//!   as a card, which the kernel's terminal takes (`CARD_READY`, then
//!   [`termcore::menu::TAKE_CARD`]), as Kai's install card does: no typed line
//!   asked for it, so the highlight starts on Deny. Only its Allow, its `ONCE`
//!   checked, writes a grant: "Allow" for this session, "Always allow on
//!   <host>" for good.
//! - **Undo.** An Allow's undo takes the grant back; a Settings revoke's undo
//!   puts it back under its own id. A revoked grant is kept in memory for its
//!   undo, so that undo works until the shell restarts, as a profile's does.

use crate::actions::{How, Pending};
use crate::actlog::{Change, Entry, Form, State};
use crate::netgrant::{Ask, Decision, NetGrants};
use crate::script::Checked;
use crate::tiers::Tier;
use crate::undo::Inverse;
use crate::{Host, Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kaigrants::{Grant, Life};
use netdest::Dest;
use termcore::menu::{Answer, Menu, Origin, read_answer};

/// Most asks waiting for the terminal at once, every app together.
pub const MAX_CARDS: usize = 8;
/// Most revoked grants kept for their undo.
const MAX_REVOKED: usize = 32;
/// The consent a Settings change is logged with.
const CLICK: &str = "user-click:settings";

/// One ask waiting for, or on, the card: the app's host and its ceiling, as
/// its package named it when it asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NetAsking {
    pub ask: Ask,
    pub ceiling: Dest,
}

/// What an app's connection to a host comes to, as its host process is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connect {
    /// Granted: the app's `NetDest`, exactly its granted hosts, to narrow its
    /// ceiling handle to.
    Granted(Dest),
    /// Not yet: the person is asked on the terminal's card; ask again after.
    Asked,
    /// Never, or not now, and why.
    Refused(String),
}

impl Connect {
    /// The one-line answer: `granted <netdest>`, `asked` or `refused <why>`.
    pub fn wire(&self) -> String {
        match self {
            Self::Granted(dest) => format!("granted {dest}"),
            Self::Asked => String::from("asked"),
            Self::Refused(why) => format!("refused {why}"),
        }
    }
}

/// The grants as the file holds them: none when there is no file.
fn read_store(host: &dyn Host) -> Result<NetGrants, String> {
    match host.net_grants_read() {
        None => Ok(NetGrants::new()),
        Some(text) => NetGrants::load(&text)
            .map_err(|why| format!("the network grants on disk are out of shape ({why})")),
    }
}

fn write_store(host: &dyn Host, grants: &NetGrants) -> Result<(), String> {
    host.net_grants_write(&grants.text()?)
}

/// `this session` or `always`.
fn life_word(life: Life) -> &'static str {
    match life {
        Life::Session => "this session",
        Life::Persistent => "always",
    }
}

/// Checks `/system/state/netgrants` at start and drops the `session` grants:
/// a file out of shape is audited and left as it is.
pub(crate) fn at_boot(host: &dyn Host) {
    let mut grants = match read_store(host) {
        Ok(grants) => grants,
        Err(why) => {
            host.audit(&format!(
                "{why}; no app reaches a new host until it is put right"
            ));
            return;
        }
    };
    let before = grants.clone();
    grants.at_boot();
    if grants != before
        && let Err(why) = write_store(host, &grants)
    {
        host.audit(&format!(
            "the session network grants were not dropped: {why}"
        ));
    }
}

impl Shell {
    /// What app `app`'s connection to `host` comes to, under `ceiling`, its
    /// package's `NetDest` in its text form. An ask waits as a card.
    pub fn net_connect(&mut self, app: &str, host: &str, ceiling: &str) -> Connect {
        let Ok(ceiling) = Dest::parse(ceiling) else {
            return Connect::Refused(format!("{app}'s package names no destination it may reach"));
        };
        let grants = match read_store(self.host.as_ref()) {
            Ok(grants) => grants,
            Err(why) => return Connect::Refused(why),
        };
        match grants.decide(app, host, &ceiling) {
            Decision::Granted(_) => match grants.dest_for(app, &ceiling) {
                Some(dest) => Connect::Granted(dest),
                None => Connect::Refused(String::from("the grant is outside the package")),
            },
            Decision::Refused(why) => Connect::Refused(why),
            Decision::Ask(ask) => self.queue_net_card(NetAsking { ask, ceiling }),
        }
    }

    /// Puts `asking` in line for the terminal's card, once.
    fn queue_net_card(&mut self, asking: NetAsking) -> Connect {
        let same = |a: &NetAsking| a.ask.app == asking.ask.app && a.ask.host == asking.ask.host;
        let on_card = self
            .pending
            .as_ref()
            .and_then(|p| p.net.as_ref())
            .is_some_and(same);
        if on_card || self.net_cards.iter().any(same) {
            return Connect::Asked;
        }
        if self.net_cards.len() >= MAX_CARDS {
            return Connect::Refused(String::from("too many network asks wait; answer one first"));
        }
        self.host.audit(&format!(
            "{} asks to connect to {}",
            asking.ask.app, asking.ask.host
        ));
        self.net_cards.push(asking);
        Connect::Asked
    }

    /// Whether a network ask waits for the terminal's card.
    pub(crate) fn net_card_waiting(&self) -> bool {
        !self.net_cards.is_empty()
    }

    /// The oldest waiting network ask, on the card; `None` when none waits.
    /// One granted or refused since it was put in line is dropped.
    pub(crate) fn take_net_card(&mut self) -> Option<Reply> {
        while !self.net_cards.is_empty() {
            let asking = self.net_cards.remove(0);
            let (app, host) = (&asking.ask.app, &asking.ask.host);
            let still = read_store(self.host.as_ref())
                .map(|grants| grants.decide(app, host, &asking.ceiling));
            if let Ok(Decision::Ask(_)) = still {
                return Some(self.net_ask(asking));
            }
        }
        None
    }

    /// Opens the card for `asking`.
    fn net_ask(&mut self, asking: NetAsking) -> Reply {
        let id = self.take_id();
        let ask = &asking.ask;
        let request = format!("{} connects to {} (the app asked)", ask.app, ask.host);
        let target = format!("on {}", ask.host);
        let menu = Menu::new(id, &ask.question, None, Some(&target), Origin::Added)
            .and_then(|menu| menu.with_call(&ask.call));
        if menu.is_err() {
            self.record(
                id,
                &request,
                &ask.call,
                "refused: it cannot be shown on the card",
            );
            return Reply::ok("");
        }
        self.host.audit(&format!("request {id}: {} asks", ask.call));
        let checked = Checked::shown_only(&ask.call);
        let encoded = Menu::encode(id, Origin::Added, &ask.question, "", &target, &ask.call);
        self.pending = Some(Pending {
            id,
            request,
            checked,
            asking: Vec::new(),
            created_ns: self.host.now_ns(),
            how: How {
                form: Form::Call,
                tier: Tier::Ask,
                consent: String::from("asked"),
            },
            call: None,
            offer: None,
            menu: Some(ask.call.clone()),
            pkg: None,
            standing: None,
            model: None,
            app: None,
            install: None,
            net: Some(asking),
        });
        Reply::menu(id, encoded)
    }

    /// The answer to a network card: an Allow whose `ONCE` checks writes the
    /// grant, for this session or, on "Always allow on <host>", for good.
    pub(crate) fn answer_net(&mut self, pending: Pending, text: &str) -> Reply {
        let id = pending.id;
        let Some(asking) = pending.net.clone() else {
            return Reply::err("nothing to confirm");
        };
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        let answer = read_answer(text.trim()).map_or(Answer::Deny, |(answer, _)| answer);
        let (app, host) = (asking.ask.app.as_str(), asking.ask.host.as_str());
        let made = self.write_net_grant(id, &asking, answer);
        let (outcome, change, reply) = match made {
            Ok((grant, existed)) => {
                let life = life_word(grant.life);
                let change = Change {
                    action: String::from("net_grant"),
                    owner: String::from("shell"),
                    res: format!("{app}'s network to {host}"),
                    before: String::from(if existed { "this session" } else { "asks" }),
                    after: String::from(life),
                    // Making a session grant lasting is not taken back alone.
                    inverse: (!existed).then_some(Inverse::NetGrant {
                        id: grant.id,
                        held: true,
                    }),
                };
                let reply = Reply::ok(format!(
                    "{app} may connect to {host} ({life}). undo takes it back"
                ));
                (String::from("ran"), Some(change), reply)
            }
            Err(why) => (
                format!("refused: {why}"),
                None,
                Reply::err(format!("{app} was not allowed {host}: {why}")),
            ),
        };
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: Some(Tier::Ask),
            pack: None,
            consent: String::from("yes"),
            state: if outcome == "ran" {
                State::Done
            } else {
                State::Nothing
            },
            request: pending.request.clone(),
            script: asking.ask.call.clone(),
            outcome,
            changes: change.into_iter().collect(),
        });
        self.save_log();
        reply
    }

    /// Writes the grant `answer` makes for `asking`: the grant, and whether
    /// it was held already (a session grant made lasting).
    fn write_net_grant(
        &mut self,
        id: u64,
        asking: &NetAsking,
        answer: Answer,
    ) -> Result<(Grant, bool), String> {
        let host = self.host.as_ref();
        let mut grants = read_store(host)?;
        let (app, name) = (&asking.ask.app, &asking.ask.host);
        let existed = grants.of(app).iter().any(|g| &g.object == name);
        let made = grants
            .answer(&asking.ask, &asking.ceiling, answer, id, host.now_ns())
            .map_err(String::from)?
            .ok_or_else(|| String::from("declined"))?;
        write_store(host, &grants)?;
        let grant = grants
            .of(app)
            .into_iter()
            .find(|g| g.id == made)
            .cloned()
            .ok_or_else(|| String::from("the grant was not kept"))?;
        Ok((grant, existed))
    }

    /// Settings' `net`: one line a grant, `#<id> <app> <host> <life>`.
    pub(crate) fn net_lines(&self) -> Result<String, String> {
        let grants = read_store(self.host.as_ref())?;
        let text = grants.text()?;
        let parsed = kaigrants::parse(&text).map_err(|e| e.why.to_string())?;
        let lines: Vec<String> = parsed
            .grants
            .iter()
            .map(|g| {
                let life = match g.life {
                    Life::Session => "session",
                    Life::Persistent => "always",
                };
                format!("#{} {} {} {life}", g.id, g.app, g.object)
            })
            .collect();
        Ok(lines.join("\n"))
    }

    /// Settings' `net-revoke <id>` from a click: the grant goes, logged with
    /// its undo, which puts it back under its own id.
    pub(crate) fn net_revoke_by_click(
        &mut self,
        request: &str,
        which: &str,
    ) -> Result<String, String> {
        let id = which
            .trim_start_matches('#')
            .parse::<u32>()
            .map_err(|_| String::from("net-revoke takes a grant number"))?;
        let grant = self.take_net_grant(id)?;
        let words = format!("{} may not connect to {}", grant.app, grant.object);
        let entry = self.take_id();
        self.log.push(Entry {
            id: entry,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: Some(Tier::Run),
            pack: None,
            consent: String::from(CLICK),
            state: State::Done,
            request: String::from(request),
            script: format!("net_revoke({id})"),
            outcome: words.clone(),
            changes: alloc::vec![Change {
                action: String::from("net_revoke"),
                owner: String::from("shell"),
                res: format!("{}'s network to {}", grant.app, grant.object),
                before: String::from(life_word(grant.life)),
                after: String::from("asks"),
                inverse: Some(Inverse::NetGrant { id, held: false }),
            }],
        });
        self.save_log();
        Ok(words)
    }

    /// Removes grant `id` from the file and keeps it for its undo.
    fn take_net_grant(&mut self, id: u32) -> Result<Grant, String> {
        let host = self.host.as_ref();
        let mut grants = read_store(host)?;
        let grant = grants
            .revoke(id)
            .ok_or_else(|| format!("there is no network grant #{id}"))?;
        write_store(host, &grants)?;
        if self.net_revoked.len() >= MAX_REVOKED {
            self.net_revoked.remove(0);
        }
        self.net_revoked.push(grant.clone());
        Ok(grant)
    }

    /// Why Kai's own `undo` may not revert the newest live entry: it would
    /// give an app back a network grant (a revoke undone, or an undo of an
    /// Allow undone), which only the card's Allow, the person's typed `undo`
    /// or Settings may do. Taking a grant back stays open to it.
    pub(crate) fn kai_undo_refusal(&self) -> Option<String> {
        let entry = self.log.get(self.log.newest_live()?)?;
        let widens = entry
            .changes
            .iter()
            .any(|c| matches!(c.inverse, Some(Inverse::NetGrant { held: false, .. })));
        widens.then(|| {
            format!(
                "#{} gives an app back a network grant, which only you may undo: type `undo`, or allow it again when the app asks",
                entry.id
            )
        })
    }

    /// `undo` of a network grant (`held`: it is taken back) or of a revoke
    /// (it is put back under its own id).
    pub(crate) fn revert_net_grant(&mut self, id: u32, held: bool) -> Result<String, String> {
        if held {
            let grant = self.take_net_grant(id)?;
            return Ok(format!("{} may not connect to {}", grant.app, grant.object));
        }
        let at = self
            .net_revoked
            .iter()
            .position(|g| g.id == id)
            .ok_or_else(|| format!("network grant #{id} is no longer kept to put back"))?;
        let host = self.host.as_ref();
        let mut grants = read_store(host)?;
        let grant = self.net_revoked[at].clone();
        grants.restore(grant.clone()).map_err(String::from)?;
        write_store(host, &grants)?;
        self.net_revoked.remove(at);
        Ok(format!(
            "{} may connect to {} again",
            grant.app, grant.object
        ))
    }
}
