//! Consent from the user's line (ADR 0002 D2, D3 and D6), as code with no model:
//! for each call in a plan, whether it runs at once, runs as "Taken as", asks, or is
//! refused. Spike S01-2's tier corpus (`tests/tier_corpus/`) measures it; task 7
//! builds `agentcore::execute` on it.
//!
//! The rule: what the line plainly asks for, and can be undone, runs. A change is
//! asked for when one of its row's cues starts a word of the line, not within three
//! words after a negation, and every `from_request` argument is literal in the line,
//! a declared alias, or `focused`/`selected`/`here` for "this", "it", "here" or
//! "selected". A literal counts only if it names a thing: "the service", "that
//! program" or "it" is a kind word, not a name, so `service_stop("service")` from
//! "stop the service" was not asked for. Anything else is added and asks, except the vague path: a plan of one
//! undoable local change, from a line that names no action at all, runs and says
//! what it was taken as. Values read from content (`Label::Content`) cannot steer
//! an undo-less or outbound call, and a line that is not English gets no consent
//! for changes.

use crate::{Action, ArgType, Inverse, Role, Tier, derive, find};
use alloc::vec::Vec;

#[path = "consent_grant.rs"]
pub mod consent_grant;
#[path = "consent_send.rs"]
mod consent_send;
// The reading of a line, by topic.
#[path = "consent_cues.rs"]
mod consent_cues;
#[path = "consent_english.rs"]
mod consent_english;
#[path = "consent_hosts.rs"]
mod consent_hosts;
#[path = "consent_said.rs"]
mod consent_said;
#[path = "consent_wants.rs"]
mod consent_wants;
#[path = "consent_words.rs"]
mod consent_words;
pub use consent_cues::named_in;
use consent_cues::*;
pub use consent_english::is_english_line;
use consent_english::*;
use consent_hosts::*;
use consent_said::*;
pub use consent_wants::asks_to_be_told;
use consent_wants::*;
use consent_words::*;
use nethosts::{Bytes, Ground, Grounding, LineWord, NoGrants, Origin, Table, Verdict};

/// Where a value came from (ADR 0002 D6). Ordered: the least trusted is greatest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Written in the plan before anything was read.
    Literal,
    /// The OS's own state.
    System,
    /// Content read from a source: a note, a page, a file.
    Content(&'static str),
}

/// The text of a value the planner does not know: it matches no word, alias,
/// host or path.
pub const UNKNOWN: &str = "\u{0}";

/// One argument value and where it came from.
#[derive(Clone, Copy, Debug)]
pub struct Value<'a> {
    pub text: &'a str,
    pub label: Label,
}

/// One call the plan makes: the action, its arguments by name, and what decided
/// that it runs at all (`Content` when it sits under an `if` on read data).
#[derive(Clone, Copy, Debug)]
pub struct Call<'a> {
    pub name: &'a str,
    pub args: &'a [(&'a str, Value<'a>)],
    pub control: Label,
}

