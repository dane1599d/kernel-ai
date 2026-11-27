//! Consent from the user's line (ADR 0002 D2, D3 and D6), as code with no model:
//! for each call in a plan, whether it runs at once, runs as "Taken as", asks, or is
//! refused. Spike S01-2's tier corpus (`tests/tier_corpus.rs`) measures it; task 7
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

/// Whether `line` names `row` by one of its cues, as consent reads a line:
/// `Some(false)` named, `Some(true)` named but negated or switched off ("never
/// move"), `None` not named. A question's or a tell clause's cue counts. For a
/// goal's scope (G2), which takes the person's words, never a model's.
pub fn named_in(line: &str, row: &Action) -> Option<bool> {
    read_every_cue(&words(line), row)
}

/// [`cued_in`] counting a telling clause's cue and a cue inside a name too:
/// whether the line names `row` at all, or says not to (`named_any`,
/// [`named_in`]). A quoted or joined span still names it, so `it's so slow,
/// "don't delete ideas"` is no vague want and a goal's `"never move
/// anything"` is still refused (#1467 review): only whether a call was asked
/// for skips a name's cues.
fn read_every_cue(words: &[Word], row: &Action) -> Option<bool> {
    cued_in(words, row, true, false, None)
}

/// Whether a cue of `row` starts a word of the line: `Some(false)` if some do and
/// none is negated, `Some(true)` if any is negated, `None` if none does. A negated
/// cue wins over the others, so "print the policy, but don't switch to round robin"
/// does not ask for a switch. Given `call`, a cue inside its target as the
/// line says it does not count either.
fn cued(words: &[Word], row: &Action, call: Option<&Call>) -> Option<bool> {
    cued_in(words, row, false, true, call)
}

/// [`cued`], with `telling_counts` saying whether a cue in a clause that asks to
/// be told counts: a question, or one that opens with a tell word. For whether a
/// call was asked for it does not (#721, ADR 0002): "tell me how to remove
/// ideas" asks for no removal. For whether the line names an action at all it
/// does, so a complaint beside it ("it's so slow, how do I remove note 2") is no
/// vague want. With `skip_names`, a cue inside a name is not counted, nor,
/// given `call`, one inside the call's own target as the line says it.
fn cued_in(
    words: &[Word],
    row: &Action,
    telling_counts: bool,
    skip_names: bool,
    call: Option<&Call>,
) -> Option<bool> {
    let telling = |clause: usize| {
        words
            .iter()
            .find(|w| w.clause == clause)
            .is_some_and(|w| TELL_WORDS.contains(&w.text.as_str()))
    };
    let mut found = None;
    for cue in row.cues {
        let off_counts = !row.cues.iter().any(|c| c.contains("off"));
        let cue_len = self::words(cue).len();
        for at in matches(words, cue, true) {
            // A question asks to be told: "is round robin on" asks for no switch.
            if !telling_counts && (words[at].question || telling(words[at].clause)) {
                continue;
            }
            let clause = words[at].clause;
            // So does a clause with a telling word before the cue ("tell me how
            // to delete note 2", "can you explain how to remove it"): it asks for
            // words, not for a change. The line still names the action, so
            // `named_any` counts it.
            if !telling_counts
                && (0..at).any(|i| {
                    words[i].clause == clause
                        && TELL_WORDS.contains(&words[i].text.as_str())
                        && !noun_before_cue(words, i, at)
                })
            {
                continue;
            }
            let negated = negated_for(words, at, at, Some(row));
            let off = off_counts
                && words
                    .iter()
                    .any(|w| w.clause == clause && OFF_WORDS.contains(&w.text.as_str()));
            if negated || off {
                return Some(true);
            }
            // A cue inside a longer name is part of the name, not a verb: "open
            // my to-delete folder" asks for no trash, "rename round-robin.md"
            // for no switch (bug-hunt-6 H6-3). A name that is the cue whole
            // ("switch to round-robin") still is. Only after the negation and
            // off checks: a negated cue in a name ("it's marked "do not
            // delete"", "my not-to-delete folder") still says not to (#1467's
            // round 20).
            if skip_names && inside_a_name(words, at, cue_len, row) {
                continue;
            }
            // So is one inside the call's own target, even a name made only
            // of the row's words: "open my trash-delete folder" asks for no
            // trash of it (#1467's round 21).
            if skip_names
                && call.is_some_and(|call| inside_argument(words, at, cue, cue_len, row, call))
            {
                continue;
            }
            found = Some(false);
        }
    }
    found
}

/// Whether the `len` words at `at` sit inside one token with more words than
/// them: part of a name joined by `-`, `.`, `/` or `_`, or quoted ("to
/// delete"). A token is a name only when one of its words is not `row`'s own
/// (a word of its cues, of an argument's alias or of an enum value):
/// "round-robin", "light-mode", "delete/trash" and "re-open" ask for the row,
/// "to-delete.txt" names a file. A token that is `row`'s own name
/// ("fs_create" in a C line) names the row, not a thing.
fn inside_a_name(words: &[Word], at: usize, len: usize, row: &Action) -> bool {
    let token = words[at].token;
    let span = &words[at..at + len];
    let whole: Vec<&str> = words
        .iter()
        .filter(|w| w.token == token)
        .map(|w| w.text.as_str())
        .collect();
    let own_name = self::words(row.name)
        .iter()
        .map(|w| w.text.as_str())
        .eq(whole.iter().copied());
    span.iter().all(|w| w.token == token)
        && whole.len() > len
        && !own_name
        && !whole.iter().all(|word| own_word(row, word))
}

