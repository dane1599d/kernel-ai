//! A model on trial (ADR 0008 §6 and §11): `model_use` names a stored model
//! for the next restart, since the kernel maps its model once per boot.
//! `updated` writes `kai-modeltrial 1` beside `/system/updates`' other records,
//! then has `fs` point `kai-modelhint 1` at the candidate. The next boot marks
//! the trial `attempted` ([`at_boot`]) before anything waits on `llm`, then
//! keeps the candidate once `llm` reports it loaded ([`decide`]), or points the
//! hint back at the model before and records why for the model manager. A boot
//! that finds a trial already `attempted` and never decided rolls it back at
//! once: the candidate hung that boot before `updated` could judge it, so it is
//! not tried again (K, 2026-10-30). `updated` started again within a boot
//! knows the mark is this boot's own, and goes on judging. While a boot judges
//! a trial, no switch is made ([`Switch::OnTrial`]), and the verdict is
//! written only over that trial's record ([`verdict`]).
//!
//! The record is text, one field a line:
//!
//! ```text
//! kai-modeltrial 1
//! candidate <64 hex>
//! fnv <16 hex>                (the candidate's FNV-1a64, as `fs` keeps it)
//! previous <64 hex> | none
//! state pending | attempted | kept | rolled-back
//! why <text>                  (rolled-back only)
//! ```
//!
//! ADR 0008's five smoke tasks after the model loads are not on the device
//! yet; until they are, loading with the candidate's FNV-1a64 is the bar.

use crate::Sha256;
use alloc::format;
use alloc::string::{String, ToString};
use comp_models::hex;

pub const MAGIC: &str = "kai-modeltrial 1";
/// Longest `why`, in bytes.
pub const MAX_WHY: usize = 200;

/// Where a trial stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// The next boot tries the candidate.
    Pending,
    /// A boot is trying it: written before that boot waits on `llm`.
    Attempted,
    Kept,
    /// The candidate did not load, and why; the hint is back on `previous`.
    RolledBack(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    pub candidate: Sha256,
    /// The candidate's FNV-1a64, which `fs` keeps for its blob: what `llm`
    /// must report loaded for the candidate to be kept ([`decide`]).
    pub fnv: u64,
    /// The model the machine ran before, which a rollback goes back to; none
    /// when it ran no model.
    pub previous: Option<Sha256>,
    pub state: State,
}

impl Trial {
    pub fn encode(&self) -> String {
        let previous = self
            .previous
            .map_or_else(|| "none".to_string(), |p| hex(&p));
        let state = match &self.state {
            State::Pending => "pending".to_string(),
            State::Attempted => "attempted".to_string(),
            State::Kept => "kept".to_string(),
            State::RolledBack(why) => match clean(why) {
                why if why.trim().is_empty() => "rolled-back\nwhy no reason was given".into(),
                why => format!("rolled-back\nwhy {why}"),
            },
        };
        format!(
            "{MAGIC}\ncandidate {}\nfnv {:016x}\nprevious {previous}\nstate {state}\n",
            hex(&self.candidate),
            self.fnv
        )
    }

    /// A record as [`Self::encode`] writes it, parsed strictly.
    pub fn parse(text: &str) -> Result<Trial, String> {
        let mut lines = text
            .strip_suffix('\n')
            .ok_or("a trial record ends without a newline")?
            .split('\n');
        if lines.next() != Some(MAGIC) {
            return Err(format!("not {MAGIC}"));
        }
        let mut field = |key: &str| -> Result<&str, String> {
            lines
                .next()
                .and_then(|line| line.strip_prefix(key))
                .and_then(|rest| rest.strip_prefix(' '))
                .ok_or_else(|| format!("no {key} line"))
        };
        let candidate = sha(field("candidate")?).ok_or("candidate is not a SHA-256")?;
        let fnv = field("fnv")?;
        let lower = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
        if fnv.len() != 16 || !fnv.bytes().all(lower) {
            return Err("fnv is not 16 lower-case hex digits".into());
        }
        let fnv = u64::from_str_radix(fnv, 16).map_err(|_| "fnv is not hex")?;
        let previous = match field("previous")? {
            "none" => None,
            text => Some(sha(text).ok_or("previous is not a SHA-256")?),
        };
        let state = match field("state")? {
            "pending" => State::Pending,
            "attempted" => State::Attempted,
            "kept" => State::Kept,
            "rolled-back" => {
                let why = field("why")?;
                let blank = why.trim().is_empty();
                if blank || why.len() > MAX_WHY || why.chars().any(char::is_control) {
                    return Err("why is not one short line".into());
                }
                State::RolledBack(why.into())
            }
            other => return Err(format!("no state {other}")),
        };
        if lines.next().is_some() {
            return Err("a trial record has an extra line".into());
        }
        Ok(Trial {
            candidate,
            fnv,
            previous,
            state,
        })
    }
}

