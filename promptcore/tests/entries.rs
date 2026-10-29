//! Entries as compile units: the five conveniences, the prelude, definitions,
//! undo, replay and the shape errors.

use promptcore::lex::{Kind as Tok, LexError, lex, open};
use promptcore::prepare::{LineMap, Mode};
use promptcore::{Def, Defs, Error, Kind, Prepared};
fn prep(defs: &Defs, seq: u64, entry: &str) -> Prepared {
    defs.prepare(seq, entry, Mode::Eval)
        .unwrap_or_else(|e| panic!("{entry:?}: {e:?}"))
}

/// Prepares and commits `entry`, as `cc` does once it ran.
fn commit(defs: &mut Defs, seq: u64, entry: &str) -> Prepared {
    let p = prep(defs, seq, entry);
    defs.commit(seq, p.defs.clone(), &p.drops);
    p
}

/// The run function's body.
fn body(p: &Prepared) -> &str {
    let run = p.run.as_deref().expect("a run function");
    let start = p.source.find(&format!("void {run}(void) {{\n")).unwrap();
    let body = &p.source[start..];
    &body[body.find('\n').unwrap() + 1..body.rfind('}').unwrap()]
}

fn keys(defs: &Defs) -> Vec<String> {
    defs.iter().flat_map(|d| d.keys.iter().cloned()).collect()
}

#[test]
fn lexes_words_literals_and_directives() {
    let src =
        "#define N \\\n  3\nI64 x = u8\"a\\\"b\" + 'c'; // note\n/* a\n b */ x->y >>= 1.5e+3;";
    let tokens = lex(src).unwrap();
    let texts: Vec<&str> = tokens.iter().map(|t| t.text(src)).collect();
    assert_eq!(
        texts,
        [
            "#define N \\\n  3",
            "I64",
            "x",
            "=",
            "u8\"a\\\"b\"",
            "+",
            "'c'",
            ";",
            "x",
            "->",
            "y",
            ">>=",
            "1.5e+3",
            ";"
        ]
    );
    assert_eq!(tokens[0].kind, Tok::Directive);
    assert_eq!(tokens[4].kind, Tok::Str);
    assert_eq!(tokens[1].line, 3);
    assert_eq!(tokens[8].line, 5);
}

#[test]
fn lex_refuses_with_lines() {
    assert_eq!(lex("x;\n/* never"), Err(LexError::OpenComment { line: 2 }));
    assert_eq!(lex("\n\"abc\nx"), Err(LexError::OpenQuote { line: 2 }));
    assert_eq!(
        lex("x;\n\nx @ y;"),
        Err(LexError::Stray {
            line: 3,
            byte: b'@'
        })
    );
    // UTF-8 is fine inside literals and comments, not outside.
    assert!(lex("Print(\"π\"); // é").is_ok());
    assert!(matches!(
        lex("I64 π;"),
        Err(LexError::Stray { line: 1, .. })
    ));
    // A `#` that does not start a line is a punctuator, and an apostrophe in a
    // directive does not fail the lexer.
    assert_eq!(lex("x # y").unwrap()[1].kind, Tok::Punct);
    assert!(lex("#error don't\nx;").is_ok());
}

#[test]
fn open_entries_ask_for_more() {
    for entry in [
        "I64 F(I64 x) {",
        "if (x) {\n  Print(\"a\");",
        "Print((1",
        "/* a comment",
        "#define TWICE(x) \\",
    ] {
        assert!(open(entry), "{entry:?}");
    }
    for entry in [
        "Print(\"{\");",
        "x = 1;",
        "}",
        "Print(\"open",
        "I64 F(I64 x) { return x; }",
        "",
    ] {
        assert!(!open(entry), "{entry:?}");
    }
}

#[test]
fn a_statement_runs_in_the_entrys_function() {
    let defs = Defs::new();
    let p = prep(&defs, 7, "Print(\"hi\\n\");");
    assert_eq!(p.run.as_deref(), Some("__kai_run_7"));
    assert!(p.source.starts_with("#include <kai.h>\n"));
    assert_eq!(body(&p), "Print(\"hi\\n\");\n");
    assert!(p.defs.is_empty());
    assert_eq!(p.fallback, None);
}

