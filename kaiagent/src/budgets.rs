//! The `model` line: an agent's budgets, which `llm` enforces by badge (ADR 0013
//! section 2). Each is a request the install card shows; a key left out takes
//! the default, and `-` takes all three (question 10).

use crate::Refusal;
use alloc::string::String;

/// Tokens a day, prefill plus decode (question 10).
pub const DEFAULT_TOKENS_PER_DAY: u32 = 100_000;
/// KV positions one request may hold (question 10).
pub const DEFAULT_KV_POSITIONS: u32 = 2_048;
/// The share of the snapshot pool, in MiB (question 10).
pub const DEFAULT_POOL_MIB: u32 = 128;
/// The whole snapshot pool (section 2, question 4): no share is more.
pub const MAX_POOL_MIB: u32 = 512;
/// The most KV positions: `llm`'s slot (`kabi::model::CONTEXT_CAP`), which no
/// request can hold more than.
pub const MAX_KV_POSITIONS: u32 = kabi::model::CONTEXT_CAP as u32;
/// The most tokens a day: about five days of the engine's decoding at the 415,000 a
/// day it manages (est.), so a request for more is not a budget but no limit.
pub const MAX_TOKENS_PER_DAY: u32 = 2_000_000;

/// Bytes a day an agent's network may move when its `net` line names a destination
/// and says no more, in MiB: 10 MiB (est.: a few hundred fetches of an API answer or a
/// page, 30 KB each; the budget is for an agent that reads, not one that downloads).
pub const DEFAULT_NET_MIB_PER_DAY: u32 = 10;
/// The most bytes a day a manifest may ask for, in MiB: 512 MiB (est.), well past what
/// an agent that reads pages and calls APIs needs, so a request for more is a download
/// and not a budget, and P5's verified downloads are for that.
pub const MAX_NET_MIB_PER_DAY: u32 = 512;

/// What `llm` holds an agent to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budgets {
    pub tokens_per_day: u32,
    pub kv_positions: u32,
    pub pool_mib: u32,
}

impl Budgets {
    /// A packaged agent's budgets when it says none.
    pub const DEFAULT: Budgets = Budgets {
        tokens_per_day: DEFAULT_TOKENS_PER_DAY,
        kv_positions: DEFAULT_KV_POSITIONS,
        pool_mib: DEFAULT_POOL_MIB,
    };

    /// The pool share in bytes.
    pub fn pool_bytes(&self) -> u64 {
        u64::from(self.pool_mib) << 20
    }

    /// `tokens/day=… kv=… pool=…m`, the line's canonical value.
    pub fn format(&self) -> String {
        alloc::format!(
            "tokens/day={} kv={} pool={}m",
            self.tokens_per_day,
            self.kv_positions,
            self.pool_mib
        )
    }

    /// The `model` line's value: `-`, or any of `tokens/day=N`, `kv=N` and `pool=Nm`
    /// once each, in that order; what it leaves out is the default.
    pub fn parse(value: &str) -> Result<Self, Refusal> {
        let mut budgets = Self::DEFAULT;
        if value == "-" {
            return Ok(budgets);
        }
        let mut next = 0;
        for word in value.split(' ') {
            let (key, number) = word
                .split_once('=')
                .ok_or_else(|| Refusal::BudgetKey(word.into()))?;
            let at = KEYS
                .iter()
                .position(|k| *k == key)
                .filter(|at| *at >= next)
                .ok_or_else(|| Refusal::BudgetKey(word.into()))?;
            next = at + 1;
            let key = KEYS[at];
            match key {
                "tokens/day" => {
                    budgets.tokens_per_day = whole(key, number, MAX_TOKENS_PER_DAY.into())?
                }
                "kv" => budgets.kv_positions = whole(key, number, MAX_KV_POSITIONS.into())?,
                _ => {
                    let mib = number.strip_suffix('m').ok_or(Refusal::BudgetUnit)?;
                    budgets.pool_mib = whole(key, mib, MAX_POOL_MIB.into())?;
                }
            }
        }
        Ok(budgets)
    }
}

/// What the agent's network may move in a day, both ways (ADR 0013 section 6): the
/// bytes a day that the network service holds a `net` line's destination to, as the
/// `llm` budgets are held by `llm`. `kaiagent` only says the number;
/// [`crate::DayBytes`] counts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetBudget {
    /// MiB a day; 0 only for `net none`, which may move nothing.
    pub mib_per_day: u32,
}

impl NetBudget {
    /// `net none`: no network, no bytes.
    pub const NONE: NetBudget = NetBudget { mib_per_day: 0 };
    /// A destination that says no more.
    pub const DEFAULT: NetBudget = NetBudget {
        mib_per_day: DEFAULT_NET_MIB_PER_DAY,
    };

    /// The bytes a day.
    pub fn bytes_per_day(&self) -> u64 {
        u64::from(self.mib_per_day) << 20
    }

    /// `bytes/day=…m`, the net line's budget word.
    pub fn format(&self) -> String {
        alloc::format!("{}{}m", NET_KEY, self.mib_per_day)
    }

    /// The word after the destination: `bytes/day=Nm`, N whole MiB above zero and at
    /// most [`MAX_NET_MIB_PER_DAY`].
    pub fn parse(word: &str) -> Result<Self, Refusal> {
        let number = word
            .strip_prefix(NET_KEY)
            .ok_or_else(|| Refusal::BudgetKey(word.into()))?;
        let mib = number.strip_suffix('m').ok_or(Refusal::BudgetUnit)?;
        Ok(Self {
            mib_per_day: whole("net bytes/day", mib, MAX_NET_MIB_PER_DAY.into())?,
        })
    }
}

/// The net line's budget key.
const NET_KEY: &str = "bytes/day=";

/// The keys, in the order they are written.
const KEYS: [&str; 3] = ["tokens/day", "kv", "pool"];

/// `digits` as a budget: a whole number with no sign or leading zero, not zero,
/// and at most `max`.
pub(crate) fn whole(key: &'static str, digits: &str, max: u64) -> Result<u32, Refusal> {
    let plain = !digits.is_empty()
        && digits.len() <= 10
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    if !plain {
        return Err(Refusal::BudgetNumber(key));
    }
    let n: u64 = digits.parse().map_err(|_| Refusal::BudgetNumber(key))?;
    match n {
        0 => Err(Refusal::BudgetZero(key)),
        n if n > max => Err(Refusal::BudgetOver { key, max }),
        n => Ok(n as u32),
    }
}
