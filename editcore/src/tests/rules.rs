use super::*;
use crate::EDITABLE;
use crate::rules::{check, check_line, check_path};

fn with_lib(lib: &str) -> Vec<u8> {
    small(lib)
}

#[test]
fn paths_must_be_plain_relative_paths() {
    for good in ["components/notes/src/lib.rs", "a/b.rs", "x"] {
        assert!(check_path(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "/etc/passwd",
        "a/../b",
        "a/./b",
        "a//b",
        "a\\b",
        "C:/x",
        "c:x",
        "a/b/",
        "a/\x07",
    ] {
        assert!(check_path(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn files_outside_the_component_or_a_changed_file_set_are_refused() {
    let notes = &EDITABLE[0];
    let base = with_lib("fn a() {}\n");
    let outside = bundle(&[
        (TOML, b"[package]\nname = \"comp-notes\"\n"),
        ("kernel/src/main.rs", b""),
    ]);
    assert!(
        check(notes, &base, &outside)
            .unwrap_err()
            .contains("outside")
    );
    let added = bundle(&[
        (TOML, b"[package]\nname = \"comp-notes\"\n"),
        (LIB, b"fn a() {}\n"),
        ("components/notes/src/new.rs", b""),
    ]);
    assert!(
        check(notes, &base, &added)
            .unwrap_err()
            .contains("added, removed")
    );
    let removed = bundle(&[(TOML, b"[package]\nname = \"comp-notes\"\n")]);
    assert!(check(notes, &base, &removed).is_err());
    let dotted = bundle(&[(TOML, b""), ("components/notes/../../x.rs", b"")]);
    assert!(check(notes, &dotted, &dotted).is_err());
}

#[test]
fn only_rust_files_under_src_may_change() {
    let notes = &EDITABLE[0];
    let base = with_lib("fn a() {}\n");
    let toml = bundle(&[
        (TOML, b"[package]\nname = \"evil\"\n"),
        (LIB, b"fn a() {}\n"),
    ]);
    assert!(
        check(notes, &base, &toml)
            .unwrap_err()
            .contains("Cargo.toml cannot change")
    );
    let other = bundle(&[(TOML, b""), ("components/notes/src/data.txt", b"a")]);
    let other_edit = bundle(&[(TOML, b""), ("components/notes/src/data.txt", b"b")]);
    assert!(check(notes, &other, &other_edit).is_err());
    let edited = with_lib("fn a() { let x = 1; }\n");
    assert_eq!(check(notes, &base, &edited), Ok(()));
}

#[test]
fn changed_lines_may_not_read_files_or_the_environment() {
    for bad in [
        r#"const X: &str = include_str!("/etc/passwd");"#,
        "include !(\"x.rs\");",
        "use core::include_bytes as x;",
        r#"const H: &str = env!("HOME");"#,
        r#"const H: Option<&str> = option_env!("PATH");"#,
        r#"#[path = "/tmp/x.rs"] mod x;"#,
        r#"# [ path = "/tmp/x.rs" ] mod x;"#,
        r#"core::arch::global_asm!(".incbin \"/etc/passwd\"");"#,
        "let r#include = 1;",
    ] {
        assert!(check_line(bad).is_err(), "{bad}");
    }
    for good in [
        r#""about" => format!("ok notes {VERSION} schema {SCHEMA}, edited live"),"#,
        "self.state.next_id += \"1\";",
        "let environment = 1; // not the word",
        "#[derive(Debug)]",
        // An inner attribute reads no host file unless it names a denied word.
        "#![allow(unused)]",
    ] {
        assert!(check_line(good).is_ok(), "{good}");
    }
}

#[test]
fn the_unchanged_option_env_line_is_fine_but_a_new_one_is_not() {
    let notes = &EDITABLE[0];
    let base = with_lib("a\noption_env!(\"KAI_VERSION\")\nb\n");
    // Edits on both sides of it make one region that carries the line over.
    let edited = with_lib("A\noption_env!(\"KAI_VERSION\")\nB\n");
    assert_eq!(check(notes, &base, &edited), Ok(()));
    let doubled = with_lib("a\noption_env!(\"KAI_VERSION\")\noption_env!(\"KAI_VERSION\")\nb\n");
    assert!(check(notes, &base, &doubled).is_err());
}

#[test]
fn a_path_attribute_is_refused_however_it_is_written() {
    for bad in [
        r#"#[cfg_attr(all(), path = "C:/x.rs")] mod leak;"#,
        r#"#[/**/path = "/tmp/x.rs"] mod x;"#,
        r#"#[r#path = "/tmp/x.rs"] mod x;"#,
        "let a = 1;\u{200e}",
        "let a = 1;\u{c}",
        "let a = 1;\t",
        "let s = \"\u{e9}\";",
        "let s = \"a\u{200b}b\";",
    ] {
        assert!(check_line(bad).is_err(), "{bad}");
    }
    let notes = &EDITABLE[0];
    let base = with_lib("fn a() {}\n");
    let split = "fn a() {}\n#\n[path = \"/tmp/x.rs\"]\nmod x;\n";
    assert!(check(notes, &base, &with_lib(split)).is_err());
}
