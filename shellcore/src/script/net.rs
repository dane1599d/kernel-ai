//! The agent's network rows on the strict engine (P5 task 11): `net_fetch`,
//! `net_resolve` and `net_ping`. Each reaches only a host consent grounded
//! (`agentabi::consent`, through `nethosts`); the plan check here makes sure the
//! call that runs names the host the line was decided for. What a fetch or a
//! resolve reads comes back as `Data`, so it can steer nothing that follows
//! without a question (ADR 0002 D6). Nothing is changed, so nothing is journalled.

use super::{RhaiResult, Tx, rhai_error};
use crate::{Host, data};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use rhai::{Engine, INT};

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
        "net_resolve" => {
            engine.register_fn(name, move |host: &str| -> RhaiResult<data::Data> {
                tx.approved(name, &[host])?;
                h.net_resolve(host)
                    .map(|addrs| data::Data::new("dns", addrs.join("\n")))
                    .map_err(fail)
            });
        }
        "net_ping" => {
            engine.register_fn(name, move |host: &str| -> RhaiResult<INT> {
                tx.approved(name, &[host])?;
                h.net_ping(host).map(|ms| ms as INT).map_err(fail)
            });
        }
        _ => return false,
    }
    true
}
