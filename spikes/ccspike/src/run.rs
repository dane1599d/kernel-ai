//! The whole spike, as one run: measures (a)-(g) of ADR 0012's S12-1, each
//! reported as `ccspike:` lines through `log`. The host tests and the boot case
//! both call [`all`]; the boot case's numbers are the ones that count.

use crate::corpus::{self, CORPUS};
use crate::rhai_bench::{self, Rhai};
use crate::{Clock, CompilationMode, Compiler, Session};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What the run is given by its host.
pub struct Env<'a> {
    pub cc_wasm: &'a [u8],
    pub clock: Clock,
    /// Bytes the process heap holds now, if the host can tell.
    pub heap: Option<fn() -> usize>,
    /// Compiles per entry and prelude, for p50 and p95.
    pub repeats: usize,
    /// Run the Rhai loops (minutes under TCG).
    pub rhai: bool,
}

/// The pass bars of S12-1 this run can check by itself.
#[derive(Default, Debug)]
pub struct Verdict {
    pub failures: Vec<String>,
}

const PRELUDES: [usize; 3] = [0, 200, 1000];
const MS: u64 = 1_000_000;

fn ms(ns: u64) -> String {
    format!("{}.{:03}", ns / MS, (ns % MS) / 1000)
}

/// The value at quantile `q` (0-100) of `samples`, nearest-rank.
pub fn quantile(samples: &mut [u64], q: usize) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    samples.sort_unstable();
    let rank = (q * samples.len()).div_ceil(100).max(1);
    samples[rank - 1]
}

pub fn all(env: &Env, log: &mut dyn FnMut(&str)) -> Verdict {
    let mut verdict = Verdict::default();
    let mut fail = |verdict: &mut Verdict, log: &mut dyn FnMut(&str), why: String| {
        log(&format!("ccspike: FAIL {why}"));
        verdict.failures.push(why);
    };
    let headers: &[(&str, &[u8])] = &[("kai.h", corpus::KAI_H.as_bytes())];
    log(&format!(
        "ccspike: (a) cc.wasm {} bytes, {} active data segments",
        env.cc_wasm.len(),
        data_segments(env.cc_wasm)
    ));
    // (b) translation, both modes; the heap delta is the module's memory.
    let mut compiler = None;
    for mode in [CompilationMode::Lazy, CompilationMode::Eager] {
        let before = env.heap.map(|h| h());
        match Compiler::new(env.cc_wasm, headers, mode, env.clock) {
            Ok((cc, ns)) => {
                let heap = match (before, env.heap) {
                    (Some(b), Some(h)) => format!("{} KiB", h().saturating_sub(b) / 1024),
                    _ => String::from("unknown"),
                };
                log(&format!(
                    "ccspike: (a) validates under wasmhost's proposal set; (b) translate {mode:?} {} ms, heap {heap}",
                    ms(ns)
                ));
                compiler = Some(cc);
            }
            Err(e) => fail(&mut verdict, log, format!("(a) cc.wasm does not load: {e}")),
        }
    }
    let Some(cc) = compiler else {
        return verdict;
    };
    corpus_runs(env, &cc, &mut verdict, log, &mut fail);
    fuel_slices(env, &cc, &mut verdict, log, &mut fail);
    redefinition(env, &cc, &mut verdict, log, &mut fail);
    speed(env, &cc, &mut verdict, log, &mut fail);
    verdict
}

type Fail<'f> = dyn FnMut(&mut Verdict, &mut dyn FnMut(&str), String) + 'f;

