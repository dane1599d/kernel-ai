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
/// Most Deny answers kept.
const MAX_DENIED: usize = MAX_CARDS;
/// How long a Deny refuses the app's connections to that host with no card:
/// the request that asked is told `refused`, and an app that asks again at
/// once does not raise the card again at once.
pub const DENY_HOLDS_NS: u64 = 60_000_000_000;
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

/// What an undo's change names a grant by: `<app>'s network to <host>`.
fn grant_res(app: &str, host: &str) -> String {
    format!("{app}'s network to {host}")
}

/// Checks `/system/state/netgrants` at start and drops the `session` grants:
/// a file out of shape is audited and left as it is. The highest id the file
/// held before the drop, for the shell's high-water mark.
pub(crate) fn at_boot(host: &dyn Host) -> u32 {
    let mut grants = match read_store(host) {
        Ok(grants) => grants,
        Err(why) => {
            host.audit(&format!(
                "{why}; no app reaches a new host until it is put right"
            ));
            return 0;
        }
    };
    let high = grants.high();
    let before = grants.clone();
    grants.at_boot();
    if grants != before
        && let Err(why) = write_store(host, &grants)
    {
        host.audit(&format!(
            "the session network grants were not dropped: {why}"
        ));
    }
    high
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
            // The person said no to this host on the card lately: refused
            // with no card until the Deny lapses. A grant, made since or put
            // back by `undo`, comes first.
            Decision::Ask(_) if self.net_denied_now(app, host) => {
                Connect::Refused(format!("you did not allow {app} to connect to {host}"))
            }
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
        let answer = read_answer(text.trim()).map_or(Answer::Deny, |(answer, _)| answer);
        if let Err(reply) = self.allowed(&pending, text) {
            // A Deny needs no proof: the app's waiting connection is refused.
            if answer == Answer::Deny {
                self.net_deny(&asking);
            }
            return reply;
        }
        let (app, host) = (asking.ask.app.as_str(), asking.ask.host.as_str());
        let made = self.write_net_grant(id, &asking, answer);
        let (outcome, change, reply) = match made {
            Ok((grant, existed)) => {
                let life = life_word(grant.life);
                let change = Change {
                    action: String::from("net_grant"),
                    owner: String::from("shell"),
                    res: grant_res(app, host),
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

    /// Keeps the person's Deny of `asking` for [`DENY_HOLDS_NS`].
    fn net_deny(&mut self, asking: &NetAsking) {
        let (app, host) = (&asking.ask.app, &asking.ask.host);
        self.net_denied.retain(|(a, h, _)| !(a == app && h == host));
        if self.net_denied.len() >= MAX_DENIED {
            self.net_denied.remove(0);
        }
        let now = self.host.now_ns();
        self.net_denied.push((app.clone(), host.clone(), now));
    }

    /// Whether a Deny of `app`'s `host` still holds; one that lapsed goes.
    fn net_denied_now(&mut self, app: &str, host: &str) -> bool {
        let now = self.host.now_ns();
        self.net_denied
            .retain(|(_, _, at)| now.saturating_sub(*at) < DENY_HOLDS_NS);
        self.net_denied
            .iter()
            .any(|(a, h, _)| a == app && h == host)
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
        // Never an id this run gave before, though its grant is gone from the file.
        grants.number_above(self.net_high);
        let (app, name) = (&asking.ask.app, &asking.ask.host);
        let existed = grants.of(app).iter().any(|g| &g.object == name);
        let made = grants
            .answer(&asking.ask, &asking.ceiling, answer, id, host.now_ns())
            .map_err(String::from)?
            .ok_or_else(|| String::from("declined"))?;
        write_store(host, &grants)?;
        self.net_high = self.net_high.max(made);
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
        let grant = self.take_net_grant(id, None)?;
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
                res: grant_res(&grant.app, &grant.object),
                before: String::from(life_word(grant.life)),
                after: String::from("asks"),
                inverse: Some(Inverse::NetGrant { id, held: false }),
            }],
        });
        self.save_log();
        Ok(words)
    }

    /// Removes grant `id` from the file and keeps it for its undo. With
    /// `logged`, refused unless the grant is still that app's host.
    fn take_net_grant(&mut self, id: u32, logged: Option<&str>) -> Result<Grant, String> {
        let host = self.host.as_ref();
        let mut grants = read_store(host)?;
        let grant = grants
            .revoke(id)
            .ok_or_else(|| format!("there is no network grant #{id}"))?;
        if let Some(logged) = logged
            && grant_res(&grant.app, &grant.object) != logged
        {
            return Err(format!("network grant #{id} is no longer {logged}"));
        }
        write_store(host, &grants)?;
        self.net_high = self.net_high.max(grant.id);
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
    ///
    /// Likewise a row Settings withheld from an app's effects: reverting an
    /// entry whose inverse gives the row back (a withhold undone, or an undo of
    /// a restore undone) is the person's, never Kai's.
    pub(crate) fn kai_undo_refusal(&self) -> Option<String> {
        let entry = self.log.get(self.log.newest_live()?)?;
        let widens = |inverse: &Option<Inverse>| match inverse {
            Some(Inverse::NetGrant { held: false, .. }) => Some("a network grant"),
            Some(Inverse::Grant {
                withheld: false, ..
            }) => Some("a row Settings withheld"),
            _ => None,
        };
        let what = entry.changes.iter().find_map(|c| widens(&c.inverse))?;
        Some(format!(
            "#{} gives an app back {what}, which only you may undo: type `undo`, or allow it again in Settings or when the app asks",
            entry.id
        ))
    }

    /// `undo` of a network grant (`held`: it is taken back) or of a revoke
    /// (it is put back under its own id). `res` is the logged change's
    /// `<app>'s network to <host>`: a grant under that id for another app or
    /// host is refused, never taken back or put back in its place.
    pub(crate) fn revert_net_grant(
        &mut self,
        id: u32,
        held: bool,
        res: &str,
    ) -> Result<String, String> {
        if held {
            let grant = self.take_net_grant(id, Some(res))?;
            return Ok(format!("{} may not connect to {}", grant.app, grant.object));
        }
        let at = self
            .net_revoked
            .iter()
            .rposition(|g| g.id == id && grant_res(&g.app, &g.object) == res)
            .ok_or_else(|| format!("network grant #{id} ({res}) is no longer kept to put back"))?;
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
