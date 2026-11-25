//! Spike S14-3 js-engines: what both runners share. Each runner implements
//! [`Engine`] for its engine; `main` hands its arguments to [`main`].
//!
//! Modes:
//! - `empty`: start the engine, run `0`, print `init_us`, `eval_us`, `mem`.
//! - `bench <file>...`: run the files as one script, `print` echoed, then
//!   `total_ms` and `mem`.
//! - `t262 <root> <manifest> <start>`: run the manifest's tests from line
//!   `start`, a fresh engine each, printing `S <i>` before and
//!   `R <i> pass|fail <why>` after each, so the driver can restart after a
//!   hang or a trap.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// A value a script threw: its constructor's name, and its text.
#[derive(Debug)]
pub struct Thrown {
    pub name: String,
    pub message: String,
}

impl Thrown {
    pub fn other(message: impl Into<String>) -> Self {
        Thrown {
            name: String::from("(engine)"),
            message: message.into(),
        }
    }
}

pub trait Engine: Sized {
    /// A fresh engine: runtime, context and its one realm, with `print` and
    /// `$262` defined.
    fn new() -> Self;
    /// Runs `src` as a classic script.
    fn script(&mut self, name: &str, src: &str) -> Result<(), Thrown>;
    /// Loads, links and evaluates the module at guest path `path`, jobs
    /// included, until its promise settles.
    fn module(&mut self, path: &str, src: &str) -> Result<(), Thrown>;
    /// Runs pending jobs until none is left.
    fn run_jobs(&mut self) -> Result<(), Thrown>;
}

thread_local! {
    static PRINTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}
static ECHO: AtomicBool = AtomicBool::new(false);

/// What the engines' `print` calls.
pub fn print(line: String) {
    if ECHO.load(Ordering::Relaxed) {
        println!("{line}");
    }
    PRINTED.with(|p| p.borrow_mut().push(line));
}

fn take_printed() -> Vec<String> {
    PRINTED.with(|p| core::mem::take(&mut *p.borrow_mut()))
}

/// The guest's linear memory now, which only grows: its peak.
pub fn memory() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::memory_size(0) * 65536
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}

pub fn main<E: Engine>() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("empty") => empty::<E>(),
        Some("bench") => bench::<E>(&args[2..]),
        Some("t262") => t262::<E>(&args[2], &args[3], args[4].parse().expect("start")),
        _ => {
            eprintln!("usage: empty | bench <file>... | t262 <root> <manifest> <start>");
            std::process::exit(2);
        }
    }
}

fn empty<E: Engine>() {
    let start = Instant::now();
    let mut engine = E::new();
    let made = start.elapsed();
    engine.script("empty", "0").expect("0 runs");
    let ran = start.elapsed() - made;
    println!(
        "init_us {} eval_us {} mem {}",
        made.as_micros(),
        ran.as_micros(),
        memory()
    );
}

fn bench<E: Engine>(files: &[String]) {
    let mut src = String::new();
    for file in files {
        src.push_str(&std::fs::read_to_string(file).expect(file));
        src.push('\n');
    }
    ECHO.store(true, Ordering::Relaxed);
    let start = Instant::now();
    let mut engine = E::new();
    let result = engine
        .script("bench", &src)
        .and_then(|()| engine.run_jobs());
    if let Err(thrown) = result {
        println!("bench threw {}: {}", thrown.name, thrown.message);
    }
    println!("total_ms {} mem {}", start.elapsed().as_millis(), memory());
}

/// One manifest line: `path mode async neg_type includes`, tab-separated;
/// `-` for none.
struct Test<'a> {
    path: &'a str,
    mode: &'a str,
    is_async: bool,
    negative: Option<&'a str>,
    includes: Vec<&'a str>,
}

fn parse(line: &str) -> Test<'_> {
    let f: Vec<&str> = line.split('\t').collect();
    Test {
        path: f[0],
        mode: f[1],
        is_async: f[2] == "1",
        negative: (f[3] != "-").then_some(f[3]),
        includes: if f[4] == "-" {
            Vec::new()
        } else {
            f[4].split(',').collect()
        },
    }
}

fn t262<E: Engine>(root: &str, manifest: &str, start: usize) {
    let text = std::fs::read_to_string(manifest).expect("manifest");
    let mut harness: HashMap<String, String> = HashMap::new();
    let mut out = std::io::stdout().lock();
    for (i, line) in text.lines().enumerate().skip(start) {
        let _ = writeln!(out, "S {i}");
        let _ = out.flush();
        let test = parse(line);
        let verdict = run_test::<E>(root, &mut harness, &test);
        let _ = match verdict {
            Ok(()) => writeln!(out, "R {i} pass"),
            Err(why) => {
                let mut why: String = why
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                if why.len() > 200 {
                    why.truncate(why.floor_char_boundary(200));
                }
                writeln!(out, "R {i} fail {why}")
            }
        };
        let _ = out.flush();
    }
    let _ = writeln!(out, "D mem {}", memory());
}

fn run_test<E: Engine>(
    root: &str,
    harness: &mut HashMap<String, String>,
    test: &Test<'_>,
) -> Result<(), String> {
    let full = format!("{root}/{}", test.path);
    let src = std::fs::read_to_string(&full).map_err(|e| format!("cannot read: {e}"))?;
    let mut engine = E::new();
    take_printed();
    if test.mode != "raw" {
        let mut names = vec!["assert.js", "sta.js"];
        if test.is_async {
            names.push("doneprintHandle.js");
        }
        names.extend(test.includes.iter().copied());
        for name in names {
            if !harness.contains_key(name) {
                let text = std::fs::read_to_string(format!("{root}/harness/{name}"))
                    .map_err(|e| format!("harness {name}: {e}"))?;
                harness.insert(name.to_string(), text);
            }
            engine
                .script(name, &harness[name])
                .map_err(|t| format!("harness {name} threw {}: {}", t.name, t.message))?;
        }
    }
    let result = match test.mode {
        "module" => engine.module(&full, &src),
        "strict" => engine.script(test.path, &format!("\"use strict\";\n{src}")),
        _ => engine.script(test.path, &src),
    }
    .and_then(|()| engine.run_jobs());
    match (test.negative, result) {
        (Some(want), Err(t)) if t.name == want => Ok(()),
        (Some(want), Err(t)) => Err(format!("threw {}: {} (want {want})", t.name, t.message)),
        (Some(want), Ok(())) => Err(format!("no {want} thrown")),
        (None, Err(t)) => Err(format!("threw {}: {}", t.name, t.message)),
        (None, Ok(())) if test.is_async => {
            let printed = take_printed();
            if printed.iter().any(|l| l == "Test262:AsyncTestComplete") {
                Ok(())
            } else if let Some(l) = printed
                .iter()
                .find(|l| l.starts_with("Test262:AsyncTestFailure"))
            {
                Err(l.clone())
            } else {
                Err(String::from("the async test never finished"))
            }
        }
        (None, Ok(())) => Ok(()),
    }
}
