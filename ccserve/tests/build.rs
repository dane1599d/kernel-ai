//! `cc build` (ADR 0012 D7): C sources to an artifact wasmhost loads and runs,
//! with nothing imported but the world's, and hostile requests refused.

use ccserve::build::{self, Build, BuildError, Built, Kind};
use ccserve::{Budget, CompileError, Compiler, Host};
use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION};
use livecore::artifact::{custom_section, parse};
use livecore::{Instance, Reason};
use std::sync::OnceLock;
use wasmhost::{Limits, LiveInstance};

const LIMITS: Limits = Limits {
    fuel_call: 50_000_000,
    fuel_life: 50_000_000,
    memory_bytes: 4 << 20,
};

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

/// A component whose lifecycle answers schema 3 and health `health`, with
/// `more` before it.
fn component(health: &str, more: &str) -> String {
    format!(
        "#include <component.h>
#include <stdlib.h>
#include <string.h>
{more}
uint32_t exports_kai_live_lifecycle_schema(void) {{ return 3; }}
bool exports_kai_live_lifecycle_init(exports_kai_live_lifecycle_start_error_t *err) {{ return true; }}
bool exports_kai_live_lifecycle_migrate(component_list_u8_t *state, uint32_t from_schema,
                                        exports_kai_live_lifecycle_start_error_t *err) {{ return true; }}
bool exports_kai_live_lifecycle_save(uint32_t to_schema, component_list_u8_t *ret) {{
    ret->ptr = NULL;
    ret->len = 0;
    return true;
}}
uint32_t exports_kai_live_lifecycle_health(void) {{ {health} }}
uint32_t exports_kai_live_lifecycle_self_test(void) {{ return 0; }}
"
    )
}

fn try_build(name: &str, kind: Kind, files: &[(&str, &[u8])]) -> Result<Built, BuildError> {
    let mut budget = Budget::new(&Clock, u64::MAX / 4);
    build::build(
        compiler(),
        &Build {
            name,
            version: "0.0.3",
            kind,
            files,
        },
        &mut budget,
    )
}

fn built(source: &str) -> Built {
    try_build("hello", Kind::Component, &[("hello.c", source.as_bytes())])
        .unwrap_or_else(|e| panic!("{e:?}"))
}

fn load(built: &Built) -> LiveInstance {
    wasmhost::load_live(&built.artifact, LIMITS, None).unwrap_or_else(|e| panic!("{e:?}"))
}

fn module_error(result: Result<Built, BuildError>) -> String {
    match result {
        Err(BuildError::Module(why)) => why,
        Ok(_) => panic!("built"),
        Err(other) => panic!("{other:?}"),
    }
}

fn link_error(result: Result<Built, BuildError>) -> String {
    match result {
        Err(BuildError::Link(CompileError::Source(why))) => why,
        Ok(_) => panic!("built"),
        Err(other) => panic!("{other:?}"),
    }
}

fn section(artifact: &[u8], name: &str) -> Option<String> {
    custom_section(artifact, name)
        .unwrap()
        .map(|s| String::from_utf8(s.to_vec()).unwrap())
}

#[test]
fn a_component_builds_and_wasmhost_runs_it() {
    let built = built(&component("return 7;", ""));
    let mut instance = load(&built);
    assert_eq!(instance.schema(), Ok(3));
    assert_eq!(instance.init(), Ok(0));
    assert_eq!(instance.health(), Ok(7));
    assert_eq!(instance.self_test(), Ok(0));

    let parsed = parse(&built.artifact).unwrap();
    assert_eq!(parsed.manifest.lifecycle, 2);
    assert_eq!(parsed.manifest.component, "hello");
    assert_eq!(parsed.manifest.version, "0-0-3");
    assert_eq!(parsed.manifest.world, "app:hello/hello@0.0.3");
    assert_eq!(parsed.manifest.engine, "wasmi");
    assert_eq!(parsed.manifest.source, built.source);
    assert_eq!(parsed.manifest.licence, build::NO_LICENCE);
    assert_eq!(
        section(&built.artifact, ACTIONS_SECTION).unwrap(),
        "kai-actions 1\nworld app:hello/hello@0.0.3\n"
    );
    assert_eq!(section(&built.artifact, build::APP_SECTION), None);
    let notices = section(&built.artifact, build::NOTICES_SECTION).unwrap();
    assert!(notices.contains(build::XCC_COMMIT) && notices.contains("MIT License"));
    assert_eq!(
        built.provenance(),
        format!("built-by cc {} source {}", build::XCC_COMMIT, built.source)
    );
}

