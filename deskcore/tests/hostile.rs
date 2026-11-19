//! Hostile input to every reader `desktop` runs on another process's bytes: a
//! client's `kai-surface 1` calls through the desk, and the text wires of the
//! shell and the apps (settings, models, effects, network, panel, files) and
//! the agent's `ui ...` requests, cut at every byte, with every field set to
//! extreme values and mutated thousands of times. Each returns: none panics or
//! loops. The bodies are `fuzz/desk_harness.rs`, also libFuzzer's targets.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/desk_harness.rs"]
mod harness;

type Body = fn(&[u8]);

/// Each target's name, body and sample.
fn targets() -> Vec<(&'static str, Body, Vec<u8>)> {
    vec![
        ("desk_server", harness::desk_server, harness::calls()),
        ("desk_queues", harness::desk_queues, harness::queues()),
        ("desk_parsers", harness::desk_parsers, harness::texts()),
        ("desk_agent", harness::desk_agent, harness::requests()),
    ]
}

#[test]
fn every_target_takes_its_sample_at_every_prefix() {
    for (name, body, sample) in targets() {
        body(&sample);
        hostile::truncations(&sample, |_, prefix| body(prefix));
        println!("{name}: {} bytes", sample.len());
    }
}

#[test]
fn every_target_takes_every_field_and_random_mutations() {
    for (name, body, sample) in targets() {
        // The first 600 bytes' fields: a field of a sample can be anything.
        let to = sample.len().min(600);
        hostile::sweep_fields(&sample, 0..to, |_, mutated| body(mutated));
        for seed in [11, 0x2545_f491_4f6c_dd1d] {
            hostile::fuzz(&sample, seed ^ name.len() as u64, 3000, body);
        }
    }
}

#[test]
fn the_seeds_are_the_samples() {
    let seeds = harness::seeds();
    assert_eq!(seeds.len(), targets().len());
    for ((name, _, sample), (target, bytes)) in targets().into_iter().zip(seeds) {
        assert_eq!((name, &sample), (target, &bytes));
    }
}

/// Writes each fuzz target's seeds, the samples above, under `FUZZ_SEED_DIR`
/// (`.github/workflows/fuzz.yml` runs it before libFuzzer).
#[test]
#[ignore = "writes the fuzz corpus seeds; fuzz.yml runs it"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    for (target, bytes) in harness::seeds() {
        let folder = std::path::Path::new(&dir).join(target);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("seed-0"), bytes).unwrap();
    }
}
