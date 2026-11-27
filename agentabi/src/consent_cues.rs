//! How a line names a row: its cues, read whole, negated or switched off, and
//! the cues a name or an argument holds.

use super::*;

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
pub(super) fn read_every_cue(words: &[Word], row: &Action) -> Option<bool> {
    cued_in(words, row, true, false, None)
}

/// Whether a cue of `row` starts a word of the line: `Some(false)` if some do and
/// none is negated, `Some(true)` if any is negated, `None` if none does. A negated
/// cue wins over the others, so "print the policy, but don't switch to round robin"
/// does not ask for a switch. Given `call`, a cue inside its target as the
/// line says it does not count either.
pub(super) fn cued(words: &[Word], row: &Action, call: Option<&Call>) -> Option<bool> {
    cued_in(words, row, false, true, call)
}

/// [`cued`], with `telling_counts` saying whether a cue in a clause that asks to
/// be told counts: a question, or one that opens with a tell word. For whether a
/// call was asked for it does not (#721, ADR 0002): "tell me how to remove
/// ideas" asks for no removal. For whether the line names an action at all it
/// does, so a complaint beside it ("it's so slow, how do I remove note 2") is no
/// vague want. With `skip_names`, a cue inside a name is not counted, nor,
/// given `call`, one inside the call's own target as the line says it.
pub(super) fn cued_in(
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
                && words[..at]
                    .iter()
                    .any(|w| w.clause == clause && TELL_WORDS.contains(&w.text.as_str()))
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
pub(super) fn inside_a_name(words: &[Word], at: usize, len: usize, row: &Action) -> bool {
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
///
/// A cue inside any match of the target is skipped, even where the target is
/// said twice: "trash the trash folder" asks. Its first "trash" reads alike
/// to a second naming of the folder ("open my trash folder and tidy my trash
/// folder"), and a word's place does not tell a verb from a name, so the
/// line that may be either asks (#1577's review).
pub(super) fn inside_argument(
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
            if value_len < len {
                return false;
            }
            matches(words, text, false)
                .into_iter()
                .any(|start| start <= at && at + len <= start + value_len)
        })
}

/// Whether `word` is one of `row`'s own: a word of one of its cues (or one
/// starting with a one-word cue stem, "polic" in "policy"), of an argument's
/// alias, or of an enum value it takes. "re" too, which only says again
/// ("re-open", "re-start").
pub(super) fn own_word(row: &Action, word: &str) -> bool {
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
pub(super) fn negated(words: &[Word], at: usize) -> bool {
    negated_for(words, at, at, None)
}

/// [`negated`] for a call of `row`, when it is known: "stop" or "quit" that is
/// one of the row's own cues is its verb, not a negation ("stop running the
/// miner" stops the miner). Only words before `upto` (at most `at`) negate, so
/// a name's own words never negate the rest of it ("trash not-mine.txt").
pub(super) fn negated_for(words: &[Word], at: usize, upto: usize, row: Option<&Action>) -> bool {
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
pub(super) fn negation_at_for(words: &[Word], i: usize, row: Option<&Action>) -> bool {
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
pub(super) fn stops_an_action(words: &[Word], i: usize, row: Option<&Action>) -> bool {
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
pub(super) fn is_own_cue(row: &Action, word: &str) -> bool {
    row.cues
        .iter()
        .any(|cue| !cue.contains([' ', '-']) && word.starts_with(cue))
}

/// Words ending "-ing" that are no action: "stop everything", "stop. bring".
pub(super) const NOT_GERUNDS: &[&str] = &[
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
pub(super) const NEGATION_STARTS: &[&str] = &["refrain", "forbid"];

/// Auxiliaries that, with `nt` and no apostrophe, negate: "didnt", "shouldnt".
pub(super) const NT_AUXILIARIES: &[&str] = &[
    "do", "does", "did", "is", "are", "was", "were", "has", "have", "had", "ca", "wo", "should",
    "could", "would", "must", "need", "ai", "might", "dare",
];

/// Words a clause of only negation may hold and still carry it into the next
/// clause: "never, ever", "don't, under any circumstances,".
pub(super) const NEGATION_FILLER: &[&str] = &[
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
pub(super) const BARE_ANSWERS: &[&str] = &["no", "nah", "nope", "neither"];

/// Words that may lead a bare answer in, or follow it, and keep it one.
pub(super) const ANSWER_LEAD: &[&str] = &["ok", "okay", "so", "really", "just", "kai", "please"];
