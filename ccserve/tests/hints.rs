//! wcc's commonest messages with what to write instead (`ccserve::hints`),
//! through the pinned cc.wasm: a window app's `cc build`, as the app gate's
//! repairs see it, and an entry at `c>`.

mod common;

use ccserve::build::{self, Build, Kind};
use ccserve::{Budget, Compiler, Host};
use common::Cc;
use std::sync::OnceLock;

struct Clock;

impl Host for Clock {
    fn now_ns(&self) -> u64 {
        0
    }

    fn interrupted(&self) -> bool {
        false
    }
}

fn compiler() -> &'static Compiler {
    static COMPILER: OnceLock<Compiler> = OnceLock::new();
    COMPILER.get_or_init(|| {
        let cc = Compiler::new(ccserve::CC_WASM, &[]).expect("cc.wasm");
        build::compiler(&cc).expect("headers")
    })
}

/// A window app whose `view` has `body` before its label.
fn app(body: &str) -> String {
    format!(
        "#include <kaiapp.h>
const char TITLE[] = \"Words\";
struct state {{ I64 count; }};
void init(struct state *s) {{ s->count = 0; }}
void on_event(struct state *s, const struct kai_event *e) {{ s->count++; }}
void view(const struct state *s) {{
    char buf[64];
    {body}
    ui_label(1, 0, buf);
}}
"
    )
}

/// A component, which has no `<kaiapp.h>` and so no `rand`, whose health
/// runs `body` first.
fn component(body: &str) -> String {
    format!(
        "#include <component.h>
#include <stdlib.h>
uint32_t exports_kai_live_lifecycle_schema(void) {{ return 1; }}
bool exports_kai_live_lifecycle_init(exports_kai_live_lifecycle_start_error_t *err) {{ return true; }}
bool exports_kai_live_lifecycle_migrate(component_list_u8_t *state, uint32_t from_schema,
                                        exports_kai_live_lifecycle_start_error_t *err) {{ return true; }}
bool exports_kai_live_lifecycle_save(uint32_t to_schema, component_list_u8_t *ret) {{
    ret->ptr = NULL;
    ret->len = 0;
    return true;
}}
uint32_t exports_kai_live_lifecycle_health(void) {{
    {body}
    return 0;
}}
uint32_t exports_kai_live_lifecycle_self_test(void) {{ return 0; }}
"
    )
}

/// The build of `source` as `kind`: what it made, or why it was refused, as
/// the shell shows it.
fn build_as(kind: Kind, source: &str) -> Result<(), String> {
    let files: Vec<(&str, &[u8])> = vec![("main.c", source.as_bytes())];
    let request = Build {
        name: "words",
        version: "0.1.0",
        kind,
        files: &files,
    };
    let mut budget = Budget::new(&Clock, u64::MAX);
    match build::build(compiler(), &request, &mut budget) {
        Ok(_) => Ok(()),
        Err(error) => Err(error.text().expect("not stopped")),
    }
}

/// Why the build of [`app`] with `body` was refused, as the shell shows it.
fn refused(body: &str) -> String {
    match build_as(Kind::Window, &app(body)) {
        Ok(()) => panic!("{body:?} built"),
        Err(why) => why,
    }
}

/// Why the build of [`component`] with `body` was refused.
fn component_refused(body: &str) -> String {
    match build_as(Kind::Component, &component(body)) {
        Ok(()) => panic!("{body:?} built"),
        Err(why) => why,
    }
}

#[test]
fn the_apps_body_builds_as_it_is() {
    let source = app("Format(buf, 64, \"%lld\", s->count);");
    let files: Vec<(&str, &[u8])> = vec![("main.c", source.as_bytes())];
    let request = Build {
        name: "words",
        version: "0.1.0",
        kind: Kind::Window,
        files: &files,
    };
    let mut budget = Budget::new(&Clock, u64::MAX);
    assert!(build::build(compiler(), &request, &mut budget).is_ok());
}

/// The app gate's `words` miss: a newline character literal written with
/// the newline itself.
#[test]
fn a_character_split_by_a_newline_is_told_to_write_backslash_n() {
    let why = refused("char c = '\n';\n    buf[0] = c;");
    assert!(
        why.starts_with("main.c: /w/main.c(8): Character not closed\n"),
        "{why}"
    );
    assert!(
        why.contains(r"hint: a character literal is one character in single quotes, closed on the same line: write '\n' for a newline"),
        "{why}"
    );
}

#[test]
fn a_string_split_by_a_newline_is_told_to_write_backslash_n() {
    let why = refused("Format(buf, 64, \"a\nb\");");
    assert!(why.contains("String not closed"), "{why}");
    assert!(
        why.contains(
            r"hint: a string must close on the line it opens: write \n for a newline in it"
        ),
        "{why}"
    );
}

#[test]
fn a_missing_semicolon_is_looked_for_before_where_wcc_stopped() {
    let why = refused("Format(buf, 64, \"x\")\n");
    // wcc stops at the next statement, the line after the one to change.
    assert!(
        why.starts_with("main.c: /w/main.c(10): `;' expected\n"),
        "{why}"
    );
    assert!(
        why.contains(
            "hint: the statement before this point has no ';' at its end, often on the line above"
        ),
        "{why}"
    );
}

/// A window app has `<kaiapp.h>`'s `rand()` (#701), declared as C's own, so
/// a call builds, and so does an app's own `int rand(void);` with it: an
/// `I64 rand(void)` would link the call's i32 against an i64 definition.
#[test]
fn a_window_apps_rand_builds_with_its_own_declaration_too() {
    for body in [
        "Format(buf, 64, \"%lld\", (I64)(rand() % 6));",
        "int rand(void);\n    Format(buf, 64, \"%lld\", (I64)(rand() % 6));",
    ] {
        assert_eq!(build_as(Kind::Window, &app(body)), Ok(()), "{body}");
    }
}

/// A component has no `rand`: a call gets one hint, with a generator.
#[test]
fn a_components_rand_gets_one_hint_with_a_generator() {
    let why = component_refused("int roll = rand() % 6;");
    assert!(why.contains("`rand' undeclared"), "{why}");
    assert_eq!(why.matches("hint: ").count(), 1, "{why}");
    assert!(
        why.trim_end()
            .ends_with("then (seed >> 33) % n is below n."),
        "{why}"
    );
}

#[test]
fn rand_declared_but_undefined_is_named_by_the_link() {
    let why = component_refused("int rand(void);\n    int roll = rand() % 6;");
    assert!(
        why.starts_with("the link: Unresolved: env.rand\nhint: there is no rand() here"),
        "{why}"
    );
}

#[test]
fn an_apps_printf_is_told_to_format_and_label() {
    let why = refused("printf(\"%lld\", s->count);");
    assert!(
        why.contains("hint: there is no printf() here: Format(out, cap, fmt, ...)"),
        "{why}"
    );
}

#[test]
fn an_entrys_rand_and_printf_are_hinted_on_their_line() {
    let mut cc = Cc::open();
    let err = cc.eval("I64 a = 1;\nI64 roll(void) { return rand() % 6; }");
    assert!(
        err[0].starts_with("err 2 line 2: `rand' undeclared"),
        "{err:?}"
    );
    assert!(err[0].contains("hint: there is no rand() here"), "{err:?}");
    let err = cc.eval("printf(\"hi\");");
    assert!(
        err[0].contains("hint: there is no printf() here: Print(fmt, ...) prints as printf does."),
        "{err:?}"
    );
}