#[test]
fn an_app_builds_with_its_kai_app_and_wasmhost_runs_it() {
    let source = "#include <app.h>
// SPDX-License-Identifier: MIT
uint32_t exports_kai_live_lifecycle_schema(void) { return 1; }
bool exports_kai_live_lifecycle_init(exports_kai_live_lifecycle_start_error_t *err) { return true; }
bool exports_kai_live_lifecycle_migrate(app_list_u8_t *state, uint32_t from_schema,
                                        exports_kai_live_lifecycle_start_error_t *err) { return true; }
bool exports_kai_live_lifecycle_save(uint32_t to_schema, app_list_u8_t *ret) {
    ret->ptr = 0;
    ret->len = 0;
    return true;
}
uint32_t exports_kai_live_lifecycle_health(void) { return 5; }
uint32_t exports_kai_live_lifecycle_self_test(void) { return 0; }
void exports_kai_app_events_on_event(exports_kai_app_events_event_t *e) {
    exports_kai_app_events_event_free(e);
}
";
    let built = try_build("greeter", Kind::App, &[("main.c", source.as_bytes())]).unwrap();
    let mut instance = load(&built);
    assert_eq!(instance.init(), Ok(0));
    assert_eq!(instance.health(), Ok(5));
    let parsed = parse(&built.artifact).unwrap();
    assert_eq!(parsed.manifest.world, "app:greeter/greeter@0.0.3");
    assert_eq!(parsed.manifest.licence, "MIT");
    assert_eq!(
        section(&built.artifact, build::APP_SECTION).unwrap(),
        "kai-app 1\nid greeter\nabi 1.0\n"
    );
    assert!(section(&built.artifact, MANIFEST_SECTION).is_some());
}

#[test]
fn the_allocator_reuses_merges_grows_and_zeroes() {
    let check = r#"
static uint32_t check(void) {
    char *g = malloc(64), *h = malloc(64);
    if (!g || h != g + 72) return 1;
    free(g);
    free(h);
    char *k = malloc(128);
    if (k != g) return 2;
    char *a = malloc(100);
    free(a);
    if (malloc(100) != a) return 3;
    char *big = malloc(300000);
    if (!big) return 4;
    memset(big, 7, 300000);
    int *zeros = calloc(1000, sizeof(int));
    if (!zeros) return 5;
    for (int i = 0; i < 1000; i++)
        if (zeros[i]) return 6;
    if (realloc(a, 50) != a) return 7;
    strcpy(a, "hello");
    char *moved = realloc(a, 5000);
    if (!moved || strcmp(moved, "hello") != 0) return 8;
    if (malloc(0x40000001u) || calloc(0x40000000u, 4)) return 9;
    free(big);
    free(zeros);
    free(moved);
    free(NULL);
    /* Memory grown by other means is never handed out. */
    char *before = malloc(16);
    int old = __builtin_wasm_memory_grow(0, 1);
    if (!before || old < 0) return 10;
    char *region = (char *)((uintptr_t)old * 65536);
    char *after = malloc(70000);
    if (!after || (after < region + 65536 && after + 70000 > region)) return 11;
    return 0;
}
"#;
    let mut instance = load(&built(&component("return check();", check)));
    assert_eq!(instance.health(), Ok(0));
}

#[test]
fn abort_traps_and_names_itself() {
    let mut instance = load(&built(&component("abort();", "")));
    let fault = instance.health().unwrap_err();
    assert_eq!(fault.reason, Reason::Trap);
    assert_eq!(fault.detail, "panicked at abort");
}

#[test]
fn a_build_that_imports_wasi_is_refused() {
    let more = r#"__attribute__((import_module("wasi_snapshot_preview1"), import_name("fd_write")))
int w(int fd, const void *iovs, int count, int *written);"#;
    let why = module_error(try_build(
        "hello",
        Kind::Component,
        &[(
            "hello.c",
            component("return w(1, 0, 0, 0);", more).as_bytes(),
        )],
    ));
    assert!(why.contains("wasi_snapshot_preview1.fd_write"), "{why}");
}

