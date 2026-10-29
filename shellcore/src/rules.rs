//! Standing rules (ADR 0002 D2a): what "Always allow" on the Allow / Deny menu
//! wrote. A rule names an action, and for a row with a target, optionally that
//! target. It waives only the ask that comes from the row itself (no undo, or an
//! ungrounded host for a per-host rule), never one from consent: an added call,
//! content steering it, a never value or a line that is not English. Only the
//! terminal's menu answer writes one; `rules` lists them and `rules_remove <n>`
//! removes one.
//!
//! Since P2 they are kept in `/system/state/rules` as `kai-grants 1` rule lines
//! (ADR 0001 D8), until `powerbox` owns `/system/grants`. The file is read as
//! hostile and refused whole on any bad line. A rule read back is a record with
//! no grant node: only the trusted path makes one, on the keystroke. So until
//! "Always allow" is chosen again, it waives no ask whose owner checks the rule
//! itself (the stops), and the shell's own asks only.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// Most rules kept: a rule is a grant node, and the user root's share is small.
pub const MAX_RULES: usize = 32;

/// Actions whose owner checks the rule's node itself (RULE_CHECK), so a rule
/// with no node cannot waive their ask.
pub const OWNER_CHECKED: [&str; 3] = ["process_stop", "service_stop", "service_restart"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: u64,
    pub action: &'static str,
    /// `None` for every call of the action.
    pub target: Option<String>,
    /// The `Grant` handle to its node, which `rules_remove` revokes.
    pub grant: Option<u32>,
    /// The log entry whose menu made it.
    pub prompt: u64,
    /// When, in nanoseconds since 1970.
    pub made_ns: u64,
    /// Read back from disk, with no node yet.
    pub restored: bool,
}

impl Rule {
    /// `#2 set_policy` or `#3 set_policy "round-robin"`.
    fn line(&self) -> String {
        let mut line = match &self.target {
            Some(target) => format!("#{} {} \"{target}\"", self.id, self.action),
            None => format!("#{} {}", self.id, self.action),
        };
        if !self.waives_owner_checks() {
            line.push_str(" (asks again until allowed on this boot)");
        }
        line
    }

    /// Whether the rule can waive its action's ask: a rule read back from disk
    /// has no node for an owner to check.
    fn waives_owner_checks(&self) -> bool {
        !(self.restored && OWNER_CHECKED.contains(&self.action))
    }