/// (c) and (d): the corpus under each prelude, compiled `repeats` times per entry.
fn corpus_runs(
    env: &Env,
    cc: &Compiler,
    verdict: &mut Verdict,
    log: &mut dyn FnMut(&str),
    fail: &mut Fail,
) {
    for lines in PRELUDES {
        let prelude = corpus::prelude(lines);
        let mut session = match Session::new(CompilationMode::Lazy) {
            Ok(s) => s,
            Err(e) => return fail(verdict, log, format!("session: {e}")),
        };
        let (mut compile, mut link, mut inst, mut run) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut peak, mut fuel_max) = (0usize, 0u64);
        for (index, entry) in CORPUS.iter().enumerate() {
            let source = format!("{prelude}{}", entry.source);
            let mut object = None;
            for _ in 0..env.repeats.max(1) {
                match cc.compile(&source, env.clock) {
                    Ok(out) => {
                        compile.push(out.ns);
                        peak = peak.max(out.memory_bytes);
                        fuel_max = fuel_max.max(out.fuel);
                        object = Some(out);
                    }
                    Err(e) => {
                        fail(
                            verdict,
                            log,
                            format!("compile {} (prelude {lines}): {e}", entry.name),
                        );
                        break;
                    }
                }
            }
            let Some(out) = object else { continue };
            let ran = match session.eval(&out.object, entry.run.map(|(f, _)| f), env.clock) {
                Ok(ran) => ran,
                Err(e) => {
                    fail(
                        verdict,
                        log,
                        format!("{} (prelude {lines}): {e}", entry.name),
                    );
                    continue;
                }
            };
            link.push(ran.link_ns);
            inst.push(ran.instantiate_ns);
            if let Some((_, want)) = entry.run {
                run.push(ran.run_ns);
                if ran.result != Some(want) {
                    let got = ran.result;
                    fail(
                        verdict,
                        log,
                        format!("{} returned {got:?}, not {want}", entry.name),
                    );
                }
            }
            if lines == 0 {
                let lines_of_entry = entry.source.lines().count();
                log(&format!(
                    "ccspike: entry {:2} {:10} {:3} lines: compile {} ms fuel {} object {} B; link {} inst {} run {} ms{}",
                    index + 1,
                    entry.name,
                    lines_of_entry,
                    ms(out.ns),
                    out.fuel,
                    out.object.len(),
                    ms(ran.link_ns),
                    ms(ran.instantiate_ns),
                    ms(ran.run_ns),
                    needs(&ran.needs)
                ));
            }
        }
        let out = session.take_output();
        let (p50, p95) = (
            quantile(&mut compile.clone(), 50),
            quantile(&mut compile, 95),
        );
        log(&format!(
            "ccspike: (c) prelude {lines:4}: compile p50 {} p95 {} ms; link p50 {} inst p50 {} run p50 {} ms; (d) compiler memory peak {} KiB, fuel max {fuel_max}",
            ms(p50),
            ms(p95),
            ms(quantile(&mut link, 50)),
            ms(quantile(&mut inst, 50)),
            ms(quantile(&mut run, 50)),
            peak / 1024
        ));
        if lines == 0 {
            log(&format!("ccspike: session output {:?}", out));
        }
        check_bars(lines, p50, p95, peak, verdict, log, fail);
    }
}

fn needs(round_trips: &[u64]) -> String {
    if round_trips.is_empty() {
        return String::new();
    }
    let list: Vec<String> = round_trips
        .iter()
        .map(|ns| format!("{} us", ns / 1000))
        .collect();
    format!("; need/resume {}", list.join(", "))
}

fn check_bars(
    lines: usize,
    p50: u64,
    p95: u64,
    peak: usize,
    verdict: &mut Verdict,
    log: &mut dyn FnMut(&str),
    fail: &mut Fail,
) {
    let bars: &[(bool, &str)] = match lines {
        200 => &[
            (p50 <= 150 * MS, "200-line p50 <= 150 ms"),
            (p95 <= 400 * MS, "200-line p95 <= 400 ms"),
        ],
        1000 => &[(p50 <= 400 * MS, "1,000-line p50 <= 400 ms")],
        _ => &[],
    };
    for (ok, bar) in bars {
        log(&format!(
            "ccspike: bar {bar}: {}",
            if *ok { "met" } else { "MISSED" }
        ));
    }
    if peak > 32 << 20 {
        fail(
            verdict,
            log,
            format!("compiler memory {} KiB > 32 MiB", peak / 1024),
        );
    }
}

/// `ResumableCallOutOfFuel`: a compile in small slices, resumed after each, gives
/// the same object as one in a single slice.
fn fuel_slices(
    env: &Env,
    cc: &Compiler,
    verdict: &mut Verdict,
    log: &mut dyn FnMut(&str),
    fail: &mut Fail,
) {
    let source = format!("{}{}", corpus::prelude(200), corpus::ENTRY_NOTES);
    let whole = cc.compile(&source, env.clock);
    let sliced = cc.with_slice(100_000).compile(&source, env.clock);
    match (whole, sliced) {
        (Ok(w), Ok(s)) if w.object == s.object => log(&format!(
            "ccspike: fuel: {} fuel in 1 slice vs {} slices of 100,000, resumed each time; same object ({} B); {} vs {} ms",
            w.fuel,
            s.slices,
            s.object.len(),
            ms(w.ns),
            ms(s.ns)
        )),
        (Ok(_), Ok(_)) => fail(
            verdict,
            log,
            "fuel: sliced compile made a different object".into(),
        ),
        (w, s) => fail(verdict, log, format!("fuel: {:?} / {:?}", w.err(), s.err())),
    }
}

