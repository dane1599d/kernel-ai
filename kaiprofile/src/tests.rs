use super::*;
use alloc::vec;

const GAMING: &str = "kai-profile 1\nname gaming\nfocused class=normal mem=- gpu=0\ngroup builds members=build-*+indexer class=background mem=65536 gpu=0\nothers class=background mem=- gpu=0\n";

#[test]
fn a_profile_reads_and_writes_back_the_same() {
    let p = parse(GAMING).unwrap();
    assert_eq!(p.name, "gaming");
    assert_eq!(p.lines.len(), 3);
    assert_eq!(
        p.lines[1],
        Line {
            target: Target::Group {
                name: "builds".into(),
                members: vec!["build-*".into(), "indexer".into()],
            },
            class: Class::Background,
            mem: Some(65536),
        }
    );
    assert_eq!(p.text(), GAMING);
}

#[test]
fn every_builtin_writes_text_that_reads_back() {
    let names: Vec<String> = builtin().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["default", "gaming", "ai-dev", "quiet"]);
    for p in builtin() {
        assert_eq!(parse(&p.text()).unwrap(), p, "{}", p.name);
    }
}

#[test]
fn the_most_specific_line_wins_and_the_system_is_left_alone() {
    let p = parse(&GAMING.replace(
        "focused class=normal",
        "app notes class=normal mem=2048 gpu=0\nfocused class=normal",
    ))
    .unwrap();
    let running = [
        "shell", "llm", "notes", "game", "build-3", "indexer", "mail",
    ];
    let set = p.resolve(&running, Some("game"));
    let got: Vec<(&str, Class, Option<u64>)> = set
        .iter()
        .map(|s| (s.process.as_str(), s.class, s.mem))
        .collect();
    assert_eq!(
        got,
        [
            ("notes", Class::Normal, Some(2048)),
            ("game", Class::Normal, None),
            ("build-3", Class::Background, Some(65536)),
            ("indexer", Class::Background, Some(65536)),
            ("mail", Class::Background, None),
        ]
    );
    // With nothing focused, the game is one of the others.
    let set = p.resolve(&["game"], None);
    assert_eq!(set[0].class, Class::Background);
}

#[test]
fn a_profile_out_of_shape_is_refused_whole() {
    let gaming = |from: &str, to: &str| GAMING.replace(from, to);
    for (bad, line) in [
        (String::from("kai-profile 2\nname x\n"), 1),
        (String::from("kai-profile 1\nname Gaming\n"), 2),
        (String::from("kai-profile 1\n"), 2),
        (gaming("gpu=0\nothers", "gpu=1\nothers"), 4),
        (
            gaming("class=background mem=- gpu=0\n", "class=firm mem=- gpu=0\n"),
            5,
        ),
        (gaming("mem=65536", "mem=63"), 4),
        (gaming("mem=65536", "mem=065536"), 4),
        (gaming("mem=65536 gpu=0", "gpu=0 mem=65536"), 4),
        (gaming("members=build-*+indexer", "members=build-*+llm"), 4),
        (gaming("others class", "app shell class"), 5),
        (gaming("others class", "focused class"), 5),
        (
            gaming(
                "others class=background mem=- gpu=0\n",
                "others class=background mem=- gpu=0",
            ),
            5,
        ),
        (gaming("others", "everyone"), 5),
        (gaming("focused class", "focused  class"), 3),
    ] {
        assert_eq!(parse(&bad).map_err(|e| e.line), Err(line), "{bad:?}");
    }
}
