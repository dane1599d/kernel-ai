//! One transaction per call (ADR 0003 section 4). A call reads the app's own
//! storage as it was when the call began, with the clock and seed fixed; its
//! writes are buffered, and its effects (anything outside the app) are queued.
//! The host commits the writes with the version's checkpoint and puts the effects
//! in the reply, or drops both when the call fails. During probation the writes
//! that commit are the standby's (the checkpoint's version), and the effects sent
//! are those of the version whose reply is sent.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use appabi::{Ty, Val, wave};

/// The app's own scope: keys to values.
pub type Storage = BTreeMap<String, Vec<u8>>;

/// Longest storage key, in bytes.
pub const MAX_KEY: usize = 128;
/// Largest storage value.
pub const MAX_VALUE: usize = 16 * 1024;
/// Most effects one call may queue.
pub const MAX_EFFECTS: usize = 8;
/// Longest registry name an effect may call (ADR 0002's).
pub const MAX_EFFECT_NAME: usize = 24;
/// Longest WAVE argument text of an effect call.
pub const MAX_EFFECT_ARGS: usize = 512;
/// Longest notify text.
pub const MAX_NOTIFY: usize = 256;

/// Something outside the app, run by the agent core after the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// A registry action and its arguments as a WAVE record.
    Call {
        name: String,
        args: String,
    },
    Notify(String),
}

impl Effect {
    /// Checks the effect against the limits; why not, if it is over them.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Self::Call { name, args } => {
                if name.is_empty() || name.len() > MAX_EFFECT_NAME {
                    return Err(format!("an effect names a {}-byte action", name.len()));
                }
                if args.len() > MAX_EFFECT_ARGS {
                    return Err(format!(
                        "an effect's arguments are over {MAX_EFFECT_ARGS} bytes"
                    ));
                }
            }
            Self::Notify(text) if text.len() > MAX_NOTIFY => {
                return Err(format!("a notify is over {MAX_NOTIFY} bytes"));
            }
            Self::Notify(_) => {}
        }
        Ok(())
    }

    /// The effect as WAVE text: `call({name: "ui_open", args: "{}"})` or
    /// `notify("saved")`.
    pub fn wave(&self) -> String {
        let quote =
            |text: &str| wave::print(&Val::String(text.into()), &Ty::String).unwrap_or_default();
        match self {
            Self::Call { name, args } => {
                format!("call({{name: {}, args: {}}})", quote(name), quote(args))
            }
            Self::Notify(text) => format!("notify({})", quote(text)),
        }
    }
}

/// What a call did besides its reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Txn {
    /// Each key's last write in the call: a value, or `None` for a delete.
    pub writes: BTreeMap<String, Option<Vec<u8>>>,
    pub effects: Vec<Effect>,
}

impl Txn {
    /// `storage` with the writes applied.
    pub fn apply(&self, storage: &mut Storage) {
        for (key, value) in &self.writes {
            match value {
                Some(value) => {
                    storage.insert(key.clone(), value.clone());
                }
                None => {
                    storage.remove(key);
                }
            }
        }
    }

    /// The reply lines for the effects, numbered from `first_ticket`.
    pub fn effect_lines(&self, first_ticket: u32) -> String {
        let mut lines = String::new();
        for (ticket, effect) in (first_ticket..).zip(&self.effects) {
            lines.push_str(&format!("effect {ticket} {}\n", effect.wave()));
        }
        lines
    }
}

/// What a call sees besides its arguments, the same for both versions on
/// probation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub now_ms: u64,
    pub seed: u64,
    /// The ticket the call's first effect gets.
    pub first_ticket: u32,
}

/// A storage key the limits allow.
pub fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > MAX_KEY {
        return Err(format!("a storage key is {} bytes", key.len()));
    }
    Ok(())
}

/// A seed for call number `call` at `now_ms` (splitmix64), so two versions given
/// the same call see the same seed and successive calls see different ones.
pub fn seed(now_ms: u64, call: u64) -> u64 {
    let mut z = now_ms ^ call.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn writes_apply_in_order_and_effects_read_as_wave() {
        let mut storage = Storage::new();
        storage.insert("a".into(), vec![1]);
        storage.insert("b".into(), vec![2]);
        let mut txn = Txn::default();
        txn.writes.insert("a".into(), None);
        txn.writes.insert("c".into(), Some(vec![3]));
        txn.effects.push(Effect::Call {
            name: "ui_open".into(),
            args: "{app: \"notes\"}".into(),
        });
        txn.effects.push(Effect::Notify("saved \"x\"".into()));
        txn.apply(&mut storage);
        assert_eq!(storage.keys().collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(
            txn.effect_lines(7),
            "effect 7 call({name: \"ui_open\", args: \"{app: \\\"notes\\\"}\"})\n\
             effect 8 notify(\"saved \\\"x\\\"\")\n"
        );
    }

    #[test]
    fn limits_hold() {
        assert!(check_key("").is_err());
        assert!(check_key(&"k".repeat(MAX_KEY + 1)).is_err());
        assert!(check_key("notes/1").is_ok());
        let long = Effect::Notify("x".repeat(MAX_NOTIFY + 1));
        assert!(long.check().is_err());
        let unnamed = Effect::Call {
            name: String::new(),
            args: "{}".into(),
        };
        assert!(unnamed.check().is_err());
        assert_ne!(seed(5, 1), seed(5, 2));
        assert_eq!(seed(5, 1), seed(5, 1));
    }
}
