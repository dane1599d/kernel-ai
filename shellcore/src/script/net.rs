//! The agent's network rows on the strict engine (P5 task 11): `net_fetch`
//! and `net_lookup` (`:resolve` and `:ping` as one row). Each reaches only a
//! host consent grounded (`agentabi::consent`, through `nethosts`); the plan
//! check here makes sure the call that runs names the host the line was
//! decided for. What a fetch or a lookup reads comes back as `Data`, so it can
//! steer nothing that follows without a question (ADR 0002 D6). Nothing is
//! changed, so nothing is journalled.

use super::{RhaiResult, Tx, rhai_error};
use crate::{Host, data};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use llmproto::query::{self, QueryKind, QueryRequest};
use rhai::{Array, Engine};

/// The longest URL `fetch` takes: `kainet::wire::MAX_URL` (shellcore does not
/// link kainet; a test holds the two equal).
const MAX_URL: usize = 2048;

/// Refuses a fetch whose URL, `https://<host><path>` (an empty path is `/`),
/// is longer than `fetch` takes, before anything is asked or sent (#831).
fn url_fits(host: &str, path: &str) -> Result<(), String> {
    let len = "https://".len() + host.len() + path.len().max(1);
    match len > MAX_URL {
        true => Err(format!("the URL is longer than {MAX_URL} bytes")),
        false => Ok(()),
    }
}

pub(super) fn bind(engine: &mut Engine, name: &'static str, h: Rc<dyn Host>, tx: Tx) -> bool {
    let fail = move |why: String| rhai_error(format!("{name}: {why}"));
    match name {
        "net_fetch" => {
            let (h2, tx2) = (Rc::clone(&h), tx.clone());
            engine.register_fn(name, move |host: &str| -> RhaiResult<data::Data> {
                url_fits(host, "").map_err(fail)?;
                tx2.approved(name, &[host])?;
                h2.net_fetch(host, "")
                    .map(|text| data::Data::new("web", text))
                    .map_err(fail)
            });
            let (h3, tx3) = (Rc::clone(&h), tx.clone());
            engine.register_fn(
                name,
                move |host: &str, path: &str| -> RhaiResult<data::Data> {
                    url_fits(host, path).map_err(fail)?;
                    tx.approved(name, &[host, path])?;
                    h.net_fetch(host, path)
                        .map(|text| data::Data::new("web", text))
                        .map_err(fail)
                },
            );
            // Read content as the path: the one way `Data` leaves the machine.
            // Consent let it through only to a host the line names (P-F), or
            // after the user said yes to an ask that named its source.
            engine.register_fn(
                name,
                move |host: &str, path: data::Data| -> RhaiResult<data::Data> {
                    url_fits(host, path.text()).map_err(fail)?;
                    tx3.approved(name, &[host, path.text()])?;
                    h3.net_fetch(host, path.text())
                        .map(|text| data::Data::new("web", text))
                        .map_err(fail)
                },
            );
        }
        "net_lookup" => {
            engine.register_fn(name, move |host: &str| -> RhaiResult<data::Data> {
                tx.approved(name, &[host])?;
                let addrs = h.net_resolve(host).map_err(fail)?;
                // Where no ping reaches (ring 3 has none), the addresses alone.
                let text = match h.net_ping(host) {
                    Ok(ms) if ms < 0 => format!("{host}: {}; no answer", addrs.join(", ")),
                    Ok(ms) => format!("{host}: {}; answers in {ms} ms", addrs.join(", ")),
                    Err(_) => format!("{host}: {}", addrs.join(", ")),
                };
                Ok(data::Data::new("dns", text))
            });
        }
        "query" => {
            let (h2, tx2) = (Rc::clone(&h), tx.clone());
            engine.register_fn(
                name,
                move |d: data::Data, question: &str| -> RhaiResult<data::Data> {
                    tx2.approved(name, &[d.text(), question])?;
                    query(h2.as_ref(), d, question, QueryKind::Short).map_err(fail)
                },
            );
            let (h3, tx3) = (Rc::clone(&h), tx.clone());
            engine.register_fn(
                name,
                move |d: data::Data, question: &str, answers: Array| -> RhaiResult<data::Data> {
                    let words = words(answers).map_err(fail)?;
                    tx3.approved(name, &[d.text(), question, &words.join(",")])?;
                    query(h3.as_ref(), d, question, kind_of(words)).map_err(fail)
                },
            );
            // Text the script wrote itself (a tagged call's arguments) is no
            // one's content: its answer is labelled as the query's own.
            let (h4, tx4) = (Rc::clone(&h), tx.clone());
            engine.register_fn(
                name,
                move |text: &str, question: &str| -> RhaiResult<data::Data> {
                    tx4.approved(name, &[text, question])?;
                    let d = data::Data::new("query", text);
                    query(h4.as_ref(), d, question, QueryKind::Short).map_err(fail)
                },
            );
            engine.register_fn(
                name,
                move |text: &str, question: &str, answers: Array| -> RhaiResult<data::Data> {
                    let words = words(answers).map_err(fail)?;
                    tx.approved(name, &[text, question, &words.join(",")])?;
                    let d = data::Data::new("query", text);
                    query(h.as_ref(), d, question, kind_of(words)).map_err(fail)
                },
            );
        }
        _ => return false,
    }
    true
}

/// The answers a script offers, as text: each one a string.
fn words(answers: Array) -> Result<Vec<String>, String> {
    answers
        .into_iter()
        .map(|answer| {
            answer
                .into_immutable_string()
                .map(|word| String::from(word.as_str()))
                .map_err(|_| String::from("each answer is a string"))
        })
        .collect()
}

/// `["yes", "no"]` is a yes or no; any other list is a choice of its words.
fn kind_of(words: Vec<String>) -> QueryKind {
    match words.as_slice() {
        [yes, no] if yes == "yes" && no == "no" => QueryKind::Bool,
        _ => QueryKind::Enum(words),
    }
}

/// One quarantined read: the question about `d`, asked apart, and the answer
/// (checked again against the kind) as `Data` that keeps `d`'s label, so what the data said steers nothing.
fn query(
    host: &dyn Host,
    d: data::Data,
    question: &str,
    kind: QueryKind,
) -> Result<data::Data, String> {
    // `new` cuts the data to `MAX_DATA_BYTES` on a character, so a page of
    // non-ASCII text under the row's character limit still fits a message.
    let request = QueryRequest::new(d.text(), question, kind)?;
    // `llm` checks the answer against the kind; a wrong or older one must not
    // hand the script an off-list word or a long line, so check it again.
    let answer = host.query(&request)?;
    let answer = query::check_answer(&request.kind, &answer)?;
    Ok(data::Data::new(d.source, answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_limit_is_fetchs_and_a_longer_url_is_refused() {
        assert_eq!(MAX_URL, kainet::wire::MAX_URL);
        // `https://example.com` is 19 bytes.
        let fits = format!("/{}", "x".repeat(MAX_URL - 20));
        assert_eq!(url_fits("example.com", &fits), Ok(()));
        assert_eq!(url_fits("example.com", ""), Ok(()));
        let long = format!("{fits}x");
        assert_eq!(
            url_fits("example.com", &long),
            Err(String::from("the URL is longer than 2048 bytes"))
        );
        assert!(url_fits(&"a".repeat(MAX_URL), "").is_err());
    }
}
