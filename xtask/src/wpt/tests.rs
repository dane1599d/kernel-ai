//! The harness on small fixtures (`fixtures/`): a hand-written version-9
//! manifest and a few pages. No WPT is fetched.

use super::compare::{Difference, Picture, differences, equal, passes};
use super::manifest::{Fuzzy, FuzzyKey, Manifest, Reftest, Relation, parse};
use super::render::{Page, Renderer, Stub};
use super::run::{MAX_CHAIN, Status, Tally, file_of, group_of, run_one, select};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/wpt/fixtures")
}

fn fixture_manifest() -> Manifest {
    parse(&std::fs::read_to_string(fixtures().join("MANIFEST.json")).unwrap()).unwrap()
}

fn fuzzy(c: (u32, u32), p: (u64, u64)) -> Fuzzy {
    Fuzzy {
        per_channel: c,
        pixels: p,
    }
}

#[test]
fn the_manifest_gives_every_reftest_item_and_nothing_else() {
    let m = fixture_manifest();
    let urls: Vec<&str> = m.reftests.keys().map(String::as_str).collect();
    assert_eq!(
        urls,
        [
            "/css/box/blue.html",
            "/css/box/chain.html",
            "/css/box/green.html",
            "/css/box/red.html?a",
            "/css/box/red.html?b",
            "/html/top.html"
        ]
    );
    let green = &m.reftests["/css/box/green.html"];
    assert_eq!(green.path, "css/box/green.html");
    assert_eq!(
        green.references,
        [(String::from("/css/box/green-ref.html"), Relation::Match)]
    );
    assert!(green.fuzzy.is_empty() && green.viewport.is_none());
    let a = &m.reftests["/css/box/red.html?a"];
    assert_eq!(
        (a.path.as_str(), a.viewport),
        ("css/box/red.html", Some((320, 240)))
    );
    let b = &m.reftests["/css/box/red.html?b"];
    assert_eq!(b.references.len(), 2);
    assert_eq!(
        (b.viewport, b.viewport_unread.as_deref()),
        (None, Some("wide"))
    );
    let blue = &m.reftests["/css/box/blue.html"];
    assert_eq!(blue.references[0].1, Relation::Mismatch);
    assert_eq!(blue.fuzzy, [(FuzzyKey::Any, fuzzy((0, 2), (0, 40)))]);
    let chain = &m.reftests["/css/box/chain.html"];
    assert_eq!(
        chain.fuzzy[0].0,
        FuzzyKey::Ref(String::from("/css/box/green.html"))
    );
    assert_eq!(
        chain.fuzzy[1].0,
        FuzzyKey::Full(
            String::from("/css/box/chain.html"),
            String::from("/css/box/green.html"),
            Relation::Match
        )
    );
}

