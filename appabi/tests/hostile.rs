//! A hostile guest through `appabi::call`, and hostile WAVE text, by the P8
//! fuzz targets' bodies (`fuzz/appabi_harness.rs`), on the host: random linear
//! memory, random core results and random result pointers give a value or a
//! trap, never a panic, and the host never allocates much more than
//! `MAX_LIFT_UNITS` for one result. libFuzzer runs the same bodies on GitHub.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/appabi_harness.rs"]
mod harness;

use appabi::MAX_LIFT_UNITS;
use appabi::wave;
use harness::{HEADER, NUMBERS, WAVE_SAMPLES, shapes};
use hostile::XorShift;

const MEMORY: usize = 64 * 1024;

/// An `appabi_call` input for the type at `pick`: the results' kind and the
/// guest's numbers from `rng`, then memory filled from it.
fn call_input(rng: &mut XorShift, pick: usize) -> Vec<u8> {
    let mut input = vec![pick as u8, rng.next() as u8];
    for _ in 0..NUMBERS {
        input.extend_from_slice(&(rng.next() as u32).to_le_bytes());
    }
    assert_eq!(input.len(), HEADER);
    input.extend((0..MEMORY).map(|_| rng.next() as u8));
    input
}

#[test]
fn hostile_results_are_values_or_traps_within_the_budget() {
    let (mut lifted, mut traps) = (0, 0);
    for seed in 1..=400 {
        let mut rng = XorShift::new(seed);
        for pick in 0..shapes().len() {
            let input = call_input(&mut rng, pick);
            let (value, peak) = hostile::peak_alloc(|| harness::appabi_call(&input));
            if value {
                lifted += 1;
            } else {
                traps += 1;
            }
            // The input's own copy of memory is the guest's.
            assert!(
                peak <= 2 * MAX_LIFT_UNITS as usize + MEMORY,
                "seed {seed}: {peak} bytes for {:?}",
                shapes()[pick]
            );
        }
    }
    // Random bytes should mostly be refused; the point is that nothing
    // panicked, and that some calls get far enough to lift a value.
    assert!(traps > 0 && lifted > 0, "{lifted} lifted, {traps} trapped");
}

#[test]
fn cut_call_inputs_return() {
    let mut rng = XorShift::new(7);
    for pick in 0..shapes().len() {
        let input = call_input(&mut rng, pick);
        hostile::truncations(&input[..HEADER + 256], |_, cut| {
            harness::appabi_call(cut);
        });
    }
}

/// WAVE text cut and mutated is a value or an error, never a panic, and whatever
/// it reads prints to text that reads and prints the same.
#[test]
fn hostile_wave_text_is_read_or_refused() {
    for (sample, seed) in WAVE_SAMPLES.into_iter().zip(21u64..) {
        for pick in 0..shapes().len() {
            let input = harness::wave_input(pick, sample);
            hostile::fuzz(&input[1..], seed, 1_000, |text| {
                let mut input = vec![pick as u8];
                input.extend_from_slice(text);
                harness::appabi_wave(&input);
            });
        }
    }
}

/// Every seed is read: each WAVE seed as its type, and some call seeds lift.
#[test]
fn the_seeds_read() {
    let shapes = shapes();
    let mut lifted = 0;
    for (target, input) in harness::seeds() {
        match target {
            "appabi_wave" => {
                let ty = &shapes[input[0] as usize];
                let text = core::str::from_utf8(&input[1..]).unwrap();
                assert!(wave::parse(text, ty).is_ok(), "{text} as {ty:?}");
                harness::appabi_wave(&input);
            }
            _ => lifted += usize::from(harness::appabi_call(&input)),
        }
    }
    assert!(lifted > 0);
}

/// Writes [`harness::seeds`] to `$FUZZ_SEED_DIR/<target>/`, for
/// `.github/workflows/fuzz.yml` to run before libFuzzer.
#[test]
#[ignore = "writes the fuzz corpus seeds; fuzz.yml runs it"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    for (n, (target, bytes)) in harness::seeds().into_iter().enumerate() {
        let folder = std::path::Path::new(&dir).join(target);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("seed-{n}")), bytes).unwrap();
    }
}
