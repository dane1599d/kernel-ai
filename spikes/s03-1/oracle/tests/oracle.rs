//! S03-1's first two measures: wit-component wraps the unmodified module, and
//! 10,000 generated calls give the same results on wasmi through `appabi` as on
//! Wasmtime 50's component runtime. Pass: 0 differences.

use appabi_spike::{Ty, Val};
use s03_1_oracle::{WasmiGuest, WasmtimeGuest, build_guest, probe_funcs, wrap};
use wasmtime::component::{ResourceAny, Val as WVal};

const CALLS: usize = 10_000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const CHARS: &[char] = &[
    'a',
    'Z',
    '0',
    ' ',
    'é',
    'ß',
    '中',
    '\u{1F600}',
    '\0',
    '"',
    '\u{10FFFF}',
];

fn text(r: &mut Rng) -> String {
    (0..r.below(12))
        .map(|_| CHARS[r.below(CHARS.len())])
        .collect()
}

fn float(r: &mut Rng) -> f64 {
    match r.below(8) {
        0 => f64::NAN,
        1 => f64::INFINITY,
        2 => -0.0,
        3 => f64::MIN_POSITIVE,
        _ => (r.next() as i64 as f64) / 1e6,
    }
}

/// A random value of `ty`. Resource arguments name an entry of the pool.
fn arbitrary(r: &mut Rng, ty: &Ty, pool: usize, depth: u32) -> Option<Val> {
    let small = |r: &mut Rng| r.below(if depth > 1 { 2 } else { 5 });
    Some(match ty {
        Ty::Bool => Val::Bool(r.next() & 1 == 1),
        Ty::U8 => Val::U8(r.next() as u8),
        Ty::U16 => Val::U16(r.next() as u16),
        Ty::U32 => Val::U32(r.next() as u32),
        Ty::U64 => Val::U64(r.next()),
        Ty::S8 => Val::S8(r.next() as i8),
        Ty::S16 => Val::S16(r.next() as i16),
        Ty::S32 => Val::S32(r.next() as i32),
        Ty::S64 => Val::S64(r.next() as i64),
        Ty::F32 => Val::F32(float(r) as f32),
        Ty::F64 => Val::F64(float(r)),
        Ty::Char => Val::Char(CHARS[r.below(CHARS.len())]),
        Ty::String => Val::String(text(r)),
        Ty::List(t) => Val::List(
            (0..small(r))
                .map(|_| arbitrary(r, t, pool, depth + 1))
                .collect::<Option<_>>()?,
        ),
        Ty::Record(fields) => Val::Record(
            fields
                .iter()
                .map(|(_, t)| arbitrary(r, t, pool, depth + 1))
                .collect::<Option<_>>()?,
        ),
        Ty::Tuple(types) => Val::Tuple(
            types
                .iter()
                .map(|t| arbitrary(r, t, pool, depth + 1))
                .collect::<Option<_>>()?,
        ),
        Ty::Variant(cases) => {
            let case = r.below(cases.len());
            let payload = match &cases[case].1 {
                Some(t) => Some(Box::new(arbitrary(r, t, pool, depth + 1)?)),
                None => None,
            };
            Val::Variant(case as u32, payload)
        }
        Ty::Enum(cases) => Val::Enum(r.below(cases.len()) as u32),
        Ty::Option(t) => Val::Option(match r.below(2) {
            0 => None,
            _ => Some(Box::new(arbitrary(r, t, pool, depth + 1)?)),
        }),
        Ty::Result(ok, err) => {
            let side = if r.below(2) == 0 { ok } else { err };
            let payload = match side {
                Some(t) => Some(Box::new(arbitrary(r, t, pool, depth + 1)?)),
                None => None,
            };
            Val::Result(if std::ptr::eq(side, ok) {
                Ok(payload)
            } else {
                Err(payload)
            })
        }
        Ty::Flags(names) => Val::Flags(
            (0..names.len() as u32)
                .filter(|_| r.next() & 1 == 1)
                .collect(),
        ),
        Ty::Own | Ty::Borrow if pool == 0 => return None,
        Ty::Own => Val::Own(r.below(pool) as u32),
        Ty::Borrow => Val::Borrow(r.below(pool) as u32),
    })
}

/// The pool: each live counter as its wasmi handle and its Wasmtime resource.
type Pool = Vec<(u32, ResourceAny)>;

/// `v` with pool indices as wasmi handles.
fn for_wasmi(v: &Val, pool: &Pool) -> Val {
    match v {
        Val::Own(i) => Val::Own(pool[*i as usize].0),
        Val::Borrow(i) => Val::Borrow(pool[*i as usize].0),
        other => other.clone(),
    }
}