#[test]
fn a_last_expression_without_a_semicolon_is_shown() {
    let defs = Defs::new();
    let p = prep(&defs, 1, "1 + 2");
    assert_eq!(body(&p), "__kai_show((1 + 2));\n");
    let fallback = p.fallback.as_deref().unwrap();
    assert!(fallback.contains("\n1 + 2;\n}\n"), "{fallback}");
    // With its semicolon it is a plain statement.
    let p = prep(&defs, 2, "1 + 2;");
    assert_eq!(body(&p), "1 + 2;\n");
    assert_eq!(p.fallback, None);
}

#[test]
fn a_string_statement_prints() {
    let defs = Defs::new();
    assert_eq!(
        body(&prep(&defs, 1, "\"x=%d\\n\", 3;")),
        "Print(\"x=%d\\n\", 3);\n"
    );
    assert_eq!(body(&prep(&defs, 2, "\"hi\"")), "Print(\"hi\");\n");
}

#[test]
fn a_function_is_a_definition_with_a_prototype() {
    let mut defs = Defs::new();
    let p = commit(&mut defs, 1, "I64 Sq(I64 x) { return x * x; }");
    assert_eq!(p.run, None);
    assert_eq!(p.defs.len(), 1);
    let sq = &p.defs[0];
    assert_eq!((sq.kind, sq.name.as_str()), (Kind::Func, "Sq"));
    assert_eq!(sq.text, "I64 Sq(I64 x);\n");
    assert!(!sq.nullary);
    // Later entries see its prototype.
    let p = prep(&defs, 2, "Sq(4)");
    assert!(p.source.contains("I64 Sq(I64 x);\n"));
    assert_eq!(body(&p), "__kai_show((Sq(4)));\n");
}

#[test]
fn a_lone_name_calls_its_function() {
    let mut defs = Defs::new();
    let p = commit(&mut defs, 1, "U0 Hi() { Print(\"hi\"); }");
    assert!(p.defs[0].nullary);
    assert_eq!(body(&prep(&defs, 2, "Hi;")), "Hi();\n");
    let shown = prep(&defs, 3, "Hi");
    assert_eq!(body(&shown), "__kai_show((Hi()));\n");
    assert!(shown.fallback.unwrap().contains("\nHi();\n"));
    // A function defined in the same entry counts too; a variable does not.
    let p = prep(&defs, 4, "Bye;\nU0 Bye(void) { }");
    assert_eq!(body(&p), "Bye();\n");
    assert!(defs.get("Bye").is_none());
    commit(&mut defs, 5, "I64 n = 2;");
    assert_eq!(body(&prep(&defs, 6, "n;")), "n;\n");
}

#[test]
fn a_global_with_a_plain_value_is_assigned_where_it_stands() {
    let mut defs = Defs::new();
    commit(&mut defs, 1, "I64 Sq(I64 x) { return x * x; }");
    let p = commit(
        &mut defs,
        2,
        "Print(\"a\");\nI64 y = Sq(3);\nPrint(\"%d\", y);",
    );
    assert!(p.source.contains("\nI64 y;\n"), "{}", p.source);
    assert_eq!(body(&p), "Print(\"a\");\ny = Sq(3);\nPrint(\"%d\", y);\n");
    assert_eq!(defs.get("y").unwrap().text, "extern I64 y;\n");
    // `const`, arrays and braces keep a static initializer.
    for entry in [
        "const I64 k = 5;",
        "I64 xs[3] = {1, 2, 3};",
        "I64 *p = 0, q = 1;",
    ] {
        let p = prep(&defs, 3, entry);
        let kept = entry.contains("const") || entry.contains('[');
        assert_eq!(p.run.is_none(), kept, "{entry}: {}", p.source);
    }
    let p = prep(&defs, 4, "I64 *p = 0, q = 1;");
    assert_eq!(body(&p), "p = 0;\nq = 1;\n");
    assert!(p.source.contains("\nI64 *p;\nI64 q;\n"), "{}", p.source);
}

