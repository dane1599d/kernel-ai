//! An installed app's rows as script calls (P4, the agent-to-app route): each
//! is a `kai-call 1` to the app, through the supervisor. A change keeps the
//! app's checkpoint undo (`Inverse::App`), its result is the app's `Data`, and
//! the effects it queued go out only once the whole run succeeded, to run
//! through the agent core with their own tiers (`Shell::run_effects`).

use super::{Queued, RhaiResult, Tx, rhai_error};
use crate::apps::AppRow;
use crate::data::Data;
use crate::undo::{Done, Inverse, Name};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use appabi::{Ty, Val, wave};
use core::any::TypeId;
use rhai::{Dynamic, Engine, NativeCallContext};

/// Binds `row` for each arity it takes: its trailing options may be left out.
pub(super) fn bind(engine: &mut Engine, row: AppRow, tx: &Tx) {
    let all = row.row.args.len();
    let required = row
        .row
        .args
        .iter()
        .rposition(|arg| !matches!(arg.ty, agentabi::ArgType::Opt(_)))
        .map_or(0, |at| at + 1);
    for arity in required..=all {
        let (row, tx) = (row.clone(), tx.clone());
        let types: Vec<TypeId> = (0..arity).map(|_| TypeId::of::<Dynamic>()).collect();
        engine.register_raw_fn(
            row.row.name,
            types,
            move |_: NativeCallContext, args: &mut [&mut Dynamic]| -> RhaiResult<Data> {
                call(&row, &tx, args)
            },
        );
    }
}

fn call(row: &AppRow, tx: &Tx, args: &mut [&mut Dynamic]) -> RhaiResult<Data> {
    let name = row.row.name;
    let fail = |why: String| rhai_error(format!("{name}: {why}"));
    let live: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let live: Vec<&str> = live.iter().map(String::as_str).collect();
    tx.approved(name, &live)?;
    let record = encode(&row.args, args).map_err(fail)?;
    let request = format!(
        "{}\nfunc {}\nargs {record}\n",
        livecore::call::CALL_HEADER,
        row.export
    );
    let text = tx.host.app_call(row.app, &request).map_err(fail)?;
    let reply = livecore::call::Reply::parse(&text).map_err(fail)?;
    // A row the app declared read-only that changed its state lied: the change
    // is put back at once, and the call fails.
    if let (true, Some(id)) = (row.row.facts.read_only, reply.undo) {
        let put_back = crate::apps::undo(tx.host.as_ref(), row.app, id, true).is_ok();
        return Err(fail(format!(
            "it is declared read-only but changed {}'s state ({})",
            row.app,
            match put_back {
                true => "put back",
                false => "it could not be put back",
            }
        )));
    }
    if let Some(id) = reply.undo {
        let app = Name::new(row.app).ok_or_else(|| fail(String::from("its name is too long")))?;
        tx.changed(Done {
            action: name.into(),
            owner: row.app.into(),
            res: format!("{}{}", crate::actlog::APP_RES, row.app),
            before: String::new(),
            after: done_words(row),
            inverse: Some(Inverse::App { app, id }),
        });
    }
    if !reply.effects.is_empty() {
        let lines: String = reply
            .effects
            .iter()
            .map(|(ticket, effect)| format!("effect {ticket} {effect}\n"))
            .collect();
        let (host, app) = (tx.host.clone(), row.app);
        tx.outbox.borrow_mut().push(Queued {
            action: name,
            send: Box::new(move || {
                let apps = host
                    .apps()
                    .ok_or_else(|| String::from("there are no apps here"))?;
                apps.borrow_mut().effects.push((app.to_string(), lines));
                Ok(())
            }),
        });
    }
    Ok(Data::new(row.app, reply.result.unwrap_or_default()))
}

/// What a change by `row` says it did: its doc's first clause in the app,
/// `add a note in Notes` for `notes-window`'s "Add a note and return its id.",
/// or its name with no doc.
fn done_words(row: &AppRow) -> String {
    let clause = row
        .row
        .doc
        .split('.')
        .next()
        .and_then(|first| first.split(" and ").next())
        .map(str::trim)
        .filter(|clause| !clause.is_empty());
    let what = match clause {
        Some(clause) => {
            // An acronym ("PID of ...") keeps its capitals.
            let acronym = clause
                .chars()
                .nth(1)
                .is_some_and(|c| c.is_ascii_uppercase());
            let mut chars = clause.chars();
            let first = chars
                .next()
                .map(|c| if acronym { c } else { c.to_ascii_lowercase() });
            first.into_iter().chain(chars).collect()
        }
        None => String::from(row.row.name),
    };
    format!("{what} in {}", crate::apps::title(row.app))
}

/// The call's arguments as the WAVE record `ty` is; a left-out option is none.
fn encode(ty: &Ty, args: &[&mut Dynamic]) -> Result<String, String> {
    let Ty::Record(fields) = ty else {
        return Err(String::from("its arguments are no record"));
    };
    let values = fields
        .iter()
        .enumerate()
        .map(|(at, (_, field))| match args.get(at) {
            Some(value) => val(field, value),
            None if matches!(field, Ty::Option(_)) => Ok(Val::Option(None)),
            None => Err(String::from("an argument is missing")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    wave::print(&Val::Record(values), ty)
}

/// One script value as WAVE type `ty`.
fn val(ty: &Ty, value: &Dynamic) -> Result<Val, String> {
    let int = || {
        value
            .as_int()
            .map_err(|_| format!("{value} is not a number"))
    };
    let ranged = |min: i64, max: i64| {
        let n = int()?;
        match (min..=max).contains(&n) {
            true => Ok(n),
            false => Err(format!("{n} is out of range")),
        }
    };
    Ok(match ty {
        Ty::String => Val::String(text(value)?),
        Ty::Bool => Val::Bool(match value.as_bool() {
            Ok(flag) => flag,
            Err(_) => text(value)? == "true",
        }),
        Ty::U8 => Val::U8(ranged(0, u8::MAX.into())? as u8),
        Ty::U16 => Val::U16(ranged(0, u16::MAX.into())? as u16),
        Ty::U32 => Val::U32(ranged(0, u32::MAX.into())? as u32),
        Ty::U64 => Val::U64(ranged(0, i64::MAX)? as u64),
        Ty::S8 => Val::S8(ranged(i8::MIN.into(), i8::MAX.into())? as i8),
        Ty::S16 => Val::S16(ranged(i16::MIN.into(), i16::MAX.into())? as i16),
        Ty::S32 => Val::S32(ranged(i32::MIN.into(), i32::MAX.into())? as i32),
        Ty::S64 => Val::S64(int()?),
        Ty::Enum(cases) => {
            let case = text(value)?;
            let at = cases
                .iter()
                .position(|c| *c == case)
                .ok_or_else(|| format!("{case} is not one of {}", cases.join(", ")))?;
            Val::Enum(at as u32)
        }
        Ty::Option(inner) if value.is_unit() => {
            let _ = inner;
            Val::Option(None)
        }
        Ty::Option(inner) => Val::Option(Some(Box::new(val(inner, value)?))),
        Ty::List(item) => {
            let items = value
                .clone()
                .into_array()
                .map_err(|_| format!("{value} is not a list"))?;
            Val::List(
                items
                    .iter()
                    .map(|v| val(item, v))
                    .collect::<Result<_, _>>()?,
            )
        }
        _ => return Err(String::from("an argument type the OS does not pass")),
    })
}

fn text(value: &Dynamic) -> Result<String, String> {
    value
        .clone()
        .into_string()
        .map_err(|_| format!("{value} is not text"))
}
