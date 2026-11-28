//! The P8 fuzz targets' bodies for `appabi` (ADR 0003's plan, P8: fuzzing of
//! `appabi` and WAVE parsing): [`appabi_call`] runs `appabi::call` against a
//! guest made of the input's bytes, and [`appabi_wave`] reads the input as
//! WAVE text. `appabi/tests/hostile.rs` includes this file and runs both over
//! hostile inputs, which also seed the targets' corpora; livecore's section
//! readers have theirs in `livecore_harness.rs`. Each must return for any
//! input; only a bug in the code under it may panic.

#![allow(dead_code)]

use appabi::wave::{self, MAX_TEXT};
use appabi::{Core, Func, Guest, Trap, Ty, Val, call};

/// The result types a call is lifted as, and WAVE text is read as: every kind
/// but handles, which have no text form.
pub fn shapes() -> Vec<Ty> {
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

/// The bytes before a call input's memory: the result type's index, the
/// results' kind, then [`NUMBERS`] little-endian `u32`s.
pub const HEADER: usize = 2 + 4 * NUMBERS;
/// The numbers the guest answers with, used in turn and then again.
pub const NUMBERS: usize = 8;

/// A guest whose linear memory is the input after [`HEADER`], and whose
/// `cabi_realloc` and export answer with the header's numbers.
struct Bytes<'a> {
    memory: Vec<u8>,
    kind: u8,
    numbers: &'a [u8],
    next: usize,
}

impl Bytes<'_> {
    /// A number's top two bits pick how it reads: an aligned address inside
    /// memory (half of them), a small number, or the raw value.
    fn number(&mut self) -> u32 {
        let at = 4 * (self.next % NUMBERS);
        self.next += 1;
        let raw = u32::from_le_bytes(self.numbers[at..at + 4].try_into().unwrap());
        match raw >> 30 {
            0 | 1 => (raw % self.memory.len().max(1) as u32) & !3,
            2 => raw & 0xf,
            _ => raw,
        }
    }
}

impl Guest for Bytes<'_> {
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
        Ok(match self.kind % 4 {
            0 => vec![Core::I32(self.number() as i32)],
            1 => vec![Core::I64(
                (u64::from(self.number()) << 32 | u64::from(self.number())) as i64,
            )],
            2 => vec![
                Core::I32(self.number() as i32),
                Core::I32(self.number() as i32),
            ],
            _ => (0..(self.kind >> 2) % 6)
                .map(|_| Core::I32(self.number() as i32))
                .collect(),
        })
    }

    fn rep(&mut self, h: u32) -> Result<u32, Trap> {
        Ok(h)
    }
}

/// The `appabi_call` target: calls a function of two parameters whose result
/// is of the type the input's first byte picks, against the guest the rest of
/// it makes. A result is lifted or refused with a trap; one that is lifted has
/// its type, so it prints as WAVE, and the printed text (when not too long to
/// read back) reads and prints the same. True when the call lifted a value.
pub fn appabi_call(data: &[u8]) -> bool {
    let mut header = [0; HEADER];
    let cut = data.len().min(HEADER);
    header[..cut].copy_from_slice(&data[..cut]);
    let shapes = shapes();
    let ty = shapes[header[0] as usize % shapes.len()].clone();
    let mut guest = Bytes {
        memory: data[cut..].to_vec(),
        kind: header[1],
        numbers: &header[2..],
        next: 0,
    };
    let func = Func {
        export: "api#f".into(),
        params: vec![Ty::String, Ty::List(Box::new(Ty::U32))],
        result: Some(ty.clone()),
    };
    let args = [
        Val::String("hostile".into()),
        Val::List(vec![Val::U32(1); 3]),
    ];
    let Ok(value) = call(&mut guest, &func, &args) else {
        return false;
    };
    let value = value.expect("a function with a result gives a value");
    let printed = wave::print(&value, &ty).expect("a lifted value has its type");
    if printed.len() <= MAX_TEXT {
        let again = wave::parse(&printed, &ty).expect("a printed value reads");
        assert_eq!(wave::print(&again, &ty).unwrap(), printed);
    }
    true
}

/// The `appabi_wave` target: the input's first byte picks a type and the rest
/// is read as WAVE text of it. Text that is read prints to text that reads and
/// prints the same.
pub fn appabi_wave(data: &[u8]) {
    let Some((&pick, text)) = data.split_first() else {
        return;
    };
    let Ok(text) = core::str::from_utf8(text) else {
        return;
    };
    let shapes = shapes();
    let ty = &shapes[pick as usize % shapes.len()];
    if let Ok(value) = wave::parse(text, ty) {
        let printed = wave::print(&value, ty).expect("a parsed value prints");
        let again = wave::parse(&printed, ty).expect("printed text parses");
        assert_eq!(wave::print(&again, ty).unwrap(), printed);
    }
}

/// WAVE texts that read as one or more of [`shapes`].
pub const WAVE_SAMPLES: [&str; 3] = [
    r#"{x: 3, label: some("a \"b\" \u{41}"), tags: [1, 2, 300], kind: b, deep: [[ok(1), err("e")]]}"#,
    r"[(1, true, 'c'), (2, false, '\n')]",
    r#"real(1.5e3) text("x") pt({x: -1, y: 2}) {f0, f3, f11} none"#,
];

/// `text` as an `appabi_wave` input of the type at `pick`.
pub fn wave_input(pick: usize, text: &str) -> Vec<u8> {
    let mut input = vec![pick as u8];
    input.extend_from_slice(text.as_bytes());
    input
}

/// The corpus seeds: `(target, input)`, a WAVE text that reads for each type,
/// and a call input for each type with numbers that point into its memory.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let texts = [
        "true",
        "18446744073709551615",
        "-1.5e3",
        "'\\u{1f600}'",
        "\"a\\tb\"",
        "[0, 255]",
        "[\"a\", \"\"]",
        "[[1], [], [2, 3]]",
        "[(), ()]",
        "some(\"x\")",
        "err(\"no\")",
        "(7, \"s\", -9)",
        "pt({x: 1, y: -2})",
        "b",
        "{f0, f11}",
        "[{name: \"n\", corners: [{x: 0, y: 0}], tag: none}]",
    ];
    let mut seeds: Vec<_> = texts
        .iter()
        .enumerate()
        .map(|(pick, text)| ("appabi_wave", wave_input(pick, text)))
        .collect();
    for pick in 0..shapes().len() {
        let mut input = vec![pick as u8, pick as u8];
        for n in 0..NUMBERS as u32 {
            input.extend_from_slice(&(64 * n).to_le_bytes());
        }
        // Each word is a small pointer or a length, so lists and strings
        // land inside memory.
        input.extend((0..1024u32).flat_map(|w| ((w * 8) % 512 + w % 3).to_le_bytes()));
        seeds.push(("appabi_call", input));
    }
    seeds
}
