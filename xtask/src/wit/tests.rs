use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

#[test]
fn the_tree_matches_its_frozen_copies() {
    let root = crate::workspace_root().join("wit");
    assert_eq!(check(&root), Ok(7));
}

const FROZEN: &str = "package demo:pkg@0.1.0;

interface api {
    record point { x: u32, y: u32 }
    /// Moves it.
    nudge: func(p: point) -> point;
}

world demo {
    export api;
}
";

/// A fresh `wit/` with `FROZEN` frozen and `now` as the package's text.
fn tree(now: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("kai-wit-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::create_dir_all(root.join("frozen")).unwrap();
    fs::write(root.join("frozen").join("demo-pkg@0.1.0.wit"), FROZEN).unwrap();
    fs::write(root.join("pkg").join("api.wit"), now).unwrap();
    root
}

fn verdict(now: &str) -> Result<usize, String> {
    let root = tree(now);
    let result = check(&root);
    let _ = fs::remove_dir_all(&root);
    result
}

#[test]
fn unchanged_or_reworded_packages_pass() {
    assert_eq!(verdict(FROZEN), Ok(1));
    let reworded = FROZEN.replace("/// Moves it.", "/// Moves the point by one.");
    assert_eq!(verdict(&reworded), Ok(1));
}

#[test]
fn additions_behind_a_later_since_pass() {
    let next = FROZEN.replace("@0.1.0;", "@0.1.1;");
    let added = next.replace(
        "    nudge: func",
        "    @since(version = 0.1.1)\n    reset: func() -> point;\n    nudge: func",
    );
    assert_eq!(verdict(&added), Ok(1));
    let world = next.replace(
        "    export api;",
        "    export api;\n    @since(version = 0.1.1)\n    import more: func();",
    );
    assert_eq!(verdict(&world), Ok(1));
}

fn refused(now: &str, words: &str) {
    let error = verdict(now).expect_err(now);
    assert!(error.contains(words), "{error}");
}

#[test]
fn anything_else_breaks_the_freeze() {
    refused(
        &FROZEN.replace(
            "    nudge: func",
            "    reset: func() -> point;\n    nudge: func",
        ),
        "reset was added without @since",
    );
    refused(
        &FROZEN.replace(
            "    nudge: func",
            "    @since(version = 0.1.0)\n    reset: func() -> point;\n    nudge: func",
        ),
        "not after the frozen version",
    );
    refused(
        &FROZEN.replace("y: u32 }", "y: u64 }"),
        "type point changed",
    );
    refused(
        &FROZEN.replace(
            "record point { x: u32, y: u32 }",
            "record point { y: u32, x: u32 }",
        ),
        "type point changed",
    );
    refused(&FROZEN.replace("-> point;", ";"), "nudge changed");
    refused(&FROZEN.replace("p: point", "q: point"), "nudge changed");
    refused(
        &FROZEN.replace(
            "    /// Moves it.\n    nudge: func(p: point) -> point;\n",
            "",
        ),
        "function nudge was removed",
    );
    refused(&FROZEN.replace("    export api;\n", ""), "was removed");
    refused(&FROZEN.replace("export api;", "import api;"), "was removed");
}

#[test]
fn a_package_without_a_frozen_copy_is_refused_until_frozen() {
    let root = tree(FROZEN);
    fs::remove_file(root.join("frozen").join("demo-pkg@0.1.0.wit")).unwrap();
    assert!(check(&root).unwrap_err().contains("no frozen copy"));
    let written = freeze(&root).unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(check(&root), Ok(1));
    // Freezing again leaves the copy alone.
    assert!(freeze(&root).unwrap().is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_new_major_needs_its_own_frozen_copy_and_the_old_one_its_package() {
    // 0.2 is a new line: it needs its own frozen copy, and 0.1 stays served.
    refused(&FROZEN.replace("@0.1.0;", "@0.2.0;"), "no frozen copy");
    let added = FROZEN.replace("@0.1.0;", "@0.1.1;").replace(
        "    nudge: func",
        "    reset: func() -> point;\n    nudge: func",
    );
    refused(&added, "reset was added without @since");
}