#[test]
fn types_and_typedefs_join_the_prelude() {
    let mut defs = Defs::new();
    let p = commit(&mut defs, 1, "struct Point { I64 x, y; };");
    assert_eq!(p.defs[0].keys, ["tag Point"]);
    assert_eq!(p.defs[0].name, "struct Point");
    let p = commit(&mut defs, 2, "struct Point p = {1, 2};");
    assert_eq!(p.run, None);
    assert_eq!(defs.get("p").unwrap().text, "extern struct Point p;\n");
    commit(&mut defs, 3, "typedef struct { I64 x; } Pt;");
    assert!(defs.is_type("Pt"));
    // `Pt q;` reads as a declaration only because Pt is now a type.
    let p = commit(&mut defs, 4, "Pt q;");
    assert_eq!(p.defs[0].kind, Kind::Var);
    // Two names start a declaration even when the type is unknown here.
    let p = prep(&defs, 5, "Foo bar;");
    assert_eq!(p.defs[0].text, "extern Foo bar;\n");
    // A tagged body with a declarator is split into the type and the variable.
    let p = prep(&defs, 6, "struct Size { I64 w; } s;");
    assert_eq!(p.defs[0].text, "struct Size { I64 w; };\n");
    assert_eq!(p.defs[1].text, "extern struct Size s;\n");
}

#[test]
fn enum_constants_are_names() {
    let mut defs = Defs::new();
    commit(
        &mut defs,
        1,
        "enum Color { RED, GREEN = 2 * (1 + 0), BLUE };",
    );
    assert_eq!(keys(&defs), ["tag Color", "RED", "GREEN", "BLUE"]);
    // Another enum that reuses RED replaces Color, so the prelude never declares
    // RED twice.
    commit(&mut defs, 2, "enum Light { RED };");
    assert_eq!(keys(&defs), ["tag Light", "RED"]);
    commit(&mut defs, 3, "enum { ON, OFF };");
    assert_eq!(defs.get("OFF").unwrap().text, "enum { ON, OFF };\n");
}

#[test]
fn a_redefinition_leaves_the_old_one_out_of_the_prelude() {
    let mut defs = Defs::new();
    commit(&mut defs, 1, "I64 Sq(I64 x) { return x * x; }");
    commit(&mut defs, 2, "I64 Twice(I64 x) { return Sq(x) + Sq(x); }");
    let p = prep(&defs, 3, "F64 Sq(F64 x) { return x * x; }");
    assert!(!p.source.contains("I64 Sq(I64 x);"), "{}", p.source);
    assert!(p.source.contains("I64 Twice(I64 x);"));
    defs.commit(3, p.defs, &p.drops);
    // The new one goes last.
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["Twice", "Sq"]);
}

#[test]
fn undo_reverts_the_last_entry_in_place() {
    let mut defs = Defs::new();
    commit(&mut defs, 1, "I64 a = 1;");
    commit(&mut defs, 2, "I64 b = 2;");
    commit(&mut defs, 3, "I64 c = 3;");
    commit(&mut defs, 4, "F64 b = 2.5;");
    commit(&mut defs, 5, "Print(\"no definitions\");");
    let undone = defs.undo().unwrap();
    assert_eq!((undone.seq, undone.dropped.len()), (5, 0));
    let undone = defs.undo().unwrap();
    assert_eq!(undone.seq, 4);
    assert_eq!(undone.dropped[0].text, "extern F64 b;\n");
    assert_eq!(undone.restored[0].text, "extern I64 b;\n");
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c"]);
    for _ in 0..3 {
        defs.undo().unwrap();
    }
    assert!(defs.is_empty());
    assert_eq!(defs.undo(), None);
}

#[test]
fn undo_goes_back_at_most_its_limit() {
    let mut defs = Defs::new();
    for seq in 0..promptcore::defs::MAX_UNDO as u64 + 10 {
        commit(&mut defs, seq, &format!("I64 v{seq} = 1;"));
    }
    let mut undone = 0;
    while defs.undo().is_some() {
        undone += 1;
    }
    assert_eq!(undone, promptcore::defs::MAX_UNDO);
    assert_eq!(defs.len(), 10);
}

#[test]
fn replay_keeps_definitions_and_runs_nothing() {
    let mut defs = Defs::new();
    commit(&mut defs, 1, "I64 Sq(I64 x) { return x * x; }");
    let p = defs
        .prepare(2, "I64 y = Sq(3);\nI64 z = 4;\nPrint(\"x\");", Mode::Replay)
        .unwrap();
    assert_eq!(p.run, None);
    assert!(p.source.contains("\nI64 y;\n"));
    assert!(p.source.contains("\nI64 z;\n"));
    assert!(!p.source.contains("Print(\"x\")"));
    assert_eq!(p.defs.len(), 2);
}

