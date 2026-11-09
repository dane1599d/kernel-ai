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
            let (h3, tx3) = (Rc::clone(&h), tx.clone());
            engine.register_fn(
                name,
                move |host: &str, path: &str| -> RhaiResult<data::Data> {
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
        _ => return false,
    }
    true
}