/// Whether the `len` words of `cue` at `at` lie inside a match of one of
/// `call`'s `from_request` targets, the thing it acts on, as written or by
/// its place under home: "trash-delete" in "open my trash-delete folder" for
/// fs_trash on `/home/user/trash-delete`, "trash" in "open my trash folder"
/// on `/home/user/trash`. A cue said apart from it still counts ("trash
/// trash-delete"). Not an option or an enum's value ("light" in "switch to
/// light-mode" is how it sets), nor a cue that is a word of the target's
/// aliases.
fn inside_argument(
    words: &[Word],
    at: usize,
    cue: &str,
    len: usize,
    row: &Action,
    call: &Call,
) -> bool {
    row.args
        .iter()
        .filter(|arg| {
            arg.from_request && arg.role == Role::Target && !matches!(arg.ty, ArgType::Enum(_))
        })
        .filter(|arg| {
            !arg.aliases.iter().any(|(said, _)| {
                let said = self::words(said);
                self::words(cue)
                    .iter()
                    .all(|c| said.iter().any(|s| s.text == c.text))
            })
        })
        .filter_map(|arg| value_of(call, arg.name))
        .any(|value| {
            let text = value.text.strip_prefix(HOME).unwrap_or(value.text);
            let value_len = self::words(text).len();
            value_len >= len
                && matches(words, text, false)
                    .into_iter()
                    .any(|start| start <= at && at + len <= start + value_len)
        })
}

/// Whether `word` is one of `row`'s own: a word of one of its cues (or one
/// starting with a one-word cue stem, "polic" in "policy"), of an argument's
/// alias, or of an enum value it takes. "re" too, which only says again
/// ("re-open", "re-start").
fn own_word(row: &Action, word: &str) -> bool {
    let in_phrase = |phrase: &str| self::words(phrase).iter().any(|w| w.text == word);
    word == "re"
        || row
            .cues
            .iter()
            .any(|cue| in_phrase(cue) || (!cue.contains([' ', '-']) && word.starts_with(cue)))
        || row.args.iter().any(|arg| {
            arg.aliases
                .iter()
                .any(|(said, value)| in_phrase(said) || in_phrase(value))
                || match arg.ty {
                    ArgType::Enum(values) => values.iter().any(|v| in_phrase(v)),
                    _ => false,
                }
        })
}

/// Whether a negation before word `at` in its clause turns it off: "don't",
/// "never", "stop using" and the rest of [`NEGATIONS`], any word ending `n't`
/// or an auxiliary's `nt` ("didn't", "shouldnt"), "cannot", "refrain" and
/// "forbid", and "stop" or "quit" before an action's -ing word ("stop
/// switching"). A clause made only of negations and filler words carries its
/// negation into the clause after it, within its sentence: "never, ever
/// switch" and "don't, under any circumstances, trash it" negate (bug-hunt-6
/// H6-1). A clause whose only negating words are bare answers ("no,", "ok
/// no,", "neither.") negates nothing after it.
fn negated(words: &[Word], at: usize) -> bool {
    negated_for(words, at, at, None)
}

/// [`negated`] for a call of `row`, when it is known: "stop" or "quit" that is
/// one of the row's own cues is its verb, not a negation ("stop running the
/// miner" stops the miner). Only words before `upto` (at most `at`) negate, so
/// a name's own words never negate the rest of it ("trash not-mine.txt").
fn negated_for(words: &[Word], at: usize, upto: usize, row: Option<&Action>) -> bool {
    let clause = words[at].clause;
    let sentence = words[at].sentence;
    let before = &words[..upto];
    // All the words, not only those before: "stop" before the cue's own -ing
    // word ("stop trashing it") is read with the word after it.
    let negates = |i: usize| negation_at_for(words, i, row);
    if before
        .iter()
        .enumerate()
        .any(|(i, w)| w.clause == clause && negates(i))
    {
        return true;
    }
    // The clauses before this one, in its sentence, that hold only negations
    // and filler words, nearest first: one of them that negates reaches this
    // clause. "Not again. Switch to round robin." carries nothing.
    let mut carried = clause;
    while carried > 0 {
        carried -= 1;
        let held: Vec<usize> = (0..before.len())
            .filter(|&i| before[i].clause == carried)
            .collect();
        let Some(&first) = held.first() else {
            return false;
        };
        if before[first].sentence != sentence {
            // A sentence of only negation ("Not.", "Please don't.", "Absolutely
            // not.") is said of the next one. An answer ("No.", "Ok no.") or a
            // sigh ("Not again.") carries nothing.
            let said: Vec<usize> = (0..before.len())
                .filter(|&i| before[i].sentence == before[first].sentence)
                .collect();
            let bare = |i: usize| BARE_ANSWERS.contains(&before[i].text.as_str());
            let lone = before[first].sentence + 1 == sentence
                && said.iter().any(|&i| negates(i) && !bare(i))
                && said.iter().all(|&i| {
                    let text = before[i].text.as_str();
                    (negates(i) && !bare(i)) || (text != "again" && NEGATION_FILLER.contains(&text))
                });
            return lone;
        }
        let only_negation = held
            .iter()
            .all(|&i| negates(i) || NEGATION_FILLER.contains(&before[i].text.as_str()));
        if !only_negation {
            return false;
        }
        // An answer: every word of it is a bare answer or leads one in ("ok
        // no,", "no really,", "neither."). Anything else that negates carries:
        // "under no circumstances," is no answer (#1460 review).
        let answer = held.iter().all(|&i| {
            let text = before[i].text.as_str();
            BARE_ANSWERS.contains(&text) || ANSWER_LEAD.contains(&text)
        });
        if !answer && held.iter().any(|&i| negates(i)) {
            return true;
        }
    }
    false
}

