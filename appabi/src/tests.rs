extern crate std;

use super::*;
use crate::layout::flatten;
use crate::lift::{Lift, lift, load};
use crate::lower::{lower, store};
use crate::{Flat, Guest};
use alloc::vec;

/// A guest with 1 MiB of memory, a bump `cabi_realloc` and no exports.
struct Fake {
    memory: Vec<u8>,
    next: u32,
}

impl Fake {
    fn new() -> Self {
        Self {
            memory: vec![0; 1 << 20],
            next: 16,
        }
    }
}

impl Guest for Fake {
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap> {
        let end = (at as usize).checked_add(len as usize);
        end.and_then(|end| self.memory.get(at as usize..end))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| trap("outside memory"))
    }

    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Trap> {
        let end = at as usize + bytes.len();
        self.memory
            .get_mut(at as usize..end)
            .ok_or_else(|| trap("outside memory"))?
            .copy_from_slice(bytes);
        Ok(())
    }

    fn realloc(&mut self, _: u32, _: u32, align: u32, size: u32) -> Result<u32, Trap> {
        let at = self.next.div_ceil(align) * align;
        self.next = at + size;
        Ok(at)
    }

    fn call(&mut self, export: &str, _: &[Core]) -> Result<Vec<Core>, Trap> {
        Err(trap(format!("no export {export}")))
    }

    fn rep(&mut self, h: u32) -> Result<u32, Trap> {
        Ok(h)
    }
}

#[test]
fn sizes_and_alignments_follow_the_spec() {
    let point = Ty::Record(vec![("x".into(), Ty::S32), ("y".into(), Ty::S32)]);
    assert_eq!((size(&point), align(&point)), (8, 4));
    let tuple = Ty::Tuple(vec![Ty::U8, Ty::String, Ty::S64]);
    assert_eq!((size(&tuple), align(&tuple)), (24, 8));
    let opt = Ty::Option(Box::new(Ty::String));
    assert_eq!((size(&opt), align(&opt)), (12, 4));
    let res = Ty::Result(Some(Box::new(Ty::U32)), Some(Box::new(Ty::String)));
    assert_eq!(flatten(&res), vec![Flat::I32, Flat::I32, Flat::I32]);
    let mixed = Ty::Variant(vec![
        ("a".into(), Some(Ty::F64)),
        ("b".into(), Some(Ty::U32)),
        ("c".into(), None),
    ]);
    assert_eq!(flatten(&mixed), vec![Flat::I32, Flat::I64]);
    assert_eq!(
        size(&Ty::Flags((0..9).map(|i| format!("f{i}")).collect())),
        2
    );
}

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

/// A random type, at most `depth` levels deep.
fn any_ty(r: &mut Rng, depth: u32) -> Ty {
    let leaves = [
        Ty::Bool,
        Ty::U8,
        Ty::U16,
        Ty::U32,
        Ty::U64,
        Ty::S8,
        Ty::S16,
        Ty::S32,
        Ty::S64,
        Ty::F32,
        Ty::F64,
        Ty::Char,
        Ty::String,
        Ty::Own,
        Ty::Borrow,
        Ty::Enum(vec!["a".into(), "b".into(), "c".into()]),
        Ty::Flags((0..r.below(20)).map(|i| format!("f{i}")).collect()),
    ];
    if depth == 0 || r.below(3) == 0 {
        return leaves[r.below(leaves.len())].clone();
    }
    let sub = |r: &mut Rng| any_ty(r, depth - 1);
    match r.below(6) {
        0 => Ty::List(Box::new(sub(r))),
        1 => Ty::Record(
            (0..1 + r.below(3))
                .map(|i| (format!("f{i}"), sub(r)))
                .collect(),
        ),
        2 => Ty::Tuple((0..1 + r.below(3)).map(|_| sub(r)).collect()),
        3 => Ty::Variant(
            (0..1 + r.below(4))
                .map(|i| (format!("c{i}"), (r.below(2) == 0).then(|| sub(r))))
                .collect(),
        ),
        4 => Ty::Option(Box::new(sub(r))),
        _ => Ty::Result(
            (r.below(2) == 0).then(|| Box::new(sub(r))),
            (r.below(2) == 0).then(|| Box::new(sub(r))),
        ),
    }
}

