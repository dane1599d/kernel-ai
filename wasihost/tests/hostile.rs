//! Hostile guests: every call with random and extreme arguments over random
//! memory returns an errno or a stop, never panics, and allocates boundedly.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::{INTERESTING, XorShift};
use wasihost::{IMPORTS, Limits, Stop, Tree, Wasi};

const MEMORY: usize = 1 << 16;

fn fresh() -> Wasi {
    let mut tree = Tree::default();
    tree.add_kai("include/kai.h", b"int x;\n".to_vec()).unwrap();
    tree.put_scratch("entry.c", b"int main(void) { return 0; }\n")
        .unwrap();
    Wasi::new(tree, &["wcc", "-c", "/w/entry.c"], Limits::default()).unwrap()
}

/// An argument: small pointers into memory, small fds, or an extreme value.
fn arg(rng: &mut XorShift) -> u64 {
    match rng.below(4) {
        0 => rng.below(MEMORY + 64) as u64,
        1 => rng.below(10) as u64,
        2 => INTERESTING[rng.below(INTERESTING.len())],
        _ => rng.next(),
    }
}

#[test]
fn random_calls_never_panic() {
    let mut rng = XorShift::new(0x5112);
    let mut memory = vec![0u8; MEMORY];
    for round in 0..200 {
        let mut wasi = fresh();
        for byte in memory.iter_mut() {
            *byte = rng.next() as u8;
        }
        // Paths the tree knows, at fixed places, so some calls get further.
        memory[..9].copy_from_slice(b"entry.c\0\0");
        for _ in 0..500 {
            let import = &IMPORTS[rng.below(IMPORTS.len())];
            let args: Vec<u64> = import.params.iter().map(|_| arg(&mut rng)).collect();
            match wasi.call(import.name, &mut memory, &args) {
                Ok(_) => {}
                Err(Stop::Exit(_)) => assert_eq!(import.name, "proc_exit"),
                Err(Stop::BadCall) => panic!("{} refused its own arity", import.name),
            }
        }
        assert!(
            wasi.stdout().len() <= Limits::default().output_bytes,
            "round {round}"
        );
        assert!(wasi.tree().scratch_used() <= wasihost::TreeLimits::default().scratch_bytes);
    }
}

#[test]
fn every_call_with_every_extreme_argument() {
    let mut memory = vec![0u8; 256];
    for import in IMPORTS {
        for value in INTERESTING {
            for position in 0..import.params.len() {
                let mut wasi = fresh();
                let mut args = vec![16u64; import.params.len()];
                args[position] = value;
                match wasi.call(import.name, &mut memory, &args) {
                    Ok(_) | Err(Stop::Exit(_)) => {}
                    Err(Stop::BadCall) => panic!("{}", import.name),
                }
            }
        }
    }
}