/// Whether word `i` of `words` negates what follows it in its clause, for a
/// call of `row` when it is known ([`stops_an_action`]).
fn negation_at_for(words: &[Word], i: usize, row: Option<&Action>) -> bool {
    let text = words[i].text.as_str();
    NEGATIONS.contains(&text)
        || NEGATION_STARTS.iter().any(|start| text.starts_with(start))
        || text == "cannot"
        || text.ends_with("n't")
        || text
            .strip_suffix("nt")
            .is_some_and(|stem| NT_AUXILIARIES.contains(&stem))
        // "stop using round robin", "quit trashing my files"
        || (matches!(text, "stop" | "quit") && stops_an_action(words, i, row))
}

/// Whether "stop" or "quit" at `i` stops an action, and so negates it: the
/// next word ends "-ing" and is no noun of [`NOT_GERUNDS`]. It is in the same
/// clause, or "stop" is a clause of its own ("Stop. Switching to round robin
/// was a mistake"). "stop everything" and "Stop. Bring up the terminal" stop
/// none, but "stop anything from switching" and "stop everything that
/// switches" do. For a `row` that asks anyway (it derives above Run), whose
/// own cue "stop" is, "stop running" in one clause is the row's verb: "stop
/// running the miner" asks for the stop, so the call asks as a change with no
/// undo, which a standing rule for the row may run, not as one the model
/// added. "Stop! Running the miner is fine" stops nothing. Any other -ing word
/// negates: "stop shutting down build-3" kills nothing, so no standing rule
/// may run it (#1460 review).
fn stops_an_action(words: &[Word], i: usize, row: Option<&Action>) -> bool {
    let clause = words[i].clause;
    let alone = words.iter().filter(|w| w.clause == clause).all(|w| {
        matches!(w.text.as_str(), "stop" | "quit") || NEGATION_FILLER.contains(&w.text.as_str())
    });
    let Some(next) = words
        .get(i + 1)
        .filter(|n| n.clause == clause || (alone && n.clause == clause + 1))
    else {
        return false;
    };
    if !next.text.ends_with("ing") {
        return false;
    }
    if NOT_GERUNDS.contains(&next.text.as_str()) {
        // "stop anything from switching", "stop everything that switches".
        return next.text.ends_with("thing")
            && words.get(i + 2).is_some_and(|after| {
                after.clause == next.clause
                    && (matches!(after.text.as_str(), "from" | "that" | "which" | "who")
                        || after.text.ends_with("ing"))
            });
    }
    match row {
        Some(row) if derive(row) != Tier::Run && is_own_cue(row, &words[i].text) => {
            !(next.text == "running" && next.clause == clause)
        }
        _ => true,
    }
}

/// Whether `word` is one of `row`'s one-word cues.
fn is_own_cue(row: &Action, word: &str) -> bool {
    row.cues
        .iter()
        .any(|cue| !cue.contains([' ', '-']) && word.starts_with(cue))
}

/// Words ending "-ing" that are no action: "stop everything", "stop. bring".
const NOT_GERUNDS: &[&str] = &[
    "everything",
    "anything",
    "something",
    "nothing",
    "thing",
    "things",
    "bring",
    "string",
    "morning",
];

/// Starts of words that negate: refrain, refraining; forbid, forbidden.
const NEGATION_STARTS: &[&str] = &["refrain", "forbid"];

/// Auxiliaries that, with `nt` and no apostrophe, negate: "didnt", "shouldnt".
const NT_AUXILIARIES: &[&str] = &[
    "do", "does", "did", "is", "are", "was", "were", "has", "have", "had", "ca", "wo", "should",
    "could", "would", "must", "need", "ai", "might", "dare",
];

