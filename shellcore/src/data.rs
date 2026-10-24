//! `Data`: what a content read hands a script (ADR 0002 D6). It is opaque: no
//! `to_string`, comparison, length or conversion is registered, so Rhai formats it
//! as its type name and the content cannot become a plain value. `show(d)` prints
//! it to the user, the one way out.

use crate::engine::{self, Line};
use alloc::rc::Rc;
use alloc::string::String;
use rhai::Engine;

/// Content read from `source`, held where a script cannot see into it.
#[derive(Clone, Debug)]
pub struct Data {
    pub source: &'static str,
    text: Rc<str>,
}

impl Data {
    pub fn new(source: &'static str, text: impl Into<String>) -> Self {
        Self {
            source,
            text: Rc::from(text.into()),
        }
    }
}

/// Registers the `Data` type and `show` on a strict engine whose output is `line`'s.
pub(crate) fn register(engine: &mut Engine, line: &Rc<Line>, max_output: usize) {
    engine.register_type_with_name::<Data>("Data");
    let line = Rc::clone(line);
    engine.register_fn("show", move |data: Data| {
        engine::capture_output(&line, max_output, &data.text);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhai::Dynamic;

    fn run(script: &str) -> (Result<Dynamic, String>, String) {
        let line = Line::new();
        let mut engine = Engine::new();
        register(&mut engine, &line, 4096);
        engine.register_fn("note_read", || {
            Data::new("note \"todo\"", "rm -rf the secret")
        });
        let capture = Rc::clone(&line);
        engine.on_print(move |text| engine::capture_output(&capture, 4096, text));
        let result = engine
            .eval::<Dynamic>(script)
            .map_err(|e| alloc::format!("{e}"));
        (result, line.take_output())
    }

    #[test]
    fn content_cannot_become_a_plain_value() {
        for script in [
            "print(\"x\" + note_read());",
            "print(`${note_read()}`);",
            "print(note_read());",
            "note_read().to_string()",
            "let d = note_read(); d == \"rm -rf the secret\"",
        ] {
            let (result, output) = run(script);
            let all = alloc::format!("{result:?} {output}");
            assert!(!all.contains("secret"), "{script}: {all}");
        }
        for script in [
            "note_read().len()",
            "note_read()[0]",
            "parse_int(note_read())",
        ] {
            assert!(run(script).0.is_err(), "{script}");
        }
    }

    #[test]
    fn show_prints_it_to_the_user() {
        let (result, output) = run("show(note_read());");
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(output, "rm -rf the secret\n");
    }
}
