//! A hostile guest: random linear memory, random core results and random result
//! pointers, through `appabi::call` for results of every shape. Every call returns
//! a value or a trap, never panics, and the host never allocates much more than
//! `MAX_LIFT_UNITS` for one result.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use appabi::{Core, Func, Guest, MAX_LIFT_UNITS, Trap, Ty, call};
use hostile::XorShift;

const MEMORY: usize = 64 * 1024;

/// Memory filled from the generator, and exports that answer with its numbers.
struct Hostile {
    memory: Vec<u8>,
    rng: XorShift,
}

impl Guest for Hostile {
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap> {
        let end = (at as usize).checked_add(len as usize);
        end.and_then(|end| self.memory.get(at as usize..end))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Trap("outside memory".into()))
    }

    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Trap> {
        let end = (at as usize).checked_add(bytes.len());
        end.and_then(|end| self.memory.get_mut(at as usize..end))
            .ok_or_else(|| Trap("outside memory".into()))?
            .copy_from_slice(bytes);
        Ok(())
    }

    fn realloc(&mut self, _: u32, _: u32, _: u32, _: u32) -> Result<u32, Trap> {
        // A hostile realloc: anywhere, aligned or not, even past the end.
        Ok(self.number())
    }

    fn call(&mut self, export: &str, _: &[Core]) -> Result<Vec<Core>, Trap> {
        if export.starts_with("cabi_post_") {
            return Ok(Vec::new());
        }
        Ok(match self.rng.below(4) {
            0 => vec![Core::I32(self.number() as i32)],
            1 => vec![Core::I64(self.rng.next() as i64)],
            2 => vec![
                Core::I32(self.number() as i32),
                Core::I32(self.number() as i32),
            ],
            _ => (0..self.rng.below(6))
                .map(|_| Core::I32(self.number() as i32))
                .collect(),
        })
    }

    fn rep(&mut self, h: u32) -> Result<u32, Trap> {
        Ok(h)
    }
}

impl Hostile {
    fn new(seed: u64) -> Self {
        let mut rng = XorShift::new(seed);
        let memory = (0..MEMORY).map(|_| rng.next() as u8).collect();
        Self { memory, rng }
    }

    /// Mostly addresses and lengths inside memory, sometimes anything at all.
    fn number(&mut self) -> u32 {
        match self.rng.below(4) {
            0 => self.rng.next() as u32,
            1 => self.rng.below(16) as u32,
            _ => self.rng.below(MEMORY) as u32 & !3,
        }
    }
}

fn shapes() -> Vec<Ty> {
    let string = || Ty::String;
    let point = Ty::Record(vec![("x".into(), Ty::S32), ("y".into(), Ty::S32)]);
    vec![
        Ty::Bool,
        Ty::U64,
        Ty::F64,
        Ty::Char,
        string(),
        Ty::List(Box::new(Ty::U8)),
        Ty::List(Box::new(string())),
        Ty::List(Box::new(Ty::List(Box::new(Ty::U32)))),
        Ty::List(Box::new(Ty::Tuple(vec![]))),
        Ty::Option(Box::new(string())),
        Ty::Result(Some(Box::new(Ty::U32)), Some(Box::new(string()))),
        Ty::Tuple(vec![Ty::U8, string(), Ty::S64]),
        Ty::Variant(vec![
            ("none".into(), None),
            ("real".into(), Some(Ty::F64)),
            ("text".into(), Some(string())),
            ("pt".into(), Some(point.clone())),
        ]),
        Ty::Enum(vec!["a".into(), "b".into()]),
        Ty::Flags((0..12).map(|i| format!("f{i}")).collect()),
        Ty::List(Box::new(Ty::Record(vec![
            ("name".into(), string()),
            ("corners".into(), Ty::List(Box::new(point))),
            ("tag".into(), Ty::Option(Box::new(string()))),
        ]))),
    ]
}

#[test]
fn hostile_results_are_values_or_traps_within_the_budget() {
    let mut traps = 0;
    for seed in 1..=400 {
        let mut guest = Hostile::new(seed);
        for result in shapes() {
            let func = Func {
                export: "api#f".into(),
                params: vec![Ty::String, Ty::List(Box::new(Ty::U32))],
                result: Some(result),
            };
            let args = [
                appabi::Val::String("hostile".into()),
                appabi::Val::List(vec![appabi::Val::U32(1); 3]),
            ];
            let (outcome, peak) = hostile::peak_alloc(|| call(&mut guest, &func, &args));
            traps += usize::from(outcome.is_err());
            assert!(
                peak <= 2 * MAX_LIFT_UNITS as usize + MEMORY,
                "seed {seed}: {peak} bytes for {:?}",
                func.result
            );
        }
    }
    // Random bytes should mostly be refused; the point is that nothing panicked.
    assert!(traps > 0);
}