/// Words a clause of only negation may hold and still carry it into the next
/// clause: "never, ever", "don't, under any circumstances,".
const NEGATION_FILLER: &[&str] = &[
    "ever",
    "under",
    "any",
    "circumstances",
    "circumstance",
    "at",
    "all",
    "whatsoever",
    "please",
    "really",
    "absolutely",
    "seriously",
    "again",
    "i",
    "repeat",
    "and",
    "or",
    "just",
    "so",
    "kai",
    "ok",
    "okay",
];

/// A clause whose only negating words are these is an answer to something said
/// before ("no, switch to round robin", "neither, remove ideas"), and negates
/// nothing after it.
const BARE_ANSWERS: &[&str] = &["no", "nah", "nope", "neither"];

/// Words that may lead a bare answer in, or follow it, and keep it one.
const ANSWER_LEAD: &[&str] = &["ok", "okay", "so", "really", "just", "kai", "please"];

/// The person's own folder: a path under it is named by the rest of it.
const HOME: &str = "/home/user/";

/// Every `from_request` argument is literal in the line, an alias it uses, or a
/// symbol the line points at.
fn arguments_in_line(words: &[Word], row: &Action, call: &Call) -> bool {
    row.args.iter().filter(|arg| arg.from_request).all(|arg| {
        let Some(value) = value_of(call, arg.name) else {
            // An optional argument left out asks for nothing.
            return matches!(arg.ty, ArgType::Opt(_));
        };
        if matches!(value.label, Label::Content(_)) {
            return false;
        }
        // A value counts only where every word of it stands un-negated: in
        // "remove calc, not notes" notes is not asked for (bug-hunt-6 H6-2).
        let named = |text: &str| {
            names_a_thing(text)
                && (said_unnegated(words, text, row) || pointed_back(words, row, text))
        };
        named(value.text)
            // A file is named by its place under home: "a note called ideas"
            // names /home/user/ideas (files.rs's PATH).
            || value.text.strip_prefix(HOME).is_some_and(named)
            || arg.aliases.iter().any(|(written, means)| {
                fold_eq(means, value.text) && said_unnegated(words, written, row)
            })
            || (SYMBOLS.contains(&value.text)
                && (0..words.len()).any(|at| {
                    POINTERS.contains(&words[at].text.as_str())
                        && !negated_for(words, at, at, Some(row))
                }))
    })
}

/// Whether `phrase` stands in `words` somewhere none of its words is negated,
/// for a call of `row`. Only words before the phrase negate it: "no-ads" and
/// "not-mine.txt" are names.
fn said_unnegated(words: &[Word], phrase: &str, row: &Action) -> bool {
    let len = self::words(phrase).len();
    matches(words, phrase, false)
        .into_iter()
        .any(|start| (start..start + len).all(|at| !negated_for(words, at, start, Some(row))))
}

/// "I don't need calc anymore, remove it": the one name of a clause that only
/// says the person has no more use for it, taken by a pointer in the next
/// clause of its sentence, where an un-negated cue of `row` stands with
/// nothing else but filler. "never touch calc, remove it", "I don't want to
/// lose calc, remove it" and "I don't need calc or notes, remove it" take
/// nothing, and ask.
fn pointed_back(words: &[Word], row: &Action, phrase: &str) -> bool {
    let len = self::words(phrase).len();
    matches(words, phrase, false).into_iter().any(|start| {
        let clause = words[start].clause;
        let lead: Vec<&str> = words[..start]
            .iter()
            .filter(|w| w.clause == clause)
            .map(|w| w.text.as_str())
            .collect();
        let tail_end = (start + len..words.len())
            .find(|&i| words[i].clause != clause)
            .unwrap_or(words.len());
        let tail_ok = words[start + len..tail_end]
            .iter()
            .all(|w| NO_USE_TAIL.contains(&w.text.as_str()));
        tail_ok
            && no_use(&lead)
            && tail_end < words.len()
            && words[tail_end].sentence == words[start].sentence
            && points_back(words, row, words[tail_end].clause)
    })
}

/// Whether a clause's words before its name say only that the person has no
/// more use for it: "I don't need", "we no longer use the", "I really don't
/// want my".
fn no_use(lead: &[&str]) -> bool {
    let dets = lead
        .iter()
        .rev()
        .take_while(|w| DETERMINERS.contains(w))
        .count();
    let Some((verb, before)) = lead[..lead.len() - dets].split_last() else {
        return false;
    };
    NO_USE_VERBS.contains(verb)
        && before.iter().all(|w| NO_USE_LEAD.contains(w))
        && before
            .iter()
            .any(|w| NEGATIONS.contains(w) || w.ends_with("n't") || w.ends_with("nt"))
}