/// Why a call got its tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A read on this machine.
    Read,
    /// A fetch from a host the line names or the host table grounds.
    Grounded,
    /// The line asked for it, and it can be undone.
    Asked,
    /// The line asked for it, and it cannot be undone: it asks once.
    NoUndo,
    /// The one change a vague line was read as.
    TakenAs,
    /// The model added it: no cue, a negated cue, or an argument not in the line.
    Added,
    /// A fetch from a host nobody named.
    Ungrounded,
    /// Read content picks its target or payload, or decides that it runs, and it has
    /// no undo or sends something out.
    Steered(&'static str),
    /// Content would go out of the machine.
    Outbound(&'static str),
    /// A read from a host only an app's grant grounds, which the line asked
    /// for and gave its path (`consent_grant`).
    GrantRead,
    /// A read from a host only an app's grant grounds, which read content or
    /// the OS's state decided ([`consent_grant::SYSTEM`]), not the line.
    GrantSteered(&'static str),
    /// A read from a host only an app's grant grounds, with an argument the
    /// line did not give: a path or query not one run of its words.
    GrantPath,
    /// A read from a host only an app's grant grounds, on a line that says to
    /// stay off the network.
    GrantOffline,
    /// More than one read from hosts only a grant grounds in one plan, or one
    /// in a loop.
    GrantMany,
    /// A send to a site the line does not name: refused, never asked.
    SendHost,
    /// A send on a line that does not ask for one (a question, a describe
    /// line, a negated cue): refused, never asked.
    SendUnasked,
    /// The line is not English, so it gives no consent for changes in v0.1.
    NotEnglish,
    /// A value no call may take, or a row that never runs.
    NeverValue,
    /// No row has this name.
    Unknown,
}

/// What happens to one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub tier: Tier,
    pub why: Why,
}

impl Decision {
    const fn new(tier: Tier, why: Why) -> Self {
        Self { tier, why }
    }
}

/// Request words that name a site, and the host each one means: the curated
/// host table (`nethosts::CURATED`, P5 task 11). An installed app's own word
/// joins it through [`decide_net`]'s [`Grounding`].
pub const HOSTS: &[(&str, &str)] = nethosts::CURATED;

/// Words that turn off every cue after them in their clause.
const NEGATIONS: &[&str] = &[
    "dont", "don't", "not", "never", "no", "without", "disable", "avoid", "neither", "nor",
];
/// Words that turn a cue off wherever they stand in its clause: "turn round
/// robin off" asks for no switch. A row whose own cues say "off" is not
/// turned off by it.
const OFF_WORDS: &[&str] = &["off", "disabled"];
/// A clause that starts with one of these and then `you` (or `u`) is a polite
/// request, not a question: "can you switch to round robin".
const POLITE: &[&str] = &["can", "could", "would", "will"];
/// Words that start a new clause, as `,`, `;`, `.`, `!`, `?` and `:` do: a
/// negation reaches the end of its clause, never past it ("don't wait, switch").
const CLAUSE_WORDS: &[&str] = &["but", "then", "however", "instead", "though"];
/// Words that point at what the OS has focused or selected.
const POINTERS: &[&str] = &["this", "it", "here", "selected"];
const SYMBOLS: &[&str] = &["focused", "selected", "here"];
/// Words for a kind of thing, or for one in passing, that name none: "stop the
/// service" names no service. Plurals are listed, not derived, so the app `notes`
/// stays a name.
const KIND_WORDS: &[&str] = &[
    "service",
    "services",
    "program",
    "programs",
    "app",
    "apps",
    "application",
    "process",
    "processes",
    "package",
    "packages",
    "device",
    "component",
    "file",
    "files",
    "folder",
    "server",
    "host",
    "site",
    "website",
    "page",
    "model",
    "rule",
    "note",
    "thing",
    "things",
    "it",
    "that",
    "this",
    "one",
    "them",
    "those",
    "these",
];
/// Words that only lead into a name.
const DETERMINERS: &[&str] = &[
    "the", "a", "an", "my", "your", "our", "its", "some", "any", "that", "this",
];

/// Decides every call of `plan`, made for the user's `line`, against `rows`,
/// with the curated host table and no grants.
pub fn decide(line: &str, plan: &[Call], rows: &[Action]) -> Vec<Decision> {
    let table = Table::curated();
    let net = Grounding {
        table: &table,
        granted: &NoGrants,
        sources: &[],
    };
    decide_net(line, plan, rows, &net)
}

/// [`decide`] with the shell's own host grounding (P5 task 11): its table with
/// the installed apps' words, the hosts the user allowed before, and the OS's
/// checked sources. An open-world read runs only when its host is grounded.
pub fn decide_net(line: &str, plan: &[Call], rows: &[Action], net: &Grounding) -> Vec<Decision> {
    let words = words(line);
    let hosts = Hosts { line, net };
    let english = is_english(line);
    // Every cue counts here, a telling clause's too: a line that names an
    // action is never vague, so "tell me how to remove ideas, it's so slow"
    // takes no vague reading (#883 review).
    let named_any = rows.iter().any(|row| read_every_cue(&words, row).is_some());
    let decisions: Vec<Decision> = consent_grant::at_most_one(
        plan.iter()
            .map(|call| decide_one(&words, &hosts, english, call, rows))
            .collect(),
    );
    let changes = plan
        .iter()
        .filter(|call| find(rows, call.name).is_some_and(|row| !row.facts.read_only))
        .count();
    // The vague path: one call in all, an undoable local change with nothing read in
    // it, from an English line that names no action but states a want or a
    // complaint, and asks no question. Default deny: any other line, such as
    // "show me the cpu count" or "how many cpus are there", asked to be told, not
    // for a change, so a change answering it is added and asks.
    if let (
        [call],
        [
            Decision {
                why: Why::Added, ..
            },
        ],
    ) = (plan, decisions.as_slice())
        && changes == 1
        && english
        && !named_any
        && wants(&words)
        && !is_question(line)
        && let Some(row) = find(rows, call.name)
        && derive(row) == Tier::Run
        && !row.facts.open_world
        && !steered_at_all(call)
    {
        return alloc::vec![Decision::new(Tier::Run, Why::TakenAs)];
    }
    decisions
}

/// The line and the grounding rule an open-world read is checked with.
struct Hosts<'a> {
    line: &'a str,
    net: &'a Grounding<'a>,
}

