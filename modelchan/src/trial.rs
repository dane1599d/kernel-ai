//! A model on trial (ADR 0008 §6 and §11): `model_use` names a stored model
//! for the next restart, since the kernel maps its model once per boot.
//! `updated` writes `kai-modeltrial 1` beside `/system/updates`' other records,
//! then has `fs` point `kai-modelhint 1` at the candidate. At the next boot the
//! candidate is kept once `llm` reports it loaded, and otherwise the hint goes
//! back to the model before, with the reason recorded for the model manager.
//!
//! The record is text, one field a line:
//!
//! ```text
//! kai-modeltrial 1
//! candidate <64 hex>
//! previous <64 hex> | none
//! state pending | kept | rolled-back
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
    Kept,
    /// The candidate did not load, and why; the hint is back on `previous`.
    RolledBack(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    pub candidate: Sha256,
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
            State::Kept => "kept".to_string(),
            State::RolledBack(why) => match clean(why) {
                why if why.trim().is_empty() => "rolled-back\nwhy no reason was given".into(),
                why => format!("rolled-back\nwhy {why}"),
            },
        };
        format!(
            "{MAGIC}\ncandidate {}\nprevious {previous}\nstate {state}\n",
            hex(&self.candidate)
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
        let previous = match field("previous")? {
            "none" => None,
            text => Some(sha(text).ok_or("previous is not a SHA-256")?),
        };
        let state = match field("state")? {
            "pending" => State::Pending,
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

/// What `model_use` of `to` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Switch {
    /// The next restart uses it already.
    Nothing,
    /// Write this record, then point the hint at its candidate.
    Try(Trial),
    /// Point the hint back at the running model and drop the pending trial:
    /// `undo` of a switch not yet tried.
    Cancel,
}

/// `model_use` of the stored model `to`, with `running` the model this boot
/// runs and `record` the last trial, if any.
pub fn switch(running: Option<Sha256>, record: Option<&Trial>, to: Sha256) -> Switch {
    let pending = record.filter(|t| t.state == State::Pending);
    match pending {
        Some(t) if t.candidate == to => Switch::Nothing,
        Some(_) if running == Some(to) => Switch::Cancel,
        _ if running == Some(to) => Switch::Nothing,
        _ => Switch::Try(Trial {
            candidate: to,
            previous: running,
            state: State::Pending,
        }),
    }
}

/// What `llm` says of its model, as `updated` reads its `health` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health<'a> {
    /// It loaded the model with this FNV-1a64.
    Ready {
        fnv: u64,
    },
    Failed(&'a str),
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

/// The pending trial's outcome, given the candidate's FNV-1a64, what `llm`
/// says, and how long this boot has waited of `limit_ms`.
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