fn to_wasmtime(v: &Val, ty: &Ty, pool: &Pool) -> WVal {
    let sub = |v: &Val, t: &Ty| to_wasmtime(v, t, pool);
    match (v, ty) {
        (Val::Bool(b), _) => WVal::Bool(*b),
        (Val::U8(n), _) => WVal::U8(*n),
        (Val::U16(n), _) => WVal::U16(*n),
        (Val::U32(n), _) => WVal::U32(*n),
        (Val::U64(n), _) => WVal::U64(*n),
        (Val::S8(n), _) => WVal::S8(*n),
        (Val::S16(n), _) => WVal::S16(*n),
        (Val::S32(n), _) => WVal::S32(*n),
        (Val::S64(n), _) => WVal::S64(*n),
        (Val::F32(n), _) => WVal::Float32(*n),
        (Val::F64(n), _) => WVal::Float64(*n),
        (Val::Char(c), _) => WVal::Char(*c),
        (Val::String(s), _) => WVal::String(s.clone()),
        (Val::List(items), Ty::List(t)) => WVal::List(items.iter().map(|i| sub(i, t)).collect()),
        (Val::Record(vals), Ty::Record(fields)) => WVal::Record(
            vals.iter()
                .zip(fields)
                .map(|(v, (n, t))| (n.clone(), sub(v, t)))
                .collect(),
        ),
        (Val::Tuple(vals), Ty::Tuple(types)) => {
            WVal::Tuple(vals.iter().zip(types).map(|(v, t)| sub(v, t)).collect())
        }
        (Val::Variant(case, payload), Ty::Variant(cases)) => {
            let (name, t) = &cases[*case as usize];
            let payload = payload
                .as_ref()
                .zip(t.as_ref())
                .map(|(v, t)| Box::new(sub(v, t)));
            WVal::Variant(name.clone(), payload)
        }
        (Val::Enum(i), Ty::Enum(cases)) => WVal::Enum(cases[*i as usize].clone()),
        (Val::Option(o), Ty::Option(t)) => WVal::Option(o.as_ref().map(|v| Box::new(sub(v, t)))),
        (Val::Result(res), Ty::Result(ok, err)) => WVal::Result(match res {
            Ok(v) => Ok(v
                .as_ref()
                .zip(ok.as_ref())
                .map(|(v, t)| Box::new(sub(v, t)))),
            Err(v) => Err(v
                .as_ref()
                .zip(err.as_ref())
                .map(|(v, t)| Box::new(sub(v, t)))),
        }),
        (Val::Flags(set), Ty::Flags(names)) => {
            WVal::Flags(set.iter().map(|i| names[*i as usize].clone()).collect())
        }
        (Val::Own(i) | Val::Borrow(i), _) => WVal::Resource(pool[*i as usize].1),
        _ => panic!("{v:?} is not a {ty:?}"),
    }
}

/// A Wasmtime result as an `appabi` value; resources become `Own(0)` and are
/// returned beside it.
fn from_wasmtime(v: &WVal, ty: &Ty, new: &mut Vec<ResourceAny>) -> Val {
    let index = |names: &[String], name: &str| names.iter().position(|n| n == name).unwrap() as u32;
    match (v, ty) {
        (WVal::Bool(b), _) => Val::Bool(*b),
        (WVal::U8(n), _) => Val::U8(*n),
        (WVal::U16(n), _) => Val::U16(*n),
        (WVal::U32(n), _) => Val::U32(*n),
        (WVal::U64(n), _) => Val::U64(*n),
        (WVal::S8(n), _) => Val::S8(*n),
        (WVal::S16(n), _) => Val::S16(*n),
        (WVal::S32(n), _) => Val::S32(*n),
        (WVal::S64(n), _) => Val::S64(*n),
        (WVal::Float32(n), _) => Val::F32(*n),
        (WVal::Float64(n), _) => Val::F64(*n),
        (WVal::Char(c), _) => Val::Char(*c),
        (WVal::String(s), _) => Val::String(s.clone()),
        (WVal::List(items), Ty::List(t)) => {
            Val::List(items.iter().map(|i| from_wasmtime(i, t, new)).collect())
        }
        (WVal::Record(fields), Ty::Record(types)) => Val::Record(
            fields
                .iter()
                .zip(types)
                .map(|((_, v), (_, t))| from_wasmtime(v, t, new))
                .collect(),
        ),
        (WVal::Tuple(vals), Ty::Tuple(types)) => Val::Tuple(
            vals.iter()
                .zip(types)
                .map(|(v, t)| from_wasmtime(v, t, new))
                .collect(),
        ),
        (WVal::Variant(name, payload), Ty::Variant(cases)) => {
            let case = cases.iter().position(|(n, _)| n == name).unwrap();
            let payload = payload
                .as_ref()
                .zip(cases[case].1.as_ref())
                .map(|(v, t)| Box::new(from_wasmtime(v, t, new)));
            Val::Variant(case as u32, payload)
        }
        (WVal::Enum(name), Ty::Enum(cases)) => Val::Enum(index(cases, name)),
        (WVal::Option(o), Ty::Option(t)) => {
            Val::Option(o.as_ref().map(|v| Box::new(from_wasmtime(v, t, new))))
        }
        (WVal::Result(res), Ty::Result(ok, err)) => Val::Result(match res {
            Ok(v) => Ok(v
                .as_ref()
                .zip(ok.as_ref())
                .map(|(v, t)| Box::new(from_wasmtime(v, t, new)))),
            Err(v) => Err(v
                .as_ref()
                .zip(err.as_ref())
                .map(|(v, t)| Box::new(from_wasmtime(v, t, new)))),
        }),
        (WVal::Flags(set), Ty::Flags(names)) => {
            let mut bits: Vec<u32> = set.iter().map(|n| index(names, n)).collect();
            bits.sort_unstable();
            Val::Flags(bits)
        }
        (WVal::Resource(r), _) => {
            new.push(*r);
            Val::Own(0)
        }
        _ => panic!("{v:?} is not a {ty:?}"),
    }
}