fn decide_one(
    words: &[Word],
    hosts: &Hosts,
    english: bool,
    call: &Call,
    rows: &[Action],
) -> Decision {
    let Some(row) = find(rows, call.name) else {
        return Decision::new(Tier::Never, Why::Unknown);
    };
    if row.min_tier == Tier::Never || has_never_value(row, call) {
        return Decision::new(Tier::Never, Why::NeverValue);
    }
    // A send goes only to a site the line names (typed, or a word of the host
    // table): never one only an app's grant grounds, nor one read content
    // gave. No answer on a card can widen that (P5 close, ADR 0002 D3).
    let sends_to_host = matches!(row.inverse, Inverse::Hold { .. })
        && row.args.iter().any(|arg| matches!(arg.ty, ArgType::Host));
    if sends_to_host && !send_destination(words, hosts, row, call) {
        return Decision::new(Tier::Never, Why::SendHost);
    }
    // A send only for a line that asks for one: a cue of the row, un-negated,
    // in a clause that asks for no telling. A question or a describe line
    // never reaches a card, whatever a view said (ADR 0002 D3, #723).
    if matches!(row.inverse, Inverse::Hold { .. }) && !consent_send::asked(words) {
        return Decision::new(Tier::Never, Why::SendUnasked);
    }
    if let Some(source) = steered(row, call) {
        return Decision::new(Tier::Ask, Why::Steered(source));
    }
    if let Some(source) = outbound(row, call)
        && !line_names_destination(words, hosts, row, call)
    {
        return Decision::new(Tier::Ask, Why::Outbound(source));
    }
    if row.facts.read_only {
        if !row.facts.open_world {
            return Decision::new(derive(row).max(Tier::Run), Why::Read);
        }
        if !hosts_grounded(words, hosts, row, call) {
            return Decision::new(Tier::Ask, Why::Ungrounded);
        }
        if granted_only(words, hosts, row, call) {
            return consent_grant::read(words, hosts.net, row, call);
        }
        return Decision::new(row.min_tier.max(Tier::Run), Why::Grounded);
    }
    // Data decided that this change runs, or gave it a value: the line did not
    // ask for it, whatever its cues (ADR 0002 D6). It asks, and the ask names
    // where the data came from.
    if let Some(source) = content_source(call) {
        return Decision::new(derive(row).max(Tier::Ask), Why::Steered(source));
    }
    if !english {
        return Decision::new(Tier::Ask, Why::NotEnglish);
    }
    let asked =
        matches!(cued(words, row, Some(call)), Some(false)) && arguments_in_line(words, row, call);
    match (asked, derive(row)) {
        (true, Tier::Run) => Decision::new(Tier::Run, Why::Asked),
        (true, tier) => Decision::new(tier, Why::NoUndo),
        (false, tier) => Decision::new(tier.max(Tier::Ask), Why::Added),
    }
}

fn has_never_value(row: &Action, call: &Call) -> bool {
    row.args.iter().any(|arg| {
        value_of(call, arg.name)
            .is_some_and(|value| arg.never.iter().any(|never| fold_eq(never, value.text)))
    })
}

/// P-T: content in the target or payload of a call with no undo or a held send, or
/// deciding that such a call runs.
fn steered(row: &Action, call: &Call) -> Option<&'static str> {
    if !matches!(row.inverse, Inverse::None(_) | Inverse::Hold { .. }) {
        return None;
    }
    if let Label::Content(source) = call.control {
        return Some(source);
    }
    row.args
        .iter()
        .filter(|arg| matches!(arg.role, Role::Target | Role::Payload))
        .find_map(|arg| match value_of(call, arg.name)?.label {
            Label::Content(source) => Some(source),
            _ => None,
        })
}

/// P-F: content as the payload, or the target, of a call that leaves the machine.
fn outbound(row: &Action, call: &Call) -> Option<&'static str> {
    if !row.facts.open_world {
        return None;
    }
    call.args.iter().find_map(|(_, value)| match value.label {
        Label::Content(source) => Some(source),
        _ => None,
    })
}

/// The source of the content that decides `call` runs, or of any of its
/// values.
fn content_source(call: &Call) -> Option<&'static str> {
    let control = match call.control {
        Label::Content(source) => Some(source),
        _ => None,
    };
    control.or_else(|| {
        call.args.iter().find_map(|(_, value)| match value.label {
            Label::Content(source) => Some(source),
            _ => None,
        })
    })
}