#[test]
fn a_component_cannot_import_an_apps_calls() {
    let more = r#"__attribute__((import_module("kai:app/storage@0.1.0"), import_name("get")))
int get(int a, int b, int c);"#;
    let why = module_error(try_build(
        "hello",
        Kind::Component,
        &[(
            "hello.c",
            component("return get(0, 0, 0);", more).as_bytes(),
        )],
    ));
    assert!(why.contains("kai:app/storage@0.1.0.get"), "{why}");
}

#[test]
fn a_world_import_with_another_type_is_refused() {
    let more = r#"__attribute__((import_module("kai:app/log@0.1.0"), import_name("write")))
void write3(int a, int b, int c);"#;
    let why = module_error(try_build(
        "hello",
        Kind::Component,
        &[(
            "hello.c",
            component("write3(0, 0, 0); return 0;", more).as_bytes(),
        )],
    ));
    assert!(why.contains("does not validate"), "{why}");
}

#[test]
fn a_symbol_nothing_defines_fails_the_link_by_name() {
    let why = link_error(try_build(
        "hello",
        Kind::Component,
        &[(
            "hello.c",
            component("return nowhere();", "int nowhere(void);").as_bytes(),
        )],
    ));
    assert!(why.contains("nowhere"), "{why}");
}

#[test]
fn a_missing_lifecycle_export_fails_the_link() {
    let source = component("return 0;", "").replace(
        "uint32_t exports_kai_live_lifecycle_self_test(void) { return 0; }",
        "",
    );
    let why = link_error(try_build(
        "hello",
        Kind::Component,
        &[("hello.c", source.as_bytes())],
    ));
    assert!(
        why.contains("exports_kai_live_lifecycle_self_test"),
        "{why}"
    );
}

#[test]
fn redefining_the_runtime_fails_the_link() {
    let more = "void *malloc(size_t size) { return 0; }";
    assert!(matches!(
        try_build(
            "hello",
            Kind::Component,
            &[("hello.c", component("return 0;", more).as_bytes())]
        ),
        Err(BuildError::Link(_))
    ));
}