#[test]
fn a_manifest_it_cannot_read_is_an_error_naming_the_file() {
    assert!(parse("{").is_err());
    assert!(
        parse(r#"{"version": 8, "items": {}}"#)
            .unwrap_err()
            .contains("version")
    );
    let bad = |item: &str| {
        parse(&format!(
            r#"{{"version": 9, "items": {{"reftest": {{"a.html": ["h", {item}]}}}}}}"#
        ))
    };
    assert!(
        bad(r#"[null, [["/b.html", "<>"]], {}]"#)
            .unwrap_err()
            .starts_with("a.html: ")
    );
    assert!(bad(r#"[null, [["/b.html"]], {}]"#).is_err());
    assert!(
        bad(r#"[null, [], {"fuzzy": [[null, [[5, 2], [0, 1]]]]}]"#)
            .unwrap_err()
            .contains("backwards")
    );
    assert!(bad(r#"[null, [], {"fuzzy": [[[1, 2], [[0, 2], [0, 1]]]]}]"#).is_err());
    assert!(bad(r#"[null, []]"#).is_err());
    // No reftests at all is an empty manifest, not an error.
    assert!(
        parse(r#"{"version": 9, "items": {}}"#)
            .unwrap()
            .reftests
            .is_empty()
    );
}

#[test]
fn fuzzy_is_looked_up_as_wptrunner_does() {
    let m = fixture_manifest();
    let chain = &m.reftests["/css/box/chain.html"];
    let (from, to) = ("/css/box/chain.html", "/css/box/green.html");
    // The full key first, then the ref url, then the key for any.
    assert_eq!(
        chain.fuzzy_for(from, to, Relation::Match),
        Some(fuzzy((0, 3), (0, 7)))
    );
    assert_eq!(
        chain.fuzzy_for(from, to, Relation::Mismatch),
        Some(fuzzy((1, 9), (1, 100)))
    );
    assert_eq!(chain.fuzzy_for(from, "/other.html", Relation::Match), None);
    let blue = &m.reftests["/css/box/blue.html"];
    assert_eq!(
        blue.fuzzy_for("/css/box/blue.html", "/x.html", Relation::Match),
        Some(fuzzy((0, 2), (0, 40)))
    );
}

fn picture(pixels: &[[u8; 4]]) -> Picture {
    Picture {
        width: pixels.len() as u32,
        height: 1,
        rgba: pixels.concat(),
    }
}

#[test]
fn pictures_differ_by_their_largest_channel_and_their_count_of_pixels() {
    let a = picture(&[[10, 10, 10, 255], [0, 0, 0, 255], [9, 9, 9, 255]]);
    let b = picture(&[[13, 10, 9, 255], [0, 0, 0, 0], [9, 9, 9, 255]]);
    // Alpha is not compared, as wptrunner converts to RGB.
    assert_eq!(
        differences(&a, &b),
        Difference {
            per_channel: 3,
            pixels: 1,
            size: false
        }
    );
    let wide = Picture::solid(4, 1, [0, 0, 0, 255]);
    assert!(differences(&a, &wide).size);
    assert!(!equal(
        differences(&a, &wide),
        Some(fuzzy((0, 255), (0, u64::MAX)))
    ));
}

#[test]
fn equality_follows_the_fuzzy_rules() {
    let same = Difference {
        per_channel: 0,
        pixels: 0,
        size: false,
    };
    let near = Difference {
        per_channel: 3,
        pixels: 10,
        size: false,
    };
    // No allowance, or 0-0;0-0: exact.
    assert!(equal(same, None));
    assert!(!equal(near, None));
    assert!(!equal(near, Some(fuzzy((0, 0), (0, 0)))));
    // Inside both ranges.
    assert!(equal(near, Some(fuzzy((0, 3), (0, 10)))));
    assert!(!equal(near, Some(fuzzy((0, 2), (0, 10)))));
    assert!(!equal(near, Some(fuzzy((0, 3), (0, 9)))));
    // A range that does not start at 0 demands a difference...
    assert!(!equal(same, Some(fuzzy((1, 5), (1, 20)))));
    assert!(equal(near, Some(fuzzy((1, 5), (1, 20)))));
    // ...unless the other range does start at 0 (wptrunner's two shortcuts).
    assert!(equal(same, Some(fuzzy((1, 5), (0, 20)))));
    assert!(equal(same, Some(fuzzy((0, 5), (1, 20)))));
    // A mismatch passes exactly when the two are not equal.
    assert!(passes(near, None, Relation::Mismatch));
    assert!(!passes(same, None, Relation::Mismatch));
    assert!(passes(same, None, Relation::Match));
}

/// Draws each url as the picture set for it; any other url is an error.
struct Fake(HashMap<String, Picture>, Vec<String>);

impl Renderer for Fake {
    fn name(&self) -> &str {
        "fake"
    }
    fn render(&mut self, page: &Page) -> Result<Picture, String> {
        self.1.push(page.url.to_string());
        self.0
            .get(page.url)
            .cloned()
            .ok_or(format!("no picture for {}", page.url))
    }
}

fn test(url: &str, refs: &[(&str, Relation)]) -> Reftest {
    Reftest {
        path: url.trim_start_matches('/').to_string(),
        url: url.to_string(),
        references: refs.iter().map(|(u, r)| (u.to_string(), *r)).collect(),
        fuzzy: Vec::new(),
        viewport: None,
        viewport_unread: None,
    }
}

fn manifest(tests: Vec<Reftest>) -> Manifest {
    Manifest {
        url_base: String::from("/"),
        reftests: tests.into_iter().map(|t| (t.url.clone(), t)).collect(),
    }
}

#[test]
fn references_are_alternatives_and_chains_must_hold_all_the_way() {
    use Relation::Match as Eq;
    let (black, white) = (
        Picture::solid(2, 2, [0, 0, 0, 255]),
        Picture::solid(2, 2, [255; 4]),
    );
    let shots: HashMap<String, Picture> = [
        ("/t", &black),
        ("/white", &white),
        ("/black", &black),
        ("/r", &black),
        ("/r2", &black),
    ]
    .into_iter()
    .map(|(u, p)| (u.to_string(), p.clone()))
    .collect();
    let root = Path::new("/nowhere");
    // The first reference fails, the second passes: a pass.
    let t = test("/t", &[("/white", Eq), ("/black", Eq)]);
    let m = manifest(vec![t.clone()]);
    let mut fake = Fake(shots.clone(), Vec::new());
    assert_eq!(run_one(&m, root, &mut fake, &t).status, Status::Pass);
    // A chain: /t == /r passes, but /r == /white fails, and there is no other
    // way: a fail, its last comparison /r against /white.
    let t = test("/t", &[("/r", Eq)]);
    let m = manifest(vec![t.clone(), test("/r", &[("/white", Eq)])]);
    let out = run_one(&m, root, &mut Fake(shots.clone(), Vec::new()), &t);
    assert_eq!(out.status, Status::Fail);
    assert_eq!(
        out.last.as_ref().map(|(u, _, d)| (u.as_str(), d.pixels)),
        Some(("/white", 4))
    );
    // The same chain whose end holds: a pass.
    let m = manifest(vec![t.clone(), test("/r", &[("/r2", Eq)])]);
    assert_eq!(
        run_one(&m, root, &mut Fake(shots.clone(), Vec::new()), &t).status,
        Status::Pass
    );
    // A page that cannot be drawn is an error; so is a test with no references.
    let t = test("/t", &[("/gone", Eq)]);
    let out = run_one(
        &manifest(vec![t.clone()]),
        root,
        &mut Fake(shots.clone(), Vec::new()),
        &t,
    );
    assert!(matches!(out.status, Status::Error(ref why) if why.contains("/gone")));
    let t = test("/t", &[]);
    assert!(matches!(
        run_one(
            &manifest(vec![t.clone()]),
            root,
            &mut Fake(shots.clone(), Vec::new()),
            &t
        )
        .status,
        Status::Error(_)
    ));
    // A cycle ends as an error, not a hang; each page is drawn once.
    let t = test("/t", &[("/r", Eq)]);
    let m = manifest(vec![t.clone(), test("/r", &[("/t", Eq)])]);
    let mut fake = Fake(shots, Vec::new());
    let out = run_one(&m, root, &mut fake, &t);
    assert!(matches!(out.status, Status::Error(ref why) if why.contains(&MAX_CHAIN.to_string())));
    assert_eq!(fake.1, ["/t", "/r"]);
}

#[test]
fn the_stub_runs_the_fixtures_end_to_end() {
    let m = fixture_manifest();
    let tests = select(&m, &[], None);
    let mut stub = Stub;
    let outcomes: Vec<_> = tests
        .iter()
        .map(|t| run_one(&m, &fixtures(), &mut stub, t))
        .collect();
    let status: Vec<(&str, &str)> = outcomes
        .iter()
        .map(|o| (o.url.as_str(), o.status.word()))
        .collect();
    assert_eq!(
        status,
        [
            ("/css/box/blue.html", "PASS"),
            ("/css/box/chain.html", "FAIL"),
            ("/css/box/green.html", "PASS"),
            ("/css/box/red.html?a", "FAIL"),
            ("/css/box/red.html?b", "PASS"),
            ("/html/top.html", "ERROR"),
        ]
    );
    let report = super::report(&outcomes, "stub", &[]);
    assert!(
        report.contains("**3 of 6 passed (50.0%)**; 2 failed, 1 errors."),
        "{report}"
    );
    assert!(
        report.contains("| css/box | 3 | 2 | 0 | 60.0% |"),
        "{report}"
    );
    assert!(report.contains("| html | 0 | 0 | 1 | 0.0% |"), "{report}");
    let json: serde_json::Value =
        serde_json::from_str(&super::results_json(&outcomes, "stub")).unwrap();
    assert_eq!(json["tests"].as_array().unwrap().len(), 6);
    assert_eq!(json["tests"][1]["last"]["relation"], "==");
    assert_eq!(json["wpt_commit"], super::COMMIT);
    // red.html?b's viewport_size "wide" ran at the default and says so.
    assert!(
        report.contains("1 ran with a note (an unread `viewport_size`, say)"),
        "{report}"
    );
    assert_eq!(
        json["tests"][4]["note"],
        "viewport_size \"wide\" is not WxH: run at 800x600"
    );
    assert!(json["tests"][3].get("note").is_none());
}

/// WPT writes `viewport_size` as the meta tag's `WxH` text; `[w, h]` is read
/// too, and anything else is no viewport, never a manifest error.
#[test]
fn a_viewport_size_is_read_as_wxh() {
    use super::manifest::viewport_of;
    use serde_json::json;
    assert_eq!(viewport_of(&json!("320x240")), Some((320, 240)));
    assert_eq!(viewport_of(&json!(" 320 x 240 ")), Some((320, 240)));
    assert_eq!(viewport_of(&json!([320, 240])), Some((320, 240)));
    for odd in [
        json!("wide"),
        json!("320"),
        json!("0x240"),
        json!("320x240x2"),
        json!("-1x2"),
        json!([320]),
        json!(320),
    ] {
        assert_eq!(viewport_of(&odd), None, "{odd}");
    }
}

#[test]
fn filters_and_limits_pick_tests_in_path_order() {
    let m = fixture_manifest();
    let urls = |t: Vec<&Reftest>| t.iter().map(|t| t.url.clone()).collect::<Vec<_>>();
    assert_eq!(
        urls(select(&m, &[String::from("html/")], None)),
        ["/html/top.html"]
    );
    assert_eq!(
        urls(select(&m, &[String::from("css/box/r")], None)),
        ["/css/box/red.html?a", "/css/box/red.html?b"]
    );
    assert_eq!(select(&m, &[], Some(2)).len(), 2);
}

#[test]
fn urls_name_files_and_paths_name_groups() {
    let root = Path::new("/wpt");
    assert_eq!(
        file_of(root, "/", "/css/a/b.html?x=1#top"),
        Path::new("/wpt/css/a/b.html")
    );
    assert_eq!(
        file_of(root, "/base/", "/base/c.html"),
        Path::new("/wpt/c.html")
    );
    assert_eq!(group_of("css/css-flexbox/a/b.html"), "css/css-flexbox");
    assert_eq!(group_of("html/top.html"), "html");
    assert_eq!(group_of("top.html"), ".");
    let mut t = Tally::default();
    for s in [Status::Pass, Status::Pass, Status::Fail] {
        t.add(&s);
    }
    assert_eq!((t.total(), t.rate()), (3, 66.6));
}