/// `why` as one line of at most [`MAX_WHY`] bytes.
fn clean(why: &str) -> String {
    let mut out = String::new();
    for c in why.chars().map(|c| if c.is_control() { ' ' } else { c }) {
        if out.len() + c.len_utf8() > MAX_WHY {
            break;
        }
        out.push(c);
    }
    out
}

/// 64 lower-case hex digits as a SHA-256.
fn sha(text: &str) -> Option<Sha256> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in bytes.chunks(2).enumerate() {
        out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(out)
}

/// What `updated` answers a switch, an import or a download with while a boot
/// judges a model on trial.
pub const ON_TRIAL: &str = "a model is on trial; ask again once it is kept or rolled back";

/// How long `updated` waits for `llm`'s `health` when it must know what `llm`
/// runs: the probation's look, and the model a switch or a remove is held to.
/// Longer than `llm`'s own wait on an LLM server's health
/// (`llmproto::health::SERVER_HEALTH_MS`), after which it still reports the
/// local model, so a slow or unreachable server does not hide a candidate
/// that loaded; and short enough that the shell's 10 s call to `updated`
/// holds one such wait and the requests after it.
pub const HEALTH_WAIT_MS: u64 = 6_000;

/// What `model_use` of `to` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Switch {
    /// The next restart uses it already.
    Nothing,
    /// Write this record, then point the hint at its candidate.
    Try(Trial),
    /// Point the hint back at `to`, the pending trial's way back, and drop the
    /// trial: `undo` of a switch not yet tried.
    Cancel,
    /// A boot is judging the trial: [`ON_TRIAL`], and nothing changes. Its
    /// verdict is written over its record and moves the hint, so a switch
    /// now would be lost or would boot untried (bug hunt 1, #25).
    OnTrial,
}

/// `model_use` of the stored model `to`, whose FNV-1a64 is `fnv`, with
/// `record` the last trial, if any, and `running` the model this boot runs,
/// which counts only with no record. The way back a new trial records is the
/// model the next restart maps without it, which the record names: a pending
/// trial's way back (replaced, it keeps it; switched to, it is cancelled),
/// a kept candidate, or the model a rollback put the hint back on, even while
/// this boot still runs the candidate. Never while a boot judges a trial.
pub fn switch(running: Option<Sha256>, record: Option<&Trial>, to: Sha256, fnv: u64) -> Switch {
    let try_it = |previous: Option<Sha256>| {
        Switch::Try(Trial {
            candidate: to,
            fnv,
            previous,
            state: State::Pending,
        })
    };
    let Some(t) = record else {
        return match running == Some(to) {
            true => Switch::Nothing,
            false => try_it(running),
        };
    };
    match &t.state {
        State::Attempted => Switch::OnTrial,
        State::Pending | State::Kept if t.candidate == to => Switch::Nothing,
        State::Pending if t.previous == Some(to) => Switch::Cancel,
        State::Pending => try_it(t.previous),
        State::Kept => try_it(Some(t.candidate)),
        State::RolledBack(_) if t.previous == Some(to) => Switch::Nothing,
        State::RolledBack(_) => try_it(t.previous),
    }
}

/// The reason a trial a boot found already attempted is rolled back with.
pub const HUNG: &str = "it never finished loading";

/// What a boot does first with the last trial, before it waits on anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AtBoot {
    /// No trial is undecided.
    Nothing,
    /// Write this record, marked attempted, then wait on `llm` and [`decide`].
    Try(Trial),
    /// A boot tried it and never decided: point the hint back at `previous`
    /// and write this record, without trying it again.
    RollBack(Trial),
    /// `updated` started again within the boot and found it attempted: point
    /// the hint back at `previous` for now, as a rollback would, so the next
    /// boot cannot hang on it, and keep the record as it is; then wait on
    /// `llm` and [`decide`] again, and on [`Decision::Keep`] point the hint at
    /// the candidate again before writing it kept.
    Resume(Trial),
}

