//! S03-1's deferred figures (ADR 0003, decisions row "S03-1 reordered and run
//! lean"): the view guest's 300-node rebuild plus `appabi` lift, and its layout
//! pass over 8 KiB of text on a 240x67 grid, on wasmi in a hard-float ring-3
//! program with comphost's crates and dispatch mode. Five rounds; each prints its
//! p50 and p99, then the median of the rounds against S03-1's bars (at most 4 ms
//! each). The figures are reported, not asserted: KVM runners stall for 100 ms
//! and more (docs/decisions.md, "KVM CI stalls"). It exits 0 when every result
//! is right. Boot case `test=appbench`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use appabi::{Func, Ty, Val};
use user::{println, sys};
use wasmhost::{CoreInstance, Limits, load_core};

user::entry!(main);

static GUEST: &[u8] =
    include_bytes!("../../../target/wasm32-unknown-unknown/component/comp_viewbench.wasm");

const ROUNDS: usize = 5;
/// No round starts after this long, so a TCG boot ends within its case timeout.
const BUDGET_NS: u64 = 90_000_000_000;
const REBUILDS: usize = 500;
const LAYOUTS: usize = 200;
/// S03-1's pass bars, in nanoseconds.
const BAR_NS: u64 = 4_000_000;

fn field(name: &str, ty: Ty) -> (String, Ty) {
    (String::from(name), ty)
}

fn rebuild_func() -> Func {
    let kind = Ty::Enum(
        ["root", "header", "row", "footer"]
            .map(String::from)
            .to_vec(),
    );
    let rect = Ty::Record(vec![
        field("x", Ty::U32),
        field("y", Ty::U32),
        field("width", Ty::U32),
        field("height", Ty::U32),
    ]);
    let node = Ty::Record(vec![
        field("id", Ty::U32),
        field("parent", Ty::U32),
        field("kind", kind),
        field("rect", rect),
        field("text", Ty::String),
    ]);
    Func {
        export: String::from("rebuild"),
        params: vec![Ty::U32],
        result: Some(Ty::List(Box::new(node))),
    }
}

fn layout_func() -> Func {
    Func {
        export: String::from("layout"),
        params: vec![Ty::U32, Ty::U32],
        result: Some(Ty::List(Box::new(Ty::String))),
    }
}

/// The value at quantile `q` (0-100) of `samples`, nearest-rank.
fn quantile(samples: &mut [u64], q: usize) -> u64 {
    samples.sort_unstable();
    let rank = (q * samples.len()).div_ceil(100).max(1);
    samples[rank - 1]
}

fn ms(ns: u64) -> String {
    alloc::format!("{}.{:03}", ns / 1_000_000, ns % 1_000_000 / 1000)
}

/// A rebuild's nodes are right: 300, the header's text, and every id in order.
fn check_view(value: &Option<Val>) -> bool {
    let Some(Val::List(nodes)) = value else {
        return false;
    };
    nodes.len() == 300
        && nodes.iter().enumerate().all(|(i, node)| {
            matches!(node, Val::Record(fields) if fields.first() == Some(&Val::U32(i as u32)))
        })
        && matches!(&nodes[1], Val::Record(f) if f.get(4) == Some(&Val::String(String::from("Notes: 1000"))))
}

/// A layout's rows are right: 67 rows of at most 240 bytes, over 30 of them used.
fn check_layout(value: &Option<Val>) -> bool {
    let Some(Val::List(rows)) = value else {
        return false;
    };
    let used = rows
        .iter()
        .filter(|row| matches!(row, Val::String(s) if !s.is_empty()))
        .count();
    rows.len() == 67
        && used > 30
        && rows
            .iter()
            .all(|row| matches!(row, Val::String(s) if s.len() <= 240))
}

/// One round: (rebuild p50, p99), (layout p50, p99), or None on a wrong result.
fn round(
    guest: &mut CoreInstance,
    rebuild: &Func,
    layout: &Func,
) -> Option<((u64, u64), (u64, u64))> {
    let mut times = Vec::with_capacity(REBUILDS);
    for event in 0..REBUILDS {
        let start = sys::time_ns();
        let value = appabi::call(guest, rebuild, &[Val::U32(event as u32 * 7)]).ok()?;
        times.push(sys::time_ns() - start);
        if event % 100 == 0 && !check_view(&value) {
            return None;
        }
    }
    let view = (quantile(&mut times, 50), quantile(&mut times, 99));
    times.clear();
    for i in 0..LAYOUTS {
        let start = sys::time_ns();
        let value = appabi::call(guest, layout, &[Val::U32(240), Val::U32(67)]).ok()?;
        times.push(sys::time_ns() - start);
        if i % 50 == 0 && !check_layout(&value) {
            return None;
        }
    }
    let wrap = (quantile(&mut times, 50), quantile(&mut times, 99));
    Some((view, wrap))
}

fn main() -> i32 {
    let limits = Limits {
        fuel_call: 50_000_000,
        fuel_life: 50_000_000,
        memory_bytes: 16 << 20,
    };
    let start = sys::time_ns();
    let mut guest = match load_core(GUEST, limits, None) {
        Ok(guest) => guest,
        Err(fault) => {
            println!("appbench: the guest did not load: {fault:?}");
            return 1;
        }
    };
    println!(
        "appbench: guest {} bytes compiled and instantiated in {} ms",
        GUEST.len(),
        ms(sys::time_ns() - start)
    );
    let (rebuild, layout) = (rebuild_func(), layout_func());
    let (mut view_p99, mut wrap_p99) = (Vec::new(), Vec::new());
    let build = if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    };
    println!("appbench: {build} build, hard float, wasmi portable dispatch");
    for r in 0..ROUNDS {
        if r > 0 && sys::time_ns() - start > BUDGET_NS {
            break;
        }
        // A blocking call between rounds, so the burst limit counts one round.
        sys::sleep_ms(1);
        let Some((view, wrap)) = round(&mut guest, &rebuild, &layout) else {
            println!("appbench: round {r}: a wrong result");
            return 1;
        };
        println!(
            "appbench: round {r}: rebuild plus lift p50 {} p99 {} ms; layout p50 {} p99 {} ms",
            ms(view.0),
            ms(view.1),
            ms(wrap.0),
            ms(wrap.1)
        );
        view_p99.push(view.1);
        wrap_p99.push(wrap.1);
    }
    let (view, wrap) = (quantile(&mut view_p99, 50), quantile(&mut wrap_p99, 50));
    let verdict = |ns: u64| if ns <= BAR_NS { "met" } else { "MISSED" };
    println!(
        "appbench: median of {} rounds: rebuild plus lift p99 {} ms (bar 4 ms: {}), layout p99 {} ms (bar 4 ms: {})",
        view_p99.len(),
        ms(view),
        verdict(view),
        ms(wrap),
        verdict(wrap)
    );
    println!("appbench: done");
    0
}