/// Values compared the way WAVE text would print them: every NaN is one NaN.
fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::F32(x), Val::F32(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        (Val::F64(x), Val::F64(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        (Val::List(x), Val::List(y))
        | (Val::Record(x), Val::Record(y))
        | (Val::Tuple(x), Val::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Val::Variant(c, x), Val::Variant(d, y)) => c == d && same_opt(x, y),
        (Val::Option(x), Val::Option(y)) => same_opt(x, y),
        (Val::Result(Ok(x)), Val::Result(Ok(y))) | (Val::Result(Err(x)), Val::Result(Err(y))) => {
            same_opt(x, y)
        }
        // A new resource's handle differs by engine; what it holds is compared later.
        (Val::Own(_), Val::Own(_)) => true,
        _ => a == b,
    }
}

fn same_opt(a: &Option<Box<Val>>, b: &Option<Box<Val>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same(a, b),
        (None, None) => true,
        _ => false,
    }
}

#[test]
fn wit_component_wraps_the_unmodified_module() {
    let wasm = build_guest("guest", "probe_guest");
    let component = wrap(&wasm).expect("wit-component wraps the module as it is");
    assert!(component.len() > wasm.len());
    println!(
        "S03-1: guest module {} bytes, wrapped component {} bytes",
        wasm.len(),
        component.len()
    );
}

#[test]
fn ten_thousand_calls_give_the_same_results_on_both_engines() {
    let wasm = build_guest("guest", "probe_guest");
    let mut wasmi = WasmiGuest::new(&wasm);
    let mut wasmtime = WasmtimeGuest::new(&wrap(&wasm).unwrap());
    let funcs = probe_funcs();
    assert_eq!(
        funcs.len(),
        28,
        "the probe's functions, methods and constructor"
    );
    let mut rng = Rng(0x5eed_0301);
    let mut pool: Pool = Vec::new();
    let (mut calls, mut differences, mut failed) = (0, Vec::new(), 0);
    let mut per_func = std::collections::BTreeMap::<&str, u32>::new();
    while calls < CALLS {
        let (name, func) = &funcs[rng.below(funcs.len())];
        let Some(args) = func
            .params
            .iter()
            .map(|t| arbitrary(&mut rng, t, pool.len(), 0))
            .collect::<Option<Vec<Val>>>()
        else {
            continue;
        };
        let wasmi_args: Vec<Val> = args.iter().map(|a| for_wasmi(a, &pool)).collect();
        let wasmtime_args: Vec<WVal> = args
            .iter()
            .zip(&func.params)
            .map(|(a, t)| to_wasmtime(a, t, &pool))
            .collect();
        let left = wasmi.invoke(func, &wasmi_args);
        let right = wasmtime.invoke(name, &wasmtime_args);
        calls += 1;
        *per_func.entry(name.as_str()).or_default() += 1;
        // An owned argument moved into the guest: it leaves the pool.
        let mut moved: Vec<usize> = args
            .iter()
            .filter_map(|a| match a {
                Val::Own(i) => Some(*i as usize),
                _ => None,
            })
            .collect();
        moved.sort_unstable();
        moved.dedup();
        for i in moved.into_iter().rev() {
            pool.remove(i);
        }
        let mut new = Vec::new();
        let right = right.map(|r| {
            r.zip(func.result.as_ref())
                .map(|(v, t)| from_wasmtime(&v, t, &mut new))
        });
        match (&left, &right) {
            (Ok(Some(l)), Ok(Some(r))) if same(l, r) => {
                if let (Val::Own(h), Some(res)) = (l, new.pop()) {
                    pool.push((*h, res));
                }
            }
            (Ok(None), Ok(None)) => {}
            (Err(_), Err(_)) => failed += 1,
            _ => differences.push(format!(
                "{name}({args:?}): wasmi {left:?}, wasmtime {right:?}"
            )),
        }
        // Keep the pool small, dropping the oldest counter on both sides.
        if pool.len() > 8 {
            let (h, res) = pool.remove(0);
            wasmi.drop_handle(h).unwrap();
            wasmtime.drop_handle(res).unwrap();
        }
    }
    println!(
        "S03-1: {calls} calls, {} differences, {failed} failed on both; per function: {per_func:?}",
        differences.len()
    );
    assert_eq!(failed, 0, "every probe call should succeed");
    assert!(
        differences.is_empty(),
        "{} differences, first ones:\n{}",
        differences.len(),
        differences[..differences.len().min(10)].join("\n")
    );
}