fn steered_at_all(call: &Call) -> bool {
    matches!(call.control, Label::Content(_))
        || call
            .args
            .iter()
            .any(|(_, value)| matches!(value.label, Label::Content(_)))
}

fn value_of<'a>(call: &Call<'a>, name: &str) -> Option<Value<'a>> {
    call.args
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn do_it_again_is_an_order_and_do_you_a_question() {
        for line in ["do it again", "Do that again please"] {
            assert!(!is_question(line), "{line}");
            assert!(wants(&words(line)), "{line}");
        }
        for line in ["do you use round robin", "do it again?", "does it run"] {
            assert!(is_question(line), "{line}");
        }
    }

    #[test]
    fn telling_lines_are_told_from_orders() {
        for line in [
            "describe this",
            "Explain the policy",
            "what is selected?",
            "how many cpus",
            "tell me about build-3",
            "list the services",
        ] {
            assert!(asks_to_be_told(line), "{line}");
        }
        for line in [
            "switch to round robin",
            "stop build-3",
            "it's sluggish",
            "do it again",
            "make it faster",
            // Orders led by a word that can also start a question (C on #723).
            "do the same for the editor",
            "can you stop build-3",
            "will you restart netd",
            "have the editor closed",
        ] {
            assert!(!asks_to_be_told(line), "{line}");
        }
    }

    #[test]
    fn a_telling_word_before_a_cue_makes_the_clause_a_question() {
        let change = crate::SET_POLICY;
        // Asks to be told: no change is asked for.
        for line in [
            "tell me how to switch to round robin",
            "can you explain how to switch the policy",
            "describe how to switch the scheduler, please",
        ] {
            assert_eq!(cued(&words(line), &change, None), None, "{line}");
            // It still names the action, so the line is never read as vague
            // (#883): `named_any` counts a telling clause's cue.
            assert_eq!(read_every_cue(&words(line), &change), Some(false), "{line}");
        }
        // The same words in another clause, or after the cue, or with no telling
        // word, are still an order.
        for line in [
            "switch the policy and tell me when it is done",
            "tell me what it is, then switch the policy",
            "switch to round robin",
        ] {
            assert_eq!(cued(&words(line), &change, None), Some(false), "{line}");
        }
    }

    #[test]
    fn a_question_asks_for_no_change_but_still_names_the_action() {
        let change = crate::SET_POLICY;
        for line in [
            "is round robin on",
            "it's so slow, how do I switch to round robin",
            "tell me, how do I switch to round robin, it's laggy",
            "explain: how do I switch to round robin? it's slow",
        ] {
            assert_eq!(cued(&words(line), &change, None), None, "{line}");
            assert_eq!(read_every_cue(&words(line), &change), Some(false), "{line}");
        }
    }

    #[test]
    fn the_word_lists_are_sorted_for_binary_search() {
        assert!(ENGLISH.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn cues_start_words_and_negation_reaches_its_clause() {
        let set = crate::SET_POLICY;
        assert_eq!(
            cued(&words("Switch the scheduler to round robin."), &set, None),
            Some(false)
        );
        assert_eq!(cued(&words("policy please"), &set, None), Some(false));
        assert_eq!(
            cued(&words("don't touch the policy"), &set, None),
            Some(true)
        );
        assert_eq!(
            cued(&words("no, not the scheduler"), &set, None),
            Some(true)
        );
        assert_eq!(
            cued(&words("not now; later, switch the policy"), &set, None),
            Some(false)
        );
        assert_eq!(cued(&words("apolicy"), &set, None), None);
        assert_eq!(cued(&words("how many cpus"), &set, None), None);
    }

    #[test]
    fn phrases_match_whole_words_across_spaces_and_hyphens() {
        let line = words("Use Round Robin, then open en.wikipedia.org");
        assert_eq!(matches(&line, "round-robin", false), [1]);
        assert_eq!(matches(&line, "en.wikipedia.org", false), [5]);
        assert!(matches(&line, "robin-round", false).is_empty());
        assert!(matches(&line, "roun", false).is_empty());
        assert_eq!(matches(&line, "roun", true), [1]);
    }

    #[test]
    fn lines_not_in_english_are_told_apart() {
        for line in [
            "Switch the scheduler to round robin.",
            "make it snappier",
            "install notes",
            "Is the second service called llm? Answer yes or no.",
        ] {
            assert!(is_english_line(line), "{line}");
        }
        for line in [
            "cambia la política a round robin",
            "passe le planificateur en round robin",
            "stell den Planer auf round robin um",
            "ラウンドロビンに切り替えて",
            "переключи планировщик",
        ] {
            assert!(!is_english_line(line), "{line}");
        }
    }
}