/// Whether `clause` is a cue of `row`, un-negated and not a question or a
/// tell clause, and a pointer after it, with only filler beside them:
/// "remove it", "so please uninstall it".
fn points_back(words: &[Word], row: &Action, clause: usize) -> bool {
    let held: Vec<usize> = (0..words.len())
        .filter(|&i| words[i].clause == clause)
        .collect();
    if held.iter().any(|&i| words[i].question) {
        return false;
    }
    let mut cue_at = None;
    let mut covered: Vec<usize> = Vec::new();
    for cue in row.cues {
        let cue_len = self::words(cue).len();
        for at in matches(words, cue, true) {
            if words[at].clause == clause
                && !inside_a_name(words, at, cue_len, row)
                && !negated_for(words, at, at, Some(row))
            {
                cue_at = Some(cue_at.map_or(at, |c: usize| c.min(at)));
                covered.extend(at..at + cue_len);
            }
        }
    }
    let Some(cue_at) = cue_at else {
        return false;
    };
    let pointer = held.iter().any(|&i| {
        i > cue_at
            && matches!(words[i].text.as_str(), "it" | "that" | "this")
            && !negated_for(words, i, i, Some(row))
    });
    pointer
        && held.iter().all(|&i| {
            covered.contains(&i)
                || matches!(words[i].text.as_str(), "it" | "that" | "this")
                || POINTER_FILLER.contains(&words[i].text.as_str())
        })
}

/// Verbs of a clause that says the person has no more use for a thing.
const NO_USE_VERBS: &[&str] = &["need", "want", "use"];
/// Words that may lead such a verb.
const NO_USE_LEAD: &[&str] = &[
    "i", "we", "do", "does", "not", "no", "longer", "don't", "dont", "doesn't", "doesnt", "really",
    "even", "actually", "just",
];
/// Words that may follow its name.
const NO_USE_TAIL: &[&str] = &["anymore", "any", "more", "now", "either", "at", "all"];
/// Words that may stand beside the cue and the pointer that takes it.
const POINTER_FILLER: &[&str] = &[
    "so", "please", "just", "then", "now", "and", "kai", "ok", "okay",
];

/// Every host argument is grounded (`nethosts::Grounding::decide`): typed in
/// the line, a host-table word of it, granted before, or the OS's checked
/// source, and none came from content. A word in a clause that negates counts
/// for nothing, so "don't fetch weather" grounds no host; a question counts,
/// since a read is asked as one ("what's the weather in Leeds?").
fn hosts_grounded(words: &[Word], hosts: &Hosts, row: &Action, call: &Call) -> bool {
    every_host(words, hosts, row, call, |verdict| {
        matches!(verdict, Verdict::Run(_))
    })
}

/// P-F: content goes out only to a destination the user's line names (ADR 0002
/// D6): every host of the call is typed in the line, or is the table's host for
/// a word of it. A grant or the OS's source list grounds a fetch, but is not the
/// user saying where this data goes. A call with no host names none.
fn line_names_destination(words: &[Word], hosts: &Hosts, row: &Action, call: &Call) -> bool {
    row.args.iter().any(|arg| matches!(arg.ty, ArgType::Host))
        && every_host(
            words,
            hosts,
            row,
            call,
            |verdict| matches!(verdict, Verdict::Run(ground) if ground.names_destination()),
        )
}

/// A send's destination (ADR 0002 D3, P5 close): every host is one the line
/// types; or, when the line types no host at all, a word of the host table
/// that follows "to" or "on" ("post it to wikipedia"). A table word never
/// stands in for a host the line typed.
fn send_destination(words: &[Word], hosts: &Hosts, row: &Action, call: &Call) -> bool {
    let typed_any = !nethosts::typed_hosts(hosts.line).is_empty();
    let after_to = |word: &str| {
        words.iter().enumerate().any(|(at, w)| {
            w.text == word
                && at > 0
                && matches!(words[at - 1].text.as_str(), "to" | "on")
                && !negated(words, at)
        })
    };
    row.args.iter().any(|arg| matches!(arg.ty, ArgType::Host))
        && every_host(words, hosts, row, call, |verdict| match verdict {
            Verdict::Run(Ground::Typed) => true,
            Verdict::Run(Ground::Table(word)) => !typed_any && after_to(word),
            _ => false,
        })
}

/// Whether `accept` takes the grounding verdict of every host argument of
/// `call`. A word in a clause that negates counts for nothing, and a host that
/// came from content is never grounded.
fn every_host(
    words: &[Word],
    hosts: &Hosts,
    row: &Action,
    call: &Call,
    accept: impl Fn(&Verdict) -> bool,
) -> bool {
    let marked: Vec<LineWord> = (0..words.len())
        .map(|at| LineWord::new(&words[at].text, !negated(words, at)))
        .collect();
    row.args
        .iter()
        .filter(|arg| matches!(arg.ty, ArgType::Host))
        .all(|arg| {
            value_of(call, arg.name).is_some_and(|value| {
                let origin = match value.label {
                    Label::Content(_) => Origin::Content,
                    _ => Origin::Line,
                };
                accept(
                    &hosts
                        .net
                        .decide(hosts.line, &marked, value.text, origin, Bytes::Unchecked),
                )
            })
        })
}

/// Whether some host of `call` is grounded only by an app's grant (ADR 0002
/// D3): the line neither types it nor holds a table word for it.
fn granted_only(words: &[Word], hosts: &Hosts, row: &Action, call: &Call) -> bool {
    !every_host(words, hosts, row, call, |verdict| {
        !matches!(verdict, Verdict::Run(Ground::Granted))
    })
}

