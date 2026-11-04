//! What is indexed: `/home/user` only, by whole components, with excluded
//! folders; paths that climb out are refused before any test of where they
//! are.

use super::{CORPUS, OUTSIDE};
use crate::Estimate;
use crate::index::prepare;
use crate::scope::{self, HOME, Scope, Skip};

#[test]
fn a_path_is_clean_only_when_absolute_with_no_empty_dot_or_dotdot_part() {
    for path in [
        "/",
        "/home/user",
        "/home/user/a b/c.txt",
        "/home/user/.config/x",
    ] {
        assert!(scope::clean(path), "{path}");
    }
    for path in [
        "",
        "home/user",
        "/home/user/",
        "/home//user",
        "/home/user/./a",
        "/home/user/../user/a",
        "/home/user/a\0b",
        "/home/user/a\nb",
    ] {
        assert!(!scope::clean(path), "{path:?}");
    }
    let long = alloc::format!("/home/user/{}", "a".repeat(scope::MAX_PATH));
    assert!(!scope::clean(&long));
}

#[test]
fn only_files_under_home_are_indexed_never_system_ones() {
    let scope = Scope::new();
    for doc in CORPUS {
        assert_eq!(scope.check(doc.path), Ok(()), "{}", doc.path);
    }
    let want = [
        Skip::Outside,
        Skip::Outside,
        Skip::Outside,
        Skip::Unclean,
        Skip::Unclean,
        Skip::Unclean,
        Skip::Outside,
    ];
    for (path, want) in OUTSIDE.iter().zip(want) {
        assert_eq!(scope.check(path), Err(want), "{path}");
    }
}

#[test]
fn an_excluded_folder_and_everything_under_it_is_left_out_and_nothing_else() {
    let mut scope = Scope::new();
    scope.exclude("/home/user/diary").unwrap();
    assert_eq!(
        scope.check("/home/user/diary/2026-11-03.txt"),
        Err(Skip::Excluded)
    );
    assert_eq!(
        scope.check("/home/user/diary/old/a.txt"),
        Err(Skip::Excluded)
    );
    assert_eq!(scope.check("/home/user/diary"), Err(Skip::Excluded));
    // A folder whose name starts the same is not under it.
    assert_eq!(scope.check("/home/user/diary-public/a.txt"), Ok(()));
    // Excluding the same folder twice keeps one.
    scope.exclude("/home/user/diary").unwrap();
    assert_eq!(scope.excluded().len(), 1);
    // Only folders under home can be excluded, and only clean ones.
    assert_eq!(scope.exclude("/system"), Err(Skip::Outside));
    assert_eq!(scope.exclude("/home/user/../system"), Err(Skip::Unclean));
    // Excluding home itself leaves nothing to index.
    scope.exclude(HOME).unwrap();
    assert_eq!(scope.check("/home/user/notes/trip.md"), Err(Skip::Excluded));
}

#[test]
fn a_file_that_is_not_text_or_is_too_large_is_skipped() {
    let scope = Scope::new();
    let path = "/home/user/photo.jpg";
    let prepared = |bytes: &[u8]| prepare(&scope, path, 1, 1, 0, bytes, &Estimate, None);
    assert_eq!(prepared(&[0xff, 0xd8, 0xff, 0xe0]), Err(Skip::NotText));
    assert_eq!(prepared(b"text\0with a NUL"), Err(Skip::NotText));
    let big = alloc::vec![b'a'; crate::index::MAX_FILE as usize + 1];
    assert_eq!(prepared(&big), Err(Skip::TooLarge));
    assert!(prepared(b"plain text").is_ok());
    // A path outside home is refused before the bytes are looked at.
    let outside = prepare(&scope, "/system/log", 1, 1, 0, b"x", &Estimate, None);
    assert_eq!(outside, Err(Skip::Outside));
}