    fn record(&self) -> Result<kaigrants::Rule, String> {
        Ok(kaigrants::Rule {
            id: u32::try_from(self.id).map_err(|_| format!("rule #{} is over u32", self.id))?,
            action: self.action.into(),
            target: self.target.clone(),
            prompt: self.prompt,
            made: kaigrants::Time::from_ns(self.made_ns),
        })
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
    /// (`for_target`) for the offer's target only, made by menu `prompt` at
    /// `made_ns`. The same rule twice is one rule; chosen again, a rule read back
    /// from disk takes the new node.
    pub fn add(
        &mut self,
        offer: &Offer,
        for_target: bool,
        grant: Option<u32>,
        (prompt, made_ns): (u64, u64),
    ) -> Result<u64, &'static str> {
        let target = match (for_target, &offer.target, offer.kind) {
            (true, Some(target), _) => Some(target.clone()),
            (false, _, true) => None,
            _ => return Err("that rule was not offered"),
        };
        if let Some(rule) = self
            .list
            .iter_mut()
            .find(|r| r.action == offer.action && r.target == target)
        {
            if rule.restored {
                (rule.grant, rule.restored) = (grant, false);
            }
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
            prompt,
            made_ns,
            restored: false,
        });
        Ok(self.next)
    }

    /// The rules as `/system/state/rules` keeps them.
    pub fn text(&self) -> Result<String, String> {
        let rules = self
            .list
            .iter()
            .map(Rule::record)
            .collect::<Result<_, _>>()?;
        kaigrants::Grants {
            grants: Vec::new(),
            rules,
        }
        .text()
    }

    /// The rules a `/system/state/rules` text holds, refused whole if any line is
    /// out of shape, names an action the registry does not have, or is a grant:
    /// grants are never reloaded from the shell's own files.
    pub fn restore(text: &str) -> Result<Self, String> {
        let read = kaigrants::parse(text).map_err(|e| format!("line {}: {}", e.line, e.why))?;
        if !read.grants.is_empty() {
            return Err(String::from(
                "it holds a grant, which is not the shell's to keep",
            ));
        }
        if read.rules.len() > MAX_RULES {
            return Err(String::from("it holds too many rules"));
        }
        let mut rules = Self::default();
        for rule in read.rules {
            let action = agentabi::ROWS
                .iter()
                .find(|row| row.name == rule.action)
                .map(|row| row.name)
                .ok_or_else(|| format!("rule {} names no action: {}", rule.id, rule.action))?;
            rules.next = rules.next.max(u64::from(rule.id));
            rules.list.push(Rule {
                id: u64::from(rule.id),
                action,
                target: rule.target,
                grant: None,
                prompt: rule.prompt,
                made_ns: rule.made.0.saturating_mul(1_000_000_000),
                restored: true,
            });
        }
        Ok(rules)
    }

    /// The rule that waives `offer`'s ask, if there is one: one for its target, or
    /// for the whole action when the offer allows that.
    pub fn covering(&self, offer: &Offer) -> Option<u64> {
        self.list
            .iter()
            .filter(|rule| rule.waives_owner_checks())
            .find(|rule| {
                rule.action == offer.action
                    && match &rule.target {
                        Some(target) => offer.target.as_ref() == Some(target),
                        None => offer.kind,
                    }
            })
            .map(|rule| rule.id)
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
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
        assert_eq!(rules.add(&build, true, None, (0, 0)), Ok(1));
        assert_eq!(rules.covering(&build), Some(1));
        assert_eq!(rules.covering(&offer(Some("llm"), true)), None);
        assert_eq!(rules.add(&build, false, None, (0, 0)), Ok(2));
        assert_eq!(rules.covering(&offer(Some("llm"), true)), Some(2));
        // A per-host ask is not waived by a rule for the whole action.
        assert_eq!(rules.covering(&offer(Some("llm"), false)), None);
        assert_eq!(
            rules.add(&build, true, None, (0, 0)),
            Ok(1),
            "the same rule twice is one"
        );
        assert_eq!(rules.list(), "#1 service_stop \"build-3\"\n#2 service_stop");
    }

    #[test]
    fn only_an_offered_rule_is_written_and_there_is_a_limit() {
        let mut rules = Rules::default();
        assert!(rules.add(&offer(None, false), false, None, (0, 0)).is_err());
        assert!(rules.add(&offer(None, true), true, None, (0, 0)).is_err());
        for n in 0..MAX_RULES {
            let name = format!("p{n}");
            rules
                .add(&offer(Some(&name), false), true, None, (0, 0))
                .unwrap();
        }
        assert!(
            rules
                .add(&offer(Some("one-more"), false), true, None, (0, 0))
                .is_err()
        );
    }

    #[test]
    fn removing_a_rule_takes_its_cover_away() {
        let mut rules = Rules::default();
        let build = offer(Some("build-3"), true);
        rules.add(&build, true, None, (0, 0)).unwrap();
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

    #[test]
    fn rules_read_back_from_disk_as_records_with_no_node() {
        let mut rules = Rules::default();
        let build = offer(Some("build-3"), true);
        rules
            .add(&build, true, Some(7), (212, 1_792_618_800_000_000_000))
            .unwrap();
        let policy = Offer {
            action: "set_policy",
            target: None,
            kind: true,
        };
        rules.add(&policy, false, Some(8), (213, 0)).unwrap();
        let text = rules.text().unwrap();
        assert_eq!(
            text,
            "kai-grants 1
rule 1 action=service_stop target=build-3 from=prompt:212 made=2026-10-21T21:40:00Z
rule 2 action=set_policy target=- from=prompt:213 made=1970-01-01T00:00:00Z
"
        );
        let mut back = Rules::restore(&text).unwrap();
        assert!(back.list.iter().all(|r| r.restored && r.grant.is_none()));
        // A stop's owner checks the node, which a rule read back lacks.
        assert_eq!(back.covering(&build), None);
        assert_eq!(back.covering(&policy), Some(2));
        assert!(
            back.list()
                .contains("#1 service_stop \"build-3\" (asks again")
        );
        // Chosen again, it takes the new node, under the same number.
        assert_eq!(back.add(&build, true, Some(9), (300, 0)), Ok(1));
        assert_eq!(back.covering(&build), Some(1));
        assert_eq!(back.add(&policy, false, None, (301, 0)), Ok(2));
    }

    #[test]
    fn a_rules_file_with_a_grant_or_an_unknown_action_is_refused_whole() {
        let rule = "rule 1 action=set_policy target=- from=prompt:3 made=2026-11-02T10:20:00Z
";
        assert!(
            Rules::restore(&format!(
                "kai-grants 1
{rule}"
            ))
            .is_ok()
        );
        let grant = "grant 2 app=notes kind=dir object=/home/user rights=read from=picker life=persistent signer=- made=2026-11-02T10:14:00Z
";
        for bad in [
            format!(
                "kai-grants 1
{rule}{grant}"
            ),
            format!(
                "kai-grants 1
{}",
                rule.replace("set_policy", "format_disk")
            ),
            format!(
                "kai-grants 1
{}",
                rule.replace("target=-", "target=a target=b")
            ),
        ] {
            assert!(Rules::restore(&bad).is_err(), "{bad}");
        }
    }
}
