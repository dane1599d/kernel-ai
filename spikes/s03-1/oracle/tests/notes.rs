//! S03-1's size and string measures, on today's notes logic bound two ways:
//! by hand through compsdk (`kai_handle`), and by wit-bindgen on
//! `kai:app/component` through `appabi`.
//!
//! - Size: the wit-bindgen module at most 20 KiB over the hand-bound one.
//! - Strings: 1 KiB in and 1 KiB out through `appabi` at most 1.5x the time
//!   `kai_handle` takes, both on wasmi in this harness.

use appabi_spike::{Core, Func, Ty, Val};
use s03_1_oracle::{WasmiGuest, build_guest};
use std::time::{Duration, Instant};

const ROUNDS: usize = 2_000;

fn hand_notes() -> Vec<u8> {
    let root = s03_1_oracle::spike_dir().join("../..");
    let target = root.join("target/s03-1-hand");
    let status = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "-q",
            "-p",
            "comp-notes",
            "--target",
            "wasm32-unknown-unknown",
            "--profile",
            "component",
        ])
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &target)
        .env("KAI_VERSION", "2")
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read(target.join("wasm32-unknown-unknown/component/comp_notes.wasm")).unwrap()
}

/// Today's binding: memory from `kai_alloc`, the reply packed as `(ptr << 32) | len`.
fn kai_handle(g: &mut WasmiGuest, request: &str) -> String {
    let len = request.len() as i32;
    let at = match g.call_core("kai_alloc", &[Core::I32(len)]).unwrap()[..] {
        [Core::I32(at)] => at as u32,
        _ => panic!("kai_alloc"),
    };
    appabi_spike::Guest::write(g, at, request.as_bytes()).unwrap();
    let packed = match g
        .call_core("kai_handle", &[Core::I32(at as i32), Core::I32(len)])
        .unwrap()[..]
    {
        [Core::I64(p)] => p as u64,
        _ => panic!("kai_handle"),
    };
    let bytes = appabi_spike::Guest::read(g, (packed >> 32) as u32, packed as u32).unwrap();
    String::from_utf8(bytes).unwrap()
}

fn wit_func(export: &str, params: Vec<Ty>, result: Option<Ty>) -> Func {
    Func {
        export: export.into(),
        params,
        result,
    }
}

fn handle_func() -> Func {
    wit_func(
        "kai:probe-notes/requests@0.1.0#handle",
        vec![Ty::String],
        Some(Ty::String),
    )
}

fn wit_handle(g: &mut WasmiGuest, request: &str) -> String {
    match g
        .invoke(&handle_func(), &[Val::String(request.into())])
        .unwrap()
    {
        Some(Val::String(reply)) => reply,
        other => panic!("{other:?}"),
    }
}

fn start_wit(g: &mut WasmiGuest) {
    let init = wit_func(
        "kai:live/lifecycle@0.1.0#init",
        vec![],
        Some(Ty::Result(
            None,
            Some(Box::new(Ty::Enum(vec![
                "unsupported-schema".into(),
                "bad-state".into(),
            ]))),
        )),
    );
    assert_eq!(g.invoke(&init, &[]).unwrap(), Some(Val::Result(Ok(None))));
}

#[test]
fn the_wit_bindgen_module_is_at_most_20_kib_larger() {
    let hand = hand_notes();
    let wit = build_guest("notes-wit", "notes_wit");
    let delta = wit.len() as i64 - hand.len() as i64;
    println!(
        "S03-1 size: hand-bound notes {} bytes, wit-bindgen notes {} bytes, difference {delta:+} bytes",
        hand.len(),
        wit.len()
    );
    assert!(delta <= 20 * 1024);
}

fn time(mut f: impl FnMut()) -> Duration {
    // Warm up, then the median of five runs of ROUNDS calls.
    f();
    let mut runs: Vec<Duration> = (0..5)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..ROUNDS {
                f();
            }
            start.elapsed()
        })
        .collect();
    runs.sort();
    runs[2]
}

#[test]
fn a_1_kib_string_in_and_out_costs_at_most_1_5x_kai_handle() {
    let note: String = (0..1030)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    let (add, find) = (format!("add {note}"), format!("find {note}"));

    let mut hand = WasmiGuest::new(&hand_notes());
    assert!(matches!(
        hand.call_core("kai_init", &[]).unwrap()[..],
        [Core::I32(0)]
    ));
    let mut wit = WasmiGuest::new(&build_guest("notes-wit", "notes_wit"));
    start_wit(&mut wit);

    assert_eq!(kai_handle(&mut hand, &add), wit_handle(&mut wit, &add));
    let out = kai_handle(&mut hand, "get 1");
    assert!(out.len() >= 1024, "{}", out.len());
    assert_eq!(out, wit_handle(&mut wit, "get 1"));
    assert_eq!(kai_handle(&mut hand, &find), wit_handle(&mut wit, &find));

    // 1 KiB in (`find`), then 1 KiB out (`get`), per round.
    let hand_time = time(|| {
        kai_handle(&mut hand, &find);
        kai_handle(&mut hand, "get 1");
    });
    let wit_time = time(|| {
        wit_handle(&mut wit, &find);
        wit_handle(&mut wit, "get 1");
    });
    let ratio = wit_time.as_secs_f64() / hand_time.as_secs_f64();
    println!(
        "S03-1 strings: {ROUNDS} rounds of 1 KiB in and out: kai_handle {hand_time:?}, appabi {wit_time:?}, {ratio:.2}x"
    );
    assert!(ratio <= 1.5, "{ratio:.2}x");
}
