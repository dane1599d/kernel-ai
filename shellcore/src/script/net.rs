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
use rhai::Engine;

pub(super) fn bind(engine: &mut Engine, name: &'static str, h: Rc<dyn Host>, tx: Tx) -> bool {
    let fail = move |why: String| rhai_error(format!("{name}: {why}"));
    match name {
        "net_fetch" => {
            let (h2, tx2) = (Rc::clone(&h), tx.clone());
            engine.register_fn(name, move |host: &str| -> RhaiResult<data::Data> {
                tx2.approved(name, &[host])?;
                h2.net_fetch(host, "")
                    .map(|text| data::Data::new("web", text))
                    .map_err(fail)
            });
            engine.register_fn(
                name,
                move |host: &str, path: &str| -> RhaiResult<data::Data> {
                    tx.approved(name, &[host, path])?;
                    h.net_fetch(host, path)
                        .map(|text| data::Data::new("web", text))
                        .map_err(fail)
                },
            );
        }
        "net_lookup" => {
            engine.register_fn(name, move |host: &str| -> RhaiResult<data::Data> {
                tx.approved(name, &[host])?;
                let addrs = h.net_resolve(host).map_err(fail)?;
                let ms = h.net_ping(host).map_err(fail)?;
                let answer = match ms {
                    ms if ms < 0 => String::from("no answer"),
                    ms => format!("answers in {ms} ms"),
                };
                Ok(data::Data::new(
                    "dns",
                    format!("{host}: {}; {answer}", addrs.join(", ")),
                ))
            });
        }
        _ => return false,
    }
    true
}