/// (g): a redefinition switches an existing caller and a stored function pointer.
fn redefinition(
    env: &Env,
    cc: &Compiler,
    verdict: &mut Verdict,
    log: &mut dyn FnMut(&str),
    fail: &mut Fail,
) {
    let mut session = match Session::new(CompilationMode::Lazy) {
        Ok(s) => s,
        Err(e) => return fail(verdict, log, format!("session: {e}")),
    };
    let prelude = corpus::prelude(0);
    for (step, (source, run)) in corpus::REDEFINE.iter().enumerate() {
        let result = cc
            .compile(&format!("{prelude}{source}"), env.clock)
            .and_then(|out| session.eval(&out.object, run.map(|(f, _)| f), env.clock));
        match (result, run) {
            (Ok(ran), Some((_, want))) if ran.result != Some(*want) => {
                return fail(
                    verdict,
                    log,
                    format!("(g) step {step}: {:?}, not {want}", ran.result),
                );
            }
            (Ok(_), _) => {}
            (Err(e), _) => return fail(verdict, log, format!("(g) step {step}: {e}")),
        }
    }
    log(
        "ccspike: (g) redefining twice switched an existing caller and a stored pointer (1010 then 1515)",
    );
}

/// (e) and (f): the loops in Kai C against Rhai, and need/resume round trips.
fn speed(
    env: &Env,
    cc: &Compiler,
    verdict: &mut Verdict,
    log: &mut dyn FnMut(&str),
    fail: &mut Fail,
) {
    let mut session = match Session::new(CompilationMode::Lazy) {
        Ok(s) => s,
        Err(e) => return fail(verdict, log, format!("session: {e}")),
    };
    let prelude = corpus::prelude(0);
    let cases = [
        (
            "sieve",
            corpus::SIEVE,
            "__kai_run_sieve",
            corpus::SIEVE_PRIMES,
            corpus::RHAI_SIEVE,
        ),
        (
            "sum",
            corpus::SUM,
            "__kai_run_sum",
            corpus::sum_expected(10_000_000),
            corpus::RHAI_SUM,
        ),
    ];
    for (name, source, run, want, rhai) in cases {
        let ran = cc
            .compile(&format!("{prelude}{source}"), env.clock)
            .and_then(|out| session.eval(&out.object, Some(run), env.clock));
        let c_ns = match ran {
            Ok(ran) if ran.result == Some(want) => ran.run_ns,
            Ok(ran) => {
                return fail(
                    verdict,
                    log,
                    format!("(e) {name}: {:?}, not {want}", ran.result),
                );
            }
            Err(e) => return fail(verdict, log, format!("(e) {name}: {e}")),
        };
        log(&format!("ccspike: (e) {name}: Kai C {} ms", ms(c_ns)));
        if !env.rhai {
            continue;
        }
        for engine in Rhai::ALL {
            match rhai_bench::run(engine, rhai, env.clock) {
                Ok((value, ns)) if value == want => {
                    let ratio = ns / c_ns.max(1);
                    log(&format!(
                        "ccspike: (e) {name}: {} {} ms, {ratio}x Kai C",
                        engine.name(),
                        ms(ns)
                    ));
                    if matches!(engine, Rhai::Ast127) && ratio < 10 {
                        fail(
                            verdict,
                            log,
                            format!("(e) {name}: Kai C only {ratio}x faster than Rhai AST"),
                        );
                    }
                }
                Ok((value, _)) => fail(
                    verdict,
                    log,
                    format!("(e) {name}: {} gave {value}", engine.name()),
                ),
                Err(e) => fail(verdict, log, format!("(e) {name}: {}: {e}", engine.name())),
            }
        }
    }
    // (f): 1,000 need/resume round trips in one run.
    let source = "I64 __kai_run_needs(void) { I64 s = 0; for (I32 i = 0; i < 1000; ++i) s += kai_need(i); return s; }\n";
    let ran = cc
        .compile(&format!("{prelude}{source}"), env.clock)
        .and_then(|out| session.eval(&out.object, Some("__kai_run_needs"), env.clock));
    match ran {
        Ok(mut ran) if ran.result == Some(500_500) => {
            let p50 = quantile(&mut ran.needs, 50);
            let p95 = quantile(&mut ran.needs, 95);
            log(&format!(
                "ccspike: (f) need/resume in process: p50 {} ns, p95 {} ns over {} round trips (bar: median <= 100 us shell to cc and back)",
                p50,
                p95,
                ran.needs.len()
            ));
        }
        Ok(ran) => fail(verdict, log, format!("(f) needs: {:?}", ran.result)),
        Err(e) => fail(verdict, log, format!("(f) needs: {e}")),
    }
}

/// Active data segments in a module, from its data section's count.
fn data_segments(wasm: &[u8]) -> u32 {
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let Some(size) = leb(wasm, &mut at) else {
            return 0;
        };
        if id == 11 {
            let mut inner = at;
            return leb(wasm, &mut inner).unwrap_or(0);
        }
        at += size as usize;
    }
    0
}

fn leb(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let (mut value, mut shift) = (0u32, 0);
    loop {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u32::from(byte & 0x7f).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
}
