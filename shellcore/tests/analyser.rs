//! A hostile script for the analyser: cut, mutated or bent at every byte, whatever
//! still compiles is walked without a panic, the plan is no larger than the text,
//! and the walk's memory stays bounded.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use rhai::Engine;
use shellcore::analyse::analyse;

const SAMPLE: &str = "let p = get_policy();\nlet n = 0;\nfor x in processes() {\n    if x.pid > 2 && p == \"interactive\" { n += 1; }\n}\ntry { print(`${cpus()} ${memory().free_mib}`); } catch { }\nswitch n { 0 => set_policy(\"rr\"), _ => \"interactive\".set_policy() }\nif uptime_ms() > 5 { return; }\nset_policy([p, \"rr\"][n % 2]);\n";

fn walk(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let mut engine = Engine::new_raw();
    engine.set_max_expr_depths(32, 16);
    let Ok(ast) = engine.compile(text) else {
        return;
    };
    let (plan, peak) = hostile::peak_alloc(|| analyse(&ast, &agentabi::SCRIPT_API));
    assert!(plan.sites.len() <= text.len());
    assert!(
        peak <= 64 * 1024 + 64 * text.len(),
        "{peak} bytes for {} bytes",
        text.len()
    );
}

#[test]
fn the_sample_has_its_calls() {
    let ast = Engine::new_raw().compile(SAMPLE).unwrap();
    let names = analyse(&ast, &agentabi::SCRIPT_API).names();
    assert_eq!(
        names,
        [
            "get_policy",
            "processes",
            "cpus",
            "memory",
            "set_policy",
            "uptime_ms"
        ]
    );
}

#[test]
fn mutated_scripts_are_walked_or_refused() {
    hostile::fuzz(SAMPLE.as_bytes(), 8, 20_000, walk);
    hostile::truncations(SAMPLE.as_bytes(), |_, cut| walk(cut));
}
