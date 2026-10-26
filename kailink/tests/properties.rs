//! Properties over random sessions, deterministic by seed (fuzzing proper is P8):
//! random orders of the fixtures, each linked, maybe run, and committed only when
//! it links. After every step the session's layout must hold.

mod common;

#[path = "../../testkit/hostile.rs"]
mod hostile;

use common::{FIXTURES, Runner};
use hostile::XorShift;
use kailink::{Error, Symbol};

/// The data symbols lie in [data_base, data_end) and never overlap; function
/// slots lie in [1, table_size); the cursors never move back.
fn check_layout(r: &Runner, last: (u32, u32)) -> (u32, u32) {
    let s = &r.session;
    let (base, end, slots) = (s.layout().data_base, s.data_end(), s.table_size());
    assert!(end >= last.1 && slots >= last.0, "a cursor moved back");
    let mut ranges = Vec::new();
    for (name, symbol) in s.symbols() {
        match symbol {
            Symbol::Data { addr, size } => {
                assert!(*addr >= base && addr + size <= end, "{name} outside data");
                ranges.push((*addr, addr + size, name.to_string()));
            }
            Symbol::Func { slot, .. } => assert!(*slot >= 1 && *slot < slots, "{name} slot"),
        }
    }
    ranges.sort();
    for pair in ranges.windows(2) {
        assert!(
            pair[0].1 <= pair[1].0,
            "{} overlaps {}",
            pair[0].2,
            pair[1].2
        );
    }
    (slots, end)
}

#[test]
fn random_sessions_keep_their_layout() {
    for seed in 1..=60u64 {
        let mut rng = XorShift::new(seed);
        let mut r = Runner::new();
        let mut cursors = (r.session.table_size(), r.session.data_end());
        for _ in 0..25 {
            let name = FIXTURES[rng.below(FIXTURES.len())];
            match r.link(name) {
                Ok(linked) => {
                    // Linking alone changes nothing.
                    assert_eq!(r.session.table_size(), cursors.0);
                    assert_eq!(r.session.data_end(), cursors.1);
                    assert!(linked.table_size >= cursors.0 && linked.data_end >= cursors.1);
                    r.instantiate(&linked, None);
                    r.session.commit(&linked);
                }
                Err(Error::Undefined(_) | Error::TypeMismatch(_)) => {}
                Err(other) => panic!("seed {seed}: {name}: {other}"),
            }
            cursors = check_layout(&r, cursors);
        }
    }
}

#[test]
fn linking_is_deterministic() {
    let mut a = Runner::new();
    let mut b = Runner::new();
    for name in ["define_sq", "global", "fnptr", "notes", "use_sq"] {
        let (la, lb) = (a.link(name).unwrap(), b.link(name).unwrap());
        assert_eq!(la.wasm, lb.wasm, "{name}");
        a.session.commit(&la);
        b.session.commit(&lb);
    }
}

#[test]
fn every_definition_is_reachable_through_its_slot() {
    // After any random session, calling a function symbol's slot through the
    // table reaches a function of the recorded type: wasmi checks the type on
    // call_indirect, and uses_many calls through three thunks.
    for seed in 1..=20u64 {
        let mut rng = XorShift::new(seed * 7919);
        let mut r = Runner::new();
        for _ in 0..10 {
            let name = FIXTURES[rng.below(FIXTURES.len())];
            if let Ok(linked) = r.link(name) {
                r.instantiate(&linked, None);
                r.session.commit(&linked);
            }
        }
        let have = |n| matches!(r.session.symbol(n), Some(Symbol::Func { .. }));
        let sq_i32 = matches!(
            r.session.symbol("sq"),
            Some(Symbol::Func { ty, .. }) if ty.params == [kailink::ValType::I32]
        );
        if sq_i32 && have("fib") && matches!(r.session.symbol("counter"), Some(Symbol::Data { .. }))
        {
            assert!(r.eval("uses_many", Some("__kai_run_20")).is_some());
        }
    }
}