/// Words that state a want or a complaint about how the system feels: the only
/// lines the vague path reads as asking for a change.
const WANT_WORDS: &[&str] = &[
    "snappier",
    "snappy",
    "faster",
    "quicker",
    "smoother",
    "slow",
    "slower",
    "sluggish",
    "laggy",
    "lag",
    "lagging",
    "lags",
    "choppy",
    "janky",
    "stutters",
    "stuttering",
    "unresponsive",
];
/// Phrases that do the same.
const WANT_PHRASES: &[&str] = &[
    "speed it up",
    "speed things up",
    "put it back",
    "put that back",
    "change it back",
    "set it back",
    "go back",
    "the way it was",
    "do it again",
    "do that again",
    "same again",
    "once more",
];

/// Lines that ask for the last thing again (P1 plan section 4): "do" starts
/// them as an order, not a question.
const REPEATS: &[&str] = &["do it again", "do that again"];

/// Whether the line states a want or a complaint ([`WANT_WORDS`], [`WANT_PHRASES`]).
fn wants(words: &[Word]) -> bool {
    words.iter().any(|w| WANT_WORDS.contains(&w.text.as_str()))
        || WANT_PHRASES
            .iter()
            .any(|phrase| !matches(words, phrase, false).is_empty())
}

/// Words that start a question in English.
const QUESTION_WORDS: &[&str] = &[
    "how", "what", "which", "who", "whom", "whose", "when", "where", "why", "is", "are", "was",
    "were", "does", "do", "did", "can", "could", "will", "would", "should", "has", "have",
    "what's", "how's", "where's", "who's", "when's", "why's", "which's",
];

/// Whether `line` is a question: it ends with `?`, or starts with a question word
/// ("how many CPUs are there", "is the network up").
fn is_question(line: &str) -> bool {
    let line = line.trim();
    let repeat = REPEATS.iter().any(|r| {
        line.get(..r.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(r))
    });
    line.ends_with('?')
        || !repeat
            && words(line)
                .first()
                .is_some_and(|w| QUESTION_WORDS.contains(&w.text.as_str()))
}

/// Words that start a line asking to be told something, not for a change.
/// Whether the telling word at `tell` is a noun naming the thing the cue at
/// `cue` asks for, and so asks to be told nothing: it comes right before the
/// cue word, after a determiner or a noun ("a shopping list app", "my report
/// app"). Anything else keeps the clause a telling one, so the rule fails
/// safe: "the list of packages to install", "a list of files I could delete"
/// and "can you briefly explain how to remove ideas" still ask nothing.
fn noun_before_cue(words: &[Word], tell: usize, cue: usize) -> bool {
    if tell + 1 != cue || tell == 0 || words[tell - 1].clause != words[tell].clause {
        return false;
    }
    let before = words[tell - 1].text.as_str();
    DETERMINERS.contains(&before)
        || (!NOT_NOUNS.contains(&before)
            && !TELL_WORDS.contains(&before)
            && !QUESTION_WORDS.contains(&before))
}

/// Words that are no noun before a telling word: pronouns, polite and modal
/// words, and fillers that lead an order to be told ("can you list", "please
/// describe", "I'd like you to explain").
const NOT_NOUNS: &[&str] = &[
    "i", "i'd", "i'm", "me", "you", "we", "us", "they", "them", "he", "she", "him", "her", "it",
    "please", "pls", "plz", "kindly", "just", "briefly", "quickly", "also", "now", "then", "and",
    "or", "so", "to", "of", "like", "want", "need", "kai", "hey", "ok", "okay", "go", "first",
    "again", "simply", "maybe",
];

const TELL_WORDS: &[&str] = &[
    "describe",
    "explain",
    "tell",
    "show",
    "list",
    "summarize",
    "summarise",
    "report",
];

/// Question words that only ever ask to be told: not `do`, `can` or `will`,
/// which also lead orders ("do the same for the editor").
const WH_WORDS: &[&str] = &[
    "how", "what", "which", "who", "whom", "whose", "when", "where", "why", "what's", "how's",
    "where's", "who's", "when's", "why's", "which's",
];

/// Whether `line` only asks to be told something: one ending in `?`, one led by
/// a wh-word ("how many cpus"), or one led by a telling word ("describe this",
/// "explain the policy"). A change consent [`Why::Added`] to such a line
/// answers nothing it asked, whatever the State or a page says (#721).
pub fn asks_to_be_told(line: &str) -> bool {
    let words = words(line);
    if wants(&words) {
        return false;
    }
    let first = words.first().map(|w| w.text.as_str());
    line.trim().ends_with('?')
        || first.is_some_and(|w| WH_WORDS.contains(&w) || TELL_WORDS.contains(&w))
}

/// Whether `text` names a thing: some word of it is neither a kind word nor a
/// determiner. "notes" and "the llm service" do; "the service" and "it" do not.
fn names_a_thing(text: &str) -> bool {
    words(text).iter().any(|w| {
        let w = w.text.as_str();
        !KIND_WORDS.contains(&w) && !DETERMINERS.contains(&w)
    })
}

