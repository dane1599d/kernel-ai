//! Whether a call's arguments are the line's own words: literal, an alias, under
//! home, or pointed back at ("this", "it").

use super::*;

/// The person's own folder: a path under it is named by the rest of it.
pub(super) const HOME: &str = "/home/user/";

/// Every `from_request` argument is literal in the line, an alias it uses, or a
/// symbol the line points at.
pub(super) fn arguments_in_line(words: &[Word], row: &Action, call: &Call) -> bool {
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
pub(super) fn said_unnegated(words: &[Word], phrase: &str, row: &Action) -> bool {
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
pub(super) fn pointed_back(words: &[Word], row: &Action, phrase: &str) -> bool {
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
pub(super) fn no_use(lead: &[&str]) -> bool {
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
pub(super) fn points_back(words: &[Word], row: &Action, clause: usize) -> bool {
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
pub(super) const NO_USE_VERBS: &[&str] = &["need", "want", "use"];
/// Words that may lead such a verb.
pub(super) const NO_USE_LEAD: &[&str] = &[
    "i", "we", "do", "does", "not", "no", "longer", "don't", "dont", "doesn't", "doesnt", "really",
    "even", "actually", "just",
];
/// Words that may follow its name.
pub(super) const NO_USE_TAIL: &[&str] = &["anymore", "any", "more", "now", "either", "at", "all"];
/// Words that may stand beside the cue and the pointer that takes it.
pub(super) const POINTER_FILLER: &[&str] = &[
    "so", "please", "just", "then", "now", "and", "kai", "ok", "okay",
];