#[test]
fn macros_and_includes() {
    let mut defs = Defs::new();
    let p = commit(&mut defs, 1, "#define N 3\n#include <string.h>\nI64 xs[N];");
    let kinds: Vec<Kind> = p.defs.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, [Kind::Macro, Kind::Include, Kind::Var]);
    assert_eq!(defs.get("#N").unwrap().name, "#define N");
    assert!(defs.get("<string.h>").is_some());
    let p = commit(&mut defs, 2, "#undef N");
    assert_eq!(p.drops, ["#N"]);
    assert!(defs.get("#N").is_none());
    for (entry, what) in [
        (
            "#include \"x.h\"",
            "only #include <…> works at c>; files come with :load",
        ),
        (
            "#if 1\nx;\n#endif",
            "only #define, #undef and #include <…> work at the top of an entry",
        ),
        ("#include <../secret>", "not a header name"),
        ("#include </abs.h>", "not a header name"),
        ("#define 9x", "the directive needs a name"),
    ] {
        assert_eq!(
            defs.prepare(3, entry, Mode::Eval),
            Err(Error::Syntax { line: 1, what }),
            "{entry}"
        );
    }
}

#[test]
fn static_names_stay_in_their_entry() {
    let mut defs = Defs::new();
    commit(&mut defs, 1, "I64 helper = 1;");
    let p = commit(
        &mut defs,
        2,
        "static I64 helper(I64 x) { return x; }\nstatic I64 k = 2;",
    );
    assert!(p.defs.is_empty());
    // The prelude leaves out the global it would clash with.
    assert!(!p.source.contains("extern I64 helper;"));
    assert!(p.source.contains("static I64 k = 2;"));
    assert!(defs.get("helper").is_some());
}

#[test]
fn control_statements_split_where_they_end() {
    let defs = Defs::new();
    let p = prep(
        &defs,
        1,
        "if (1) Print(\"a\"); else if (2) { Print(\"b\"); } else Print(\"c\");\n\
         do { Print(\"d\"); } while (0);\n\
         for (I64 i = 0; i < 3; i++) Print(\"%d\", i)",
    );
    assert_eq!(
        body(&p),
        "if (1) Print(\"a\"); else if (2) { Print(\"b\"); } else Print(\"c\");\n\
         do { Print(\"d\"); } while (0);\n\
         for (I64 i = 0; i < 3; i++) Print(\"%d\", i);\n"
    );
    let p = prep(
        &defs,
        2,
        "while (0) {}\nswitch (1) { case 1: break; }\nreturn;",
    );
    assert_eq!(p.fallback, None);
}

#[test]
fn shape_errors_name_their_line() {
    let defs = Defs::new();
    for (entry, line, what) in [
        ("x = 1\n#define N 2", 1, "a ; is missing at the end of this"),
        ("Print(\"a\");\nelse x;", 2, "else without if"),
        ("x;\n}", 2, "a closing bracket with nothing open"),
        ("f((1]);", 1, "the brackets do not match"),
        ("do x; y;", 1, "do needs its while"),
        ("if x;", 1, "( is missing"),
        ("I64 = 3;", 1, "a declaration needs a name"),
        ("typedef I64 F(void) { }", 1, "a typedef has no body"),
        (
            "I64 a, b(void) { }",
            1,
            "a { where a declaration should end",
        ),
        ("I64 x = ;", 1, "= needs a value"),
        ("if (1)", 1, "a statement is missing"),
    ] {
        assert_eq!(
            defs.prepare(1, entry, Mode::Eval),
            Err(Error::Syntax { line, what }),
            "{entry}"
        );
    }
    let long = "x;".repeat(promptcore::MAX_ENTRY / 2 + 1);
    assert_eq!(defs.prepare(1, &long, Mode::Eval), Err(Error::TooLong));
    let deep = "if (1) ".repeat(100) + "x;";
    assert!(matches!(
        defs.prepare(1, &deep, Mode::Eval),
        Err(Error::Syntax {
            what: "statements nest too deeply",
            ..
        })
    ));
}