/// One word of the line, lower case.
#[derive(Debug)]
struct Word {
    text: alloc::string::String,
    /// Which clause of the line it is in, counted from 0.
    clause: usize,
    /// Whether that clause is a question: it starts with a question word (and is
    /// not a polite request) or ends with `?`.
    question: bool,
    /// Which sentence of the line it is in, counted from 0: a sentence ends at
    /// `.` `!` `?` or `;`.
    sentence: usize,
    /// Whether that sentence ends with `?`, whatever its clauses.
    sentence_asks: bool,
    /// Which token of the line it is in: words joined by one `-`, `.`, `/` or
    /// `_` ("to-delete", "round-robin.md") share one, as a name's words do,
    /// and so do all the words of a quoted span ("to delete" in quotes).
    token: usize,
}

/// Marks that open or close a quoted span: straight and curly double quotes.
const QUOTES: [char; 3] = ['"', '\u{201C}', '\u{201D}'];

/// The line's words: letters, digits and apostrophes, lower case. Everything else
/// separates, so "round-robin", "round robin" and "Round Robin." read alike.
fn words(text: &str) -> Vec<Word> {
    // Curly apostrophes and their look-alikes, as phone keyboards and pastes
    // write them, are the straight one: "Don’t" is "don't".
    let folded: alloc::string::String = text
        .chars()
        .map(|c| match c {
            '\u{2019}' | '\u{2018}' | '\u{2BC}' | '\u{60}' | '\u{B4}' | '\u{2032}' | '\u{FF07}'
            | '\u{2B9}' | '\u{55A}' => '\'',
            c => c,
        })
        .collect();
    let text = folded.as_str();
    let is_word = |c: char| c.is_alphanumeric() || c == '\'';
    let mut clause = 0;
    let mut sentence = 0;
    let mut token = 0;
    let mut quoted = false;
    let mut asked_sentences: Vec<usize> = Vec::new();
    let mut out: Vec<Word> = Vec::new();
    let mut asked = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(|c: char| !is_word(c)).unwrap_or(rest.len());
        let word = rest[..end].trim_matches('\'').to_lowercase();
        rest = &rest[end..];
        let gap = rest.find(is_word).unwrap_or(rest.len());
        let had_word = !word.is_empty();
        if had_word {
            if CLAUSE_WORDS.contains(&word.as_str()) {
                clause += 1;
            }
            out.push(Word {
                text: word,
                clause,
                question: false,
                sentence,
                sentence_asks: false,
                token,
            });
        }
        if rest[..gap].contains('?') {
            asked.push(clause);
        }
        // A dot between two word characters is inside a word, a host's or a
        // number's ("hooks.example", "1.5"): it ends no clause or sentence.
        let inside = had_word && &rest[..gap] == "." && gap < rest.len();
        if !inside && rest[..gap].contains([',', ';', '.', '!', '?', ':']) {
            clause += 1;
        }
        if rest[..gap].contains('?') {
            asked_sentences.push(sentence);
        }
        // A run of dots ("Don't... remove it") trails off: it ends a clause,
        // not a sentence.
        let trails = rest[..gap].contains("..");
        if !inside
            && (rest[..gap].contains([';', '!', '?']) || (rest[..gap].contains('.') && !trails))
        {
            sentence += 1;
        }
        // A quoted span ("to delete") is one token, as a joined name is.
        let quotes = rest[..gap].matches(QUOTES).count();
        let opens = quotes % 2 == 1 && !quoted && rest[gap..].contains(QUOTES);
        let closes = quotes % 2 == 1 && quoted;
        let joined = had_word && gap < rest.len() && matches!(&rest[..gap], "-" | "." | "/" | "_");
        if !(joined || (quoted && !closes)) {
            token += 1;
        }
        quoted = (quoted && !closes) || opens;
        rest = &rest[gap..];
    }
    for c in 0..=clause {
        let mut in_clause = out.iter().filter(|w| w.clause == c);
        let first = in_clause.next().map(|w| w.text.clone());
        let second = in_clause.next().map(|w| w.text.clone());
        let polite = first.as_deref().is_some_and(|w| POLITE.contains(&w))
            && matches!(second.as_deref(), Some("you" | "u"));
        let question = !polite
            && (asked.contains(&c)
                || first
                    .as_deref()
                    .is_some_and(|w| QUESTION_WORDS.contains(&w)));
        for w in out.iter_mut().filter(|w| w.clause == c) {
            w.question = question;
        }
    }
    for w in out.iter_mut() {
        w.sentence_asks = asked_sentences.contains(&w.sentence);
    }
    out
}

