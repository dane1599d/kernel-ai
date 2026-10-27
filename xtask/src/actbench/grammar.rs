//! The mock registry's rows as `infer::tagged` actions, so actbench measures the
//! decoder that ships (task 6). Its grammar tests live with it in
//! `infer/src/tagged/tests.rs`.

use super::registry::{Row, Ty};
use infer::tagged::{Action, Arg, Kind};

pub type Tagged = infer::tagged::Tagged<'static>;

fn kind(ty: &Ty) -> Kind<'static> {
    let text = |chars, lines, line_chars, empty| Kind::Text {
        chars,
        lines,
        line_chars,
        empty,
    };
    match ty {
        Ty::Enum(words) => Kind::Enum(words),
        Ty::Int { min, max } => Kind::Int {
            min: *min,
            max: *max,
        },
        Ty::Text(max) => text(*max, 1, *max, true),
        Ty::Host => Kind::Host,
        Ty::Path => text(128, 1, 128, false),
        Ty::Code => text(
            llmproto::script::SCRIPT_MAX_BYTES as u16,
            llmproto::script::SCRIPT_MAX_LINES as u8,
            llmproto::script::SCRIPT_MAX_LINE_CHARS as u16,
            true,
        ),
        Ty::List(inner, n) => Kind::List(Box::leak(Box::new(kind(inner))), *n),
        Ty::Opt(inner) => Kind::Opt(Box::leak(Box::new(kind(inner)))),
    }
}

/// The tagged grammar over `rows`. The mock rows are all valid (a test checks).
pub fn tagged(rows: &[&'static Row]) -> Tagged {
    let actions: Vec<Action<'static>> = rows
        .iter()
        .map(|row| Action {
            name: row.name,
            args: row
                .args
                .iter()
                .map(|a| Arg {
                    name: a.name,
                    kind: kind(&a.ty),
                })
                .collect::<Vec<_>>()
                .leak(),
        })
        .collect();
    Tagged::new(&actions).expect("the mock registry decodes")
}

/// [`tagged`], with the rows no task needs (`registry::DISTRACTORS`) closing
/// with no arguments, as the shipping decoder closes an index-only row
/// (`llmproto::local::act::Grammar`). No task calls one, so the second round
/// never changes a score and is not run.
pub fn indexed(rows: &[&'static Row]) -> Tagged {
    let rows: Vec<&'static Row> = rows
        .iter()
        .map(|row| match super::registry::is_distractor(row) {
            true => &*Box::leak(Box::new(Row { args: &[], ..**row })),
            false => *row,
        })
        .collect();
    tagged(&rows)
}

#[cfg(test)]
mod tests {
    use super::super::registry::rows;
    use super::*;
    use infer::json::Constraint;

    #[test]
    fn every_scale_of_the_registry_decodes_and_its_shapes_close() {
        let g = tagged(&rows(80));
        for text in [
            r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#,
            r#"{"name":"editor_open","arguments":{"path":"/home/a.rs","line":null}}"#,
            r#"{"name":"script","arguments":{"code":"print(1);"}}"#,
            r#"{"name":"sound_set","arguments":{"level":100}}"#,
        ] {
            let state = g.advance(g.start(), text.as_bytes()).unwrap();
            assert!(g.is_done(&state), "{text}");
        }
        tagged(&rows(40));
        tagged(&super::super::registry::m8_rows());
    }

    #[test]
    fn at_the_index_level_a_distractor_closes_with_no_arguments() {
        let g = indexed(&rows(80));
        let closed = r#"{"name":"sound_set","arguments":{}}"#;
        assert!(g.is_done(&g.advance(g.start(), closed.as_bytes()).unwrap()));
        let full = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
        assert!(g.is_done(&g.advance(g.start(), full.as_bytes()).unwrap()));
        assert!(
            g.advance(g.start(), br#"{"name":"sound_set","arguments":{"level""#)
                .is_err()
        );
    }
}