/// The first step of a start of `updated` with `record` the last trial, and
/// `again` whether it started before this boot ([`started_again`]). Its first
/// start marks a pending trial attempted and rolls one already attempted back
/// at once, since only a boot that hung leaves one (K's condition). A start
/// again finds this boot's own marks: an attempted trial is this boot's, which
/// it goes on judging rather than take it for a hang (bug hunt 1, #27), with
/// the hint on the way back meanwhile ([`AtBoot::Resume`]), so even a first
/// start that ended before it settled a hung boot's trial never leaves the
/// hint on that candidate; a pending one was written this boot for the next
/// restart, which this boot does not judge.
pub fn at_boot(record: Option<&Trial>, again: bool) -> AtBoot {
    match record {
        Some(t) if t.state == State::Pending && !again => AtBoot::Try(Trial {
            state: State::Attempted,
            ..t.clone()
        }),
        Some(t) if t.state == State::Attempted && again => AtBoot::Resume(t.clone()),
        Some(t) if t.state == State::Attempted => AtBoot::RollBack(Trial {
            state: State::RolledBack(HUNG.into()),
            ..t.clone()
        }),
        _ => AtBoot::Nothing,
    }
}

/// Whether `updated`'s arguments say it started before this boot: the
/// supervisor's `restart=N` counts its restarts of it since the boot began,
/// those after an exit and those asked for (`supervise::Record::restarts`).
/// Its records are the kernel's, in memory, so N starts at 0 in every boot.
pub fn started_again(args: &str) -> bool {
    args.split_whitespace()
        .filter_map(|word| word.strip_prefix("restart="))
        .any(|n| n.parse::<u64>().is_ok_and(|n| n > 0))
}

/// What `llm` says of its model, as `updated` reads its `health` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health<'a> {
    /// It loaded the model with this FNV-1a64.
    Ready {
        fnv: u64,
    },
    Failed(&'a str),
    /// It has no model. Final: `llm` maps its model, or learns there is none,
    /// before it serves at all, so while the store fills it gives no answer
    /// ([`Health::NotYet`]) rather than this.
    NoModel,
    /// No answer yet: it is still mapping or warming the model.
    NotYet,
}

/// What a boot does with a pending trial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Wait,
    Keep,
    /// Point the hint back at `previous`, and record why.
    RollBack(String),
}

/// The attempted trial's outcome, given the candidate's FNV-1a64, what `llm`
/// says, and how long this boot has waited of `limit_ms`. Only `Keep` moves
/// it to `kept`.
pub fn decide(fnv: u64, health: Health<'_>, waited_ms: u64, limit_ms: u64) -> Decision {
    match health {
        Health::Ready { fnv: loaded } if loaded == fnv => Decision::Keep,
        Health::Ready { .. } => Decision::RollBack("another model started in its place".into()),
        Health::Failed(why) => Decision::RollBack(format!("it did not load: {}", clean(why))),
        Health::NoModel => Decision::RollBack("no model started".into()),
        Health::NotYet if waited_ms >= limit_ms => {
            Decision::RollBack(format!("it did not load within {} s", limit_ms / 1000))
        }
        Health::NotYet => Decision::Wait,
    }
}

/// The record the verdict on the trial `judged` writes, `state` now: only
/// while `record`, as it stands, is still that trial attempted. Anything else
/// (replaced, cancelled, decided) was written since, and it and the hint are
/// left as they are.
pub fn verdict(record: Option<&Trial>, judged: &Trial, state: State) -> Option<Trial> {
    let ours = judged.state == State::Attempted && record == Some(judged);
    ours.then(|| Trial {
        state,
        ..judged.clone()
    })
}

/// `llm`'s `health` reply `text` as the probation reads it, with `state` its
/// state word: `llmproto::health::state_of`'s, as `State::word` writes it
/// (`none` for the error `llm` gives with no model and no server). The bin
/// reads it; this crate does not link `llmproto`. No state is no answer yet:
/// an error from the server alone, or words without the status.
pub fn health_of<'a>(text: &'a str, state: Option<&str>) -> Health<'a> {
    match state {
        Some("ready" | "warming") => match crate::listing::loaded(text) {
            Some((_, fnv)) => Health::Ready { fnv },
            None => Health::Failed("its health names no model"),
        },
        Some("failed") => Health::Failed(text),
        Some("none" | "remote") => Health::NoModel,
        // `waiting` and `busy`: still mapping, loading or answering.
        _ => Health::NotYet,
    }
}