/// Where `phrase`'s words appear in a row in `words`. With `prefix`, its last word
/// need only start a word of the line, as a cue does ("polic" in "policy").
fn matches(words: &[Word], phrase: &str, prefix: bool) -> Vec<usize> {
    let parts = self::words(phrase);
    let Some((last, first)) = parts.split_last() else {
        return Vec::new();
    };
    if words.len() < parts.len() {
        return Vec::new();
    }
    (0..=words.len() - parts.len())
        .filter(|&at| {
            first
                .iter()
                .zip(&words[at..])
                .all(|(p, w)| p.text == w.text)
                && {
                    let w = &words[at + first.len()].text;
                    if prefix {
                        w.starts_with(last.text.as_str())
                    } else {
                        *w == last.text
                    }
                }
        })
        .collect()
}

fn fold_eq(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.text == y.text)
}

/// v0.1's English check: no letter outside the Latin alphabet, and at least half
/// the words in the shipped word list or written like code (digits, a dot).
pub fn is_english_line(line: &str) -> bool {
    is_english(line)
}

fn is_english(line: &str) -> bool {
    if line
        .chars()
        .any(|c| c.is_alphabetic() && !c.is_ascii_alphabetic())
    {
        return false;
    }
    // Whole tokens here, so `report.txt` or `build-3` counts once, as code.
    let tokens: Vec<&str> = line
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|t| !t.is_empty())
        .collect();
    let known = tokens.iter().filter(|t| known_token(t)).count();
    tokens.len() < 3 || known * 2 >= tokens.len()
}

fn known_token(token: &str) -> bool {
    let word = token.to_ascii_lowercase();
    let word = word.trim_matches('\'');
    token.contains(['.', '/', '_', '-', ':', '@'])
        || token.chars().any(|c| c.is_ascii_digit())
        || ENGLISH.binary_search(&word).is_ok()
        || ENGLISH_STEMS.iter().any(|stem| word.starts_with(stem))
}

/// Common English words, sorted. A line needs half its words here (or among the
/// stems) to count as English; names and numbers in it are the other half.
const ENGLISH: &[&str] = &[
    "a",
    "about",
    "add",
    "after",
    "again",
    "all",
    "also",
    "an",
    "and",
    "any",
    "apt",
    "are",
    "as",
    "at",
    "back",
    "be",
    "been",
    "before",
    "below",
    "brew",
    "but",
    "by",
    "call",
    "called",
    "can",
    "change",
    "changed",
    "changes",
    "check",
    "clean",
    "close",
    "containing",
    "contents",
    "could",
    "create",
    "current",
    "dark",
    "delete",
    "dnf",
    "do",
    "does",
    "doesn't",
    "don't",
    "down",
    "each",
    "else",
    "empty",
    "erase",
    "every",
    "feel",
    "few",
    "fewer",
    "first",
    "folder",
    "folders",
    "for",
    "free",
    "from",
    "get",
    "give",
    "go",
    "has",
    "have",
    "hello",
    "history",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "jot",
    "just",
    "keep",
    "last",
    "less",
    "let",
    "light",
    "like",
    "line",
    "list",
    "long",
    "look",
    "make",
    "many",
    "me",
    "mode",
    "more",
    "most",
    "move",
    "much",
    "my",
    "name",
    "named",
    "names",
    "new",
    "next",
    "no",
    "not",
    "note",
    "notes",
    "now",
    "number",
    "of",
    "off",
    "old",
    "on",
    "one",
    "only",
    "open",
    "or",
    "other",
    "out",
    "over",
    "overwrite",
    "pacman",
    "please",
    "print",
    "pull",
    "put",
    "recover",
    "remove",
    "rename",
    "restore",
    "run",
    "same",
    "save",
    "saying",
    "second",
    "see",
    "set",
    "show",
    "so",
    "some",
    "something",
    "start",
    "stop",
    "sudo",
    "sure",
    "switch",
    "take",
    "tell",
    "text",
    "than",
    "thanks",
    "that",
    "the",
    "their",
    "them",
    "theme",
    "then",
    "there",
    "these",
    "things",
    "this",
    "three",
    "tidy",
    "to",
    "too",
    "total",
    "trash",
    "turn",
    "two",
    "type",
    "up",
    "use",
    "used",
    "using",
    "was",
    "way",
    "what",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whole",
    "why",
    "will",
    "winget",
    "with",
    "would",
    "write",
    "you",
    "your",
];

/// Stems of English words the OS's requests use; a word starting with one counts.
const ENGLISH_STEMS: &[&str] = &[
    "answer",
    "app",
    "array",
    "browser",
    "char",
    "comput",
    "context",
    "count",
    "cpu",
    "decimal",
    "digit",
    "disk",
    "edit",
    "email",
    "eject",
    "fals",
    "file",
    "format",
    "heap",
    "high",
    "instal",
    "interactiv",
    "kernel",
    "length",
    "letter",
    "low",
    "map",
    "memor",
    "migrat",
    "minute",
    "model",
    "modul",
    "note",
    "packag",
    "percent",
    "pid",
    "polic",
    "process",
    "ratio",
    "read",
    "reboot",
    "restart",
    "robin",
    "round",
    "rule",
    "schedul",
    "second",
    "servic",
    "snapp",
    "sum",
    "system",
    "thread",
    "time",
    "true",
    "upper",
    "uptime",
    "version",
    "watchdog",
    "websit",
    "word",
    "yes",
];

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
