//! Whether a call's hosts are grounded in the line, a host-table word or a
//! grant (`nethosts`).

use super::*;

/// Every host argument is grounded (`nethosts::Grounding::decide`): typed in
/// the line, a host-table word of it, granted before, or the OS's checked
/// source, and none came from content. A word in a clause that negates counts
/// for nothing, so "don't fetch weather" grounds no host; a question counts,
/// since a read is asked as one ("what's the weather in Leeds?").
pub(super) fn hosts_grounded(words: &[Word], hosts: &Hosts, row: &Action, call: &Call) -> bool {
    every_host(words, hosts, row, call, |verdict| {
        matches!(verdict, Verdict::Run(_))
    })
}

/// P-F: content goes out only to a destination the user's line names (ADR 0002
/// D6): every host of the call is typed in the line, or is the table's host for
/// a word of it. A grant or the OS's source list grounds a fetch, but is not the
/// user saying where this data goes. A call with no host names none.
pub(super) fn line_names_destination(
    words: &[Word],
    hosts: &Hosts,
    row: &Action,
    call: &Call,
) -> bool {
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
pub(super) fn send_destination(words: &[Word], hosts: &Hosts, row: &Action, call: &Call) -> bool {
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
pub(super) fn every_host(
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
pub(super) fn granted_only(words: &[Word], hosts: &Hosts, row: &Action, call: &Call) -> bool {
    !every_host(words, hosts, row, call, |verdict| {
        !matches!(verdict, Verdict::Run(Ground::Granted))
    })
}
