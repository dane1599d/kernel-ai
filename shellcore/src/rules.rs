//! Standing rules (ADR 0002 D2a): what "Always allow" on the Allow / Deny menu
//! wrote. A rule names an action, and for a row with a target, optionally that
//! target. It waives only the ask that comes from the row itself (no undo, or an
//! ungrounded host for a per-host rule), never one from consent: an added call,
//! content steering it, a never value or a line that is not English. Only the
//! terminal's menu answer writes one; `rules` lists them and `rules_remove <n>`
//! removes one. They live in memory until P2.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// Most rules kept: a rule is a grant node, and the user root's share is small.
pub const MAX_RULES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: u64,
    pub action: &'static str,
    /// `None` for every call of the action.
    pub target: Option<String>,
    /// The `Grant` handle to its node, which `rules_remove` revokes.
    pub grant: Option<u32>,
}

impl Rule {
    /// `#2 set_policy` or `#3 set_policy "round-robin"`.
    fn line(&self) -> String {
        match &self.target {
            Some(target) => format!("#{} {} \"{target}\"", self.id, self.action),
            None => format!("#{} {}", self.id, self.action),
        }
    }
}

/// What a menu may offer for one ask: the action's rule, the target's, or both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub action: &'static str,
    /// The call's target, when its row has one.
    pub target: Option<String>,
    /// Whether a rule for every call of the action may waive this ask; a per-host
    /// ask takes only a rule for its host.
    pub kind: bool,
}

impl Offer {
    /// The menu's labels for the two rules, empty where none is offered:
    /// "Always allow set_policy calls", "Always allow set_policy on \"x\"".
    pub fn labels(&self) -> (String, String) {
        let kind = match self.kind {
            true => format!("{} calls", self.action),
            false => String::new(),
        };
        let target = self
            .target
            .as_ref()
            .map_or(String::new(), |t| format!("{} on \"{t}\"", self.action));
        (kind, target)
    }
}

#[derive(Debug, Default)]
pub struct Rules {
    list: Vec<Rule>,
    next: u64,
}

impl Rules {
    /// Writes the rule an "Always allow" answer chose: for the whole action, or
    /// (`for_target`) for the offer's target only. The same rule twice is one rule.
    pub fn add(
        &mut self,
        offer: &Offer,
        for_target: bool,
        grant: Option<u32>,
    ) -> Result<u64, &'static str> {
        let target = match (for_target, &offer.target, offer.kind) {
            (true, Some(target), _) => Some(target.clone()),
            (false, _, true) => None,
            _ => return Err("that rule was not offered"),
        };
        if let Some(rule) = self
            .list
            .iter()
            .find(|r| r.action == offer.action && r.target == target)
        {
            return Ok(rule.id);
        }
        if self.list.len() >= MAX_RULES {
            return Err("there are too many rules; remove one with rules_remove");
        }
        self.next += 1;
        self.list.push(Rule {
            id: self.next,
            action: offer.action,
            target,
            grant,
        });
        Ok(self.next)
    }

    /// The rule that waives `offer`'s ask, if there is one: one for its target, or
    /// for the whole action when the offer allows that.
    pub fn covering(&self, offer: &Offer) -> Option<u64> {
        self.list
            .iter()
            .find(|rule| {
                rule.action == offer.action
                    && match &rule.target {
                        Some(target) => offer.target.as_ref() == Some(target),
                        None => offer.kind,
                    }
            })
            .map(|rule| rule.id)
    }

    /// `rules`: one line per rule.
    pub fn list(&self) -> String {
        if self.list.is_empty() {
            return String::from("no rules: asks are answered one at a time");
        }
        let mut text = String::new();
        for rule in &self.list {
            let _ = writeln!(text, "{}", rule.line());
        }
        text.pop();
        text
    }

    /// `rules_remove <n>`: the rule it removed.
    pub fn remove(&mut self, id: u64) -> Result<Rule, String> {
        let at = self
            .list
            .iter()
            .position(|rule| rule.id == id)
            .ok_or_else(|| format!("no rule #{id}"))?;
        Ok(self.list.remove(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(target: Option<&str>, kind: bool) -> Offer {
        Offer {
            action: "service_stop",
            target: target.map(String::from),
            kind,
        }
    }

    #[test]
    fn a_rule_covers_what_it_names_and_no_more() {
        let mut rules = Rules::default();
        let build = offer(Some("build-3"), true);
        assert_eq!(rules.covering(&build), None);
        assert_eq!(rules.add(&build, true, None), Ok(1));
        assert_eq!(rules.covering(&build), Some(1));
        assert_eq!(rules.covering(&offer(Some("llm"), true)), None);
        assert_eq!(rules.add(&build, false, None), Ok(2));
        assert_eq!(rules.covering(&offer(Some("llm"), true)), Some(2));
        // A per-host ask is not waived by a rule for the whole action.
        assert_eq!(rules.covering(&offer(Some("llm"), false)), None);
        assert_eq!(
            rules.add(&build, true, None),
            Ok(1),
            "the same rule twice is one"
        );
        assert_eq!(rules.list(), "#1 service_stop \"build-3\"\n#2 service_stop");
    }

    #[test]
    fn only_an_offered_rule_is_written_and_there_is_a_limit() {
        let mut rules = Rules::default();
        assert!(rules.add(&offer(None, false), false, None).is_err());
        assert!(rules.add(&offer(None, true), true, None).is_err());
        for n in 0..MAX_RULES {
            let name = format!("p{n}");
            rules.add(&offer(Some(&name), false), true, None).unwrap();
        }
        assert!(
            rules
                .add(&offer(Some("one-more"), false), true, None)
                .is_err()
        );
    }

    #[test]
    fn removing_a_rule_takes_its_cover_away() {
        let mut rules = Rules::default();
        let build = offer(Some("build-3"), true);
        rules.add(&build, true, None).unwrap();
        assert_eq!(rules.remove(1).map(|r| r.id), Ok(1));
        assert_eq!(rules.covering(&build), None);
        assert_eq!(rules.remove(1), Err(String::from("no rule #1")));
        assert_eq!(rules.list(), "no rules: asks are answered one at a time");
        assert_eq!(
            build.labels(),
            (
                String::from("service_stop calls"),
                String::from("service_stop on \"build-3\"")
            )
        );
    }
}