#[test]
fn unit_lines_map_back_to_the_entry() {
    let mut defs = Defs::new();
    commit(&mut defs, 1, "I64 a = 1;\nI64 b = 2;");
    let p = prep(
        &defs,
        2,
        "I64 F(void) {\n  return 1;\n}\nPrint(\"x\");\nF()",
    );
    let unit: Vec<&str> = p.source.lines().collect();
    let at = |text: &str| unit.iter().position(|l| l.contains(text)).unwrap() as u32 + 1;
    assert_eq!(p.lines.entry_line(at("return 1;")), Some(2));
    assert_eq!(p.lines.entry_line(at("Print(\"x\")")), Some(4));
    assert_eq!(p.lines.entry_line(at("__kai_show")), Some(5));
    // The prelude and the wrappers are not the entry's.
    assert_eq!(p.lines.entry_line(at("extern I64 a;")), None);
    assert_eq!(p.lines.entry_line(at("void __kai_run_2")), None);
    assert_eq!(LineMap::default().entry_line(1), None);
}

#[test]
fn defs_lists_and_resets() {
    let mut defs = Defs::new();
    commit(
        &mut defs,
        1,
        "#define TWICE(x) \\\n ((x) * 2)\nI64 Sq(I64 x) { return x * x; }\nI64 n = 3;",
    );
    assert_eq!(
        defs.listing(),
        "macro   #define TWICE(x) \\ …\nfn      I64 Sq(I64 x);\nvar     I64 n;\n"
    );
    defs.clear();
    assert!(defs.is_empty() && !defs.can_undo());
}

#[test]
fn a_full_session_refuses_more() {
    let mut defs = Defs::new();
    let defs_list: Vec<Def> = (0..promptcore::defs::MAX_DEFS)
        .map(|i| Def {
            keys: vec![format!("v{i}")],
            kind: Kind::Var,
            name: format!("v{i}"),
            text: format!("extern I64 v{i};\n"),
            nullary: false,
            seq: 1,
        })
        .collect();
    defs.commit(1, defs_list, &[]);
    assert_eq!(
        defs.prepare(2, "I64 one_more;", Mode::Eval),
        Err(Error::Full)
    );
    // A statement adds nothing, so it still runs.
    assert!(defs.prepare(3, "Print(\"x\");", Mode::Eval).is_ok());
}

/// An entry cannot change how later units compile: no directive inside
/// brackets (it would be copied into a prototype or a body the prelude
/// repeats), no backslash at its end, no name the wire cannot carry.
#[test]
fn an_entry_stays_in_its_place() {
    let defs = Defs::new();
    let inside = "a directive inside brackets; put it at the top of the entry";
    for (entry, line, what) in [
        (
            "I64 f(I64 a\n#define Print(...) 0\n) { return a; }",
            2,
            inside,
        ),
        ("U0 g() {\n#define X 1\n}", 2, inside),
        ("struct S {\n#if 0\n I64 x; };", 2, inside),
        ("Print(\"a\"\n#define Y\n);", 2, inside),
        ("#define X 1 \\", 1, "the entry ends in a backslash"),
        (
            "x;\n#define X 1 \\\n  \n",
            2,
            "the entry ends in a backslash",
        ),
    ] {
        assert_eq!(
            defs.prepare(1, entry, Mode::Eval),
            Err(Error::Syntax { line, what }),
            "{entry}"
        );
    }
    let long = format!("I64 {} = 1;", "a".repeat(64));
    assert_eq!(
        defs.prepare(1, &long, Mode::Eval),
        Err(Error::Lex(LexError::LongName { line: 1 }))
    );
    assert!(
        defs.prepare(1, &format!("I64 {} = 1;", "a".repeat(63)), Mode::Eval)
            .is_ok()
    );
}

#[test]
fn a_line_comment_continues_after_a_backslash() {
    // The compiler splices the line after a backslash onto the comment, so
    // `y = 2;` is part of it.
    let src = "x = 1; // a \\\ny = 2;\nz = 3;";
    let texts: Vec<&str> = lex(src).unwrap().iter().map(|t| t.text(src)).collect();
    assert_eq!(texts, ["x", "=", "1", ";", "z", "=", "3", ";"]);
    assert_eq!(lex(src).unwrap()[4].line, 3);
    let directive = "#define N 3 // three \\\nfour\nx;";
    assert_eq!(
        lex(directive).unwrap()[0].text(directive),
        "#define N 3 // three \\\nfour"
    );
}
