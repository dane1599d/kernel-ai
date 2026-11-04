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
/// The most KV positions: Qwen3's native context window (est.), which no request
/// can fill past.
pub const MAX_KV_POSITIONS: u32 = 32_768;

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
                "tokens/day" => budgets.tokens_per_day = whole(key, number, u32::MAX.into())?,
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

/// The keys, in the order they are written.
const KEYS: [&str; 3] = ["tokens/day", "kv", "pool"];

/// `digits` as a budget: a whole number with no sign or leading zero, not zero,
/// and at most `max`.
fn whole(key: &'static str, digits: &str, max: u64) -> Result<u32, Refusal> {
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
