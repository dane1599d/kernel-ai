//! The probe guest: every function is a small, deterministic transform of its
//! arguments, so two engines given the same call must give the same result.

use std::cell::Cell;

wit_bindgen::generate!({ world: "probe", path: "../wit" });

use exports::kai::probe::api::{
    Color, Counter, Guest, GuestCounter, Perms, Point, Shape, Value,
};

struct Probe;

struct Count(Cell<u32>);

impl GuestCounter for Count {
    fn new(start: u32) -> Self {
        Count(Cell::new(start))
    }

    fn add(&self, n: u32) -> u32 {
        self.0.set(self.0.get().wrapping_add(n));
        self.0.get()
    }

    fn get(&self) -> u32 {
        self.0.get()
    }
}

fn turn(p: Point) -> Point {
    Point {
        x: p.y.wrapping_neg(),
        y: p.x.wrapping_add(1),
    }
}

fn next(c: Color) -> Color {
    match c {
        Color::Red => Color::Green,
        Color::Green => Color::Blue,
        Color::Blue => Color::Red,
    }
}

impl Guest for Probe {
    type Counter = Count;

    fn f_bool(a: bool) -> bool {
        !a
    }

    fn f_u8(a: u8, b: u8) -> u8 {
        a.wrapping_add(b)
    }

    fn f_s16(a: i16) -> i16 {
        a.wrapping_mul(-3)
    }

    fn f_u32(a: u32, b: u32) -> u32 {
        a ^ b.rotate_left(7)
    }

    fn f_s64(a: i64) -> i64 {
        a.wrapping_neg().wrapping_sub(1)
    }

    fn f_u64(a: u64) -> u64 {
        a.swap_bytes()
    }

    fn f_f32(a: f32) -> f32 {
        a * 2.5 - 1.0
    }

    fn f_f64(a: f64, b: f64) -> f64 {
        a / 3.0 + b
    }

    fn f_char(c: char) -> char {
        char::from_u32(c as u32 ^ 1).unwrap_or(c)
    }

    fn f_string(s: String) -> String {
        s.chars().rev().collect()
    }

    fn f_bytes(b: Vec<u8>) -> Vec<u8> {
        b.iter().rev().map(|x| x.wrapping_add(1)).collect()
    }

    fn f_strings(l: Vec<String>) -> Vec<String> {
        l.into_iter().map(|s| format!("{}:{s}", s.len())).collect()
    }

    fn f_record(p: Point) -> Point {
        turn(p)
    }

    fn f_tuple(t: (u8, String, i64)) -> (i64, String, u8) {
        (t.2.wrapping_mul(2), t.1.to_uppercase(), t.0.wrapping_sub(1))
    }

    fn f_variant(v: Value) -> Value {
        match v {
            Value::None => Value::Int(0),
            Value::Int(n) => Value::Text(n.to_string()),
            Value::Real(r) => Value::Int(r as i64),
            Value::Text(t) => Value::Real(t.len() as f64 / 2.0),
            Value::Pt(p) => Value::Pt(turn(p)),
        }
    }

    fn f_enum(c: Color) -> Color {
        next(c)
    }

    fn f_option(o: Option<String>) -> Option<u32> {
        o.map(|s| s.len() as u32)
    }

    fn f_result(r: Result<u32, String>) -> Result<String, u32> {
        match r {
            Ok(n) => Ok(n.to_string()),
            Err(e) => Err(e.len() as u32),
        }
    }

    fn f_flags(p: Perms) -> Perms {
        p ^ (Perms::READ | Perms::ADMIN)
    }

    fn f_shape(s: Shape) -> Shape {
        Shape {
            name: format!("{}!", s.name),
            corners: s.corners.into_iter().rev().map(turn).collect(),
            color: next(s.color),
            tag: match s.tag {
                Some(t) if t.is_empty() => None,
                Some(t) => Some(t.to_uppercase()),
                None => Some(String::from("new")),
            },
        }
    }

    fn f_shapes(l: Vec<Shape>) -> Vec<u32> {
        l.iter()
            .map(|s| s.corners.len() as u32 * 10 + s.name.len() as u32)
            .collect()
    }

    fn f_many(
        a: u8,
        b: u16,
        c: u32,
        d: u64,
        e: i8,
        f: i16,
        g: i32,
        h: i64,
        i: f32,
        j: f64,
        k: char,
        l: bool,
        m: String,
        n: Point,
        o: Point,
        p: u32,
    ) -> u64 {
        let mut sum = u64::from(a) + u64::from(b) + u64::from(c);
        sum = sum.wrapping_add(d).wrapping_add(e as u64).wrapping_add(f as u64);
        sum = sum.wrapping_add(g as u64).wrapping_add(h as u64);
        sum = sum.wrapping_add(i.to_bits() as u64).wrapping_add(j.to_bits());
        sum = sum.wrapping_add(k as u64).wrapping_add(l as u64);
        sum = sum.wrapping_add(m.len() as u64);
        sum = sum.wrapping_add((n.x as u64) << 1).wrapping_add(n.y as u64);
        sum.wrapping_add((o.x as u64) << 2)
            .wrapping_add(o.y as u64)
            .wrapping_add(u64::from(p))
    }

    fn make_counter(start: u32) -> Counter {
        Counter::new(Count(Cell::new(start.wrapping_mul(2))))
    }

    fn peek(c: exports::kai::probe::api::CounterBorrow<'_>) -> u32 {
        c.get::<Count>().get()
    }

    fn consume(c: Counter) -> u32 {
        c.get::<Count>().get().wrapping_add(1000)
    }
}

export!(Probe);