/// A random value of `ty`, in the form lifting gives back (`list<u8>` as bytes).
fn any_val(r: &mut Rng, ty: &Ty) -> Val {
    let chars = ['a', 'é', '中', '\u{1F600}', '\0'];
    match ty {
        Ty::Bool => Val::Bool(r.next() & 1 == 1),
        Ty::U8 => Val::U8(r.next() as u8),
        Ty::U16 => Val::U16(r.next() as u16),
        Ty::U32 => Val::U32(r.next() as u32),
        Ty::U64 => Val::U64(r.next()),
        Ty::S8 => Val::S8(r.next() as i8),
        Ty::S16 => Val::S16(r.next() as i16),
        Ty::S32 => Val::S32(r.next() as i32),
        Ty::S64 => Val::S64(r.next() as i64),
        Ty::F32 => Val::F32(r.next() as i32 as f32 / 7.0),
        Ty::F64 => Val::F64(r.next() as i64 as f64 / 7.0),
        Ty::Char => Val::Char(chars[r.below(chars.len())]),
        Ty::String => Val::String((0..r.below(6)).map(|_| chars[r.below(5)]).collect()),
        Ty::Own => Val::Own(r.next() as u32),
        Ty::Borrow => Val::Borrow(r.next() as u32),
        Ty::Enum(cases) => Val::Enum(r.below(cases.len()) as u32),
        Ty::Flags(names) => Val::Flags(
            (0..names.len() as u32)
                .filter(|_| r.next() & 1 == 1)
                .collect(),
        ),
        Ty::List(elem) if **elem == Ty::U8 => {
            Val::Bytes((0..r.below(5)).map(|_| r.next() as u8).collect())
        }
        Ty::List(elem) => Val::List((0..r.below(4)).map(|_| any_val(r, elem)).collect()),
        Ty::Record(fields) => Val::Record(fields.iter().map(|(_, t)| any_val(r, t)).collect()),
        Ty::Tuple(types) => Val::Tuple(types.iter().map(|t| any_val(r, t)).collect()),
        Ty::Variant(cases) => {
            let case = r.below(cases.len());
            let payload = cases[case].1.as_ref().map(|t| Box::new(any_val(r, t)));
            Val::Variant(case as u32, payload)
        }
        Ty::Option(t) => Val::Option((r.below(2) == 0).then(|| Box::new(any_val(r, t)))),
        Ty::Result(ok, err) => {
            let side = if r.below(2) == 0 { ok } else { err };
            let payload = side.as_ref().map(|t| Box::new(any_val(r, t)));
            Val::Result(if core::ptr::eq(side, ok) {
                Ok(payload)
            } else {
                Err(payload)
            })
        }
    }
}

#[test]
fn every_value_survives_lowering_and_lifting_flat_and_in_memory() {
    let mut r = Rng(0x0a99_ab10);
    for case in 0..3_000 {
        let ty = any_ty(&mut r, 3);
        let val = any_val(&mut r, &ty);
        let mut g = Fake::new();
        // Borrows lower to the representation, which this fake makes the handle.
        let mut core = Vec::new();
        lower(&mut g, &val, &ty, &mut core).unwrap();
        assert_eq!(core.len(), flatten(&ty).len(), "case {case}: {ty:?}");
        let flat = lift(&mut Lift::new(&mut g), &ty, &mut core.into_iter()).unwrap();
        assert_eq!(flat, val, "case {case}: {ty:?}");
        let at = g.realloc(0, 0, align(&ty), size(&ty)).unwrap();
        store(&mut g, &val, &ty, at).unwrap();
        let loaded = load(&mut Lift::new(&mut g), &ty, at).unwrap();
        assert_eq!(loaded, val, "case {case}: {ty:?}");
    }
}

#[test]
fn a_list_of_bytes_also_lowers_from_a_list_of_u8() {
    let ty = Ty::List(Box::new(Ty::U8));
    let mut g = Fake::new();
    let mut core = Vec::new();
    lower(
        &mut g,
        &Val::List(vec![Val::U8(7), Val::U8(9)]),
        &ty,
        &mut core,
    )
    .unwrap();
    let lifted = lift(&mut Lift::new(&mut g), &ty, &mut core.into_iter()).unwrap();
    assert_eq!(lifted, Val::Bytes(vec![7, 9]));
}