#[test]
fn a_file_that_does_not_compile_is_named() {
    match try_build(
        "hello",
        Kind::Component,
        &[
            ("hello.c", component("return 0;", "").as_bytes()),
            ("bad.c", b"int x = ;"),
        ],
    ) {
        Err(BuildError::Compile(name, CompileError::Source(why))) => {
            assert_eq!(name, "bad.c");
            assert!(why.contains("bad.c"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_source_includes_its_own_header() {
    let source = component("return answer();", "#include \"answer.h\"");
    let built = try_build(
        "hello",
        Kind::Component,
        &[
            ("hello.c", source.as_bytes()),
            ("answer.h", b"static uint32_t answer(void) { return 42; }\n"),
        ],
    )
    .unwrap();
    assert_eq!(load(&built).health(), Ok(42));
}

#[test]
fn the_same_input_builds_the_same_bytes() {
    let one = built(&component("return 1;", ""));
    let two = built(&component("return 1;", ""));
    assert_eq!(one.artifact, two.artifact);
    let other = built(&component("return 2;", ""));
    assert_ne!(one.source, other.source);
    assert_ne!(one.provenance(), other.provenance());
}

#[test]
fn the_provenance_names_the_pinned_xcc() {
    let upstream = include_str!("../../third_party/xcc/UPSTREAM.toml");
    assert!(upstream.contains(&format!("commit = \"{}\"", build::XCC_COMMIT)));
}

#[test]
fn sources_that_expand_into_huge_objects_stop_before_the_link() {
    let main = component("return 0;", "");
    // A few bytes of source whose compile needs more than a compile's 4 MiB
    // of memory (`compiler::MAX_MEMORY`): cc.wasm traps, naming the file.
    let huge = "static const unsigned char big[1572864] = {1};\n";
    let sources: Vec<(&str, &[u8])> =
        vec![("huge.c", huge.as_bytes()), ("main.c", main.as_bytes())];
    match try_build("hello", Kind::Component, &sources) {
        Err(BuildError::Compile(name, CompileError::Compiler(why))) => {
            assert_eq!(name, "huge.c", "{why}");
        }
        other => panic!("{other:?}"),
    }
    // Each of these compiles within it, its object holding 256 KiB of data,
    // and wcc spends about 67 M fuel on each. The build's fuel cap, there
    // should the clock fail, stops it at the sixteenth, as the 4 MiB object
    // cap would at about the same file.
    let big = "static const unsigned char big[262144] = {1};\n";
    let files: Vec<String> = (0..17).map(|i| format!("big{i:02}.c")).collect();
    let mut sources: Vec<(&str, &[u8])> =
        files.iter().map(|n| (n.as_str(), big.as_bytes())).collect();
    sources.push(("main.c", main.as_bytes()));
    match try_build("hello", Kind::Component, &sources) {
        Err(BuildError::Compile(name, CompileError::Compiler(why))) => {
            assert!(why.contains("too much fuel"), "{name}: {why}");
        }
        Err(BuildError::Module(why)) => assert!(why.contains("over 4 MiB"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn names_versions_and_files_are_checked_before_anything_compiles() {
    let ok = component("return 0;", "");
    let ok: &[u8] = ok.as_bytes();
    let refused = |name: &str, version: &str, files: &[(&str, &[u8])]| {
        let mut budget = Budget::new(&Clock, 0);
        let build = Build {
            name,
            version,
            kind: Kind::Component,
            files,
        };
        // A zero budget: anything that compiled would stop at the deadline.
        match build::build(compiler(), &build, &mut budget) {
            Err(BuildError::Request(why)) => why,
            other => panic!("{name} {version}: {other:?}"),
        }
    };
    for name in [
        "h",
        "Hello",
        "1hello",
        "hello_world",
        "thirteen-char",
        "",
        "fs",
        "pkg",
        // Package names, but not WIT names.
        "app-2",
        "a-",
        "ab--c",
    ] {
        refused(name, "0.0.1", &[("hello.c", ok)]);
    }
    for version in [
        "",
        "1",
        "0.0",
        "0.0.1.0",
        "01.0.0",
        "0.0.x",
        "0.0.-1",
        "1000000000.0.0",
        " 0.0.1",
        // Over a manifest label's 16 bytes.
        "100000000.100000.1",
    ] {
        refused("hello", version, &[("hello.c", ok)]);
    }
    let long = format!("{}.c", "a".repeat(63));
    for name in [
        "../x.c",
        "a/b.c",
        ".c",
        "x.txt",
        "x.C",
        "x c.c",
        "x.c\n",
        "x:y.c",
        long.as_str(),
    ] {
        refused("hello", "0.0.1", &[(name, ok)]);
    }
    refused("hello", "0.0.1", &[("a.h", ok)]);
    refused("hello", "0.0.1", &[("a.c", ok), ("a.c", ok)]);
    let many: Vec<String> = (0..=build::MAX_FILES).map(|i| format!("f{i}.c")).collect();
    let many: Vec<(&str, &[u8])> = many.iter().map(|n| (n.as_str(), ok)).collect();
    refused("hello", "0.0.1", &many);
    let big = vec![b'x'; 300 << 10];
    refused("hello", "0.0.1", &[("a.c", ok), ("big.h", &big)]);
}

#[test]
fn the_licence_is_the_one_every_file_names() {
    let mit: &[u8] = b"/* SPDX-License-Identifier: MIT */\n";
    let either: &[u8] = b"// SPDX-License-Identifier: MIT OR Apache-2.0\n";
    assert_eq!(build::licence_of(&[("a.c", mit), ("a.h", mit)]), "MIT");
    assert_eq!(build::licence_of(&[("a.c", either)]), "MIT OR Apache-2.0");
    let none: &[u8] = b"int x;";
    let twice: &[u8] = b"// SPDX-License-Identifier: MIT\n// SPDX-License-Identifier: MIT\n";
    let odd: &[u8] = b"// SPDX-License-Identifier: MIT; rm -rf\n";
    let empty: &[u8] = b"// SPDX-License-Identifier:\n";
    let binary: &[u8] = b"\xff SPDX-License-Identifier: MIT\n";
    for files in [
        &[("a.c", mit), ("b.c", either)][..],
        &[("a.c", mit), ("b.c", none)][..],
        &[("a.c", twice)][..],
        &[("a.c", odd)][..],
        &[("a.c", empty)][..],
        &[("a.c", binary)][..],
    ] {
        assert_eq!(build::licence_of(files), build::NO_LICENCE, "{files:?}");
    }
}