#[test]
fn aliased_or_empty_lists_cannot_blow_up_the_host() {
    let mut g = Fake::new();
    // 1,000 list headers in a row, every one pointing at the same 1,000 u32s.
    let inner_at = 4096u32;
    let outer_at = 8192u32;
    for i in 0..1_000u32 {
        let header = [inner_at.to_le_bytes(), 1_000u32.to_le_bytes()].concat();
        g.write(outer_at + i * 8, &header).unwrap();
    }
    let nested = Ty::List(Box::new(Ty::List(Box::new(Ty::U32))));
    let core = vec![Core::I32(outer_at as i32), Core::I32(1_000)];
    let error = lift(&mut Lift::new(&mut g), &nested, &mut core.into_iter()).unwrap_err();
    assert!(error.0.contains("too large"), "{error:?}");
    // Values that take no memory are still charged.
    let empty = Ty::List(Box::new(Ty::Tuple(vec![])));
    let core = vec![Core::I32(64), Core::I32(i32::MAX)];
    let error = lift(&mut Lift::new(&mut g), &empty, &mut core.into_iter()).unwrap_err();
    assert!(error.0.contains("too large"), "{error:?}");
}

#[test]
fn bad_guest_values_are_traps() {
    let mut g = Fake::new();
    let bad = |g: &mut Fake, ty: Ty, core: Vec<Core>| {
        lift(&mut Lift::new(g), &ty, &mut core.into_iter()).unwrap_err()
    };
    assert!(
        bad(&mut g, Ty::Char, vec![Core::I32(0xD800)])
            .0
            .contains("Unicode")
    );
    let colors = Ty::Enum(vec!["r".into(), "g".into()]);
    assert!(
        bad(&mut g, colors, vec![Core::I32(2)])
            .0
            .contains("out of range")
    );
    let flags = Ty::Flags(vec!["a".into(), "b".into()]);
    assert!(bad(&mut g, flags, vec![Core::I32(4)]).0.contains("beyond"));
    g.write(64, &[0xff, 0xfe]).unwrap();
    assert!(
        bad(&mut g, Ty::String, vec![Core::I32(64), Core::I32(2)])
            .0
            .contains("UTF-8")
    );
    let far = vec![Core::I32(-8), Core::I32(16)];
    assert!(bad(&mut g, Ty::String, far).0.contains("overflows"));
    let unaligned = vec![Core::I32(3), Core::I32(1)];
    assert!(
        bad(&mut g, Ty::List(Box::new(Ty::U32)), unaligned)
            .0
            .contains("misaligned")
    );
    assert!(
        bad(&mut g, Ty::U64, vec![Core::I32(1)])
            .0
            .contains("wrong type")
    );
}

#[test]
fn host_imports_lift_their_arguments_flat_or_by_pointer() {
    let mut g = Fake::new();
    let result = Ty::Result(Some(Box::new(Ty::U32)), Some(Box::new(Ty::U8)));
    // Two flat parameters and a result that comes back by pointer.
    let small = Func {
        export: String::new(),
        params: vec![Ty::String, Ty::U16],
        result: Some(result.clone()),
    };
    let args = vec![Val::String("hi".into()), Val::U16(7)];
    let mut core = Vec::new();
    for (v, t) in args.iter().zip(&small.params) {
        lower(&mut g, v, t, &mut core).unwrap();
    }
    core.push(Core::I32(512));
    assert_eq!(
        lift_args(&mut g, &small, &core),
        Ok((args.clone(), Some(512)))
    );
    let ok = Val::Result(Ok(Some(Box::new(Val::U32(9)))));
    store_result(&mut g, &ok, &result, 512).unwrap();
    assert_eq!(load(&mut Lift::new(&mut g), &result, 512), Ok(ok));
    // A missing result pointer, or one flat value too many, is a trap.
    assert!(lift_args(&mut g, &small, &core[..core.len() - 1]).is_err());
    let plain = Func {
        result: None,
        ..small.clone()
    };
    assert!(lift_args(&mut g, &plain, &core).is_err());
    // More than 16 flat parameters come as a pointer to their tuple.
    let many = Func {
        export: String::new(),
        params: vec![Ty::U32; 17],
        result: None,
    };
    let vals: Vec<Val> = (0..17).map(Val::U32).collect();
    let tuple = Ty::Tuple(many.params.clone());
    let at = g.realloc(0, 0, align(&tuple), size(&tuple)).unwrap();
    store(&mut g, &Val::Tuple(vals.clone()), &tuple, at).unwrap();
    assert_eq!(
        lift_args(&mut g, &many, &[Core::I32(at as i32)]),
        Ok((vals, None))
    );
}
