//! The grounding rule against text nobody wrote carefully: a model's host argument,
//! a user's line, a page's address. Whatever the text is, nothing panics, a host
//! is only ever one in its normal form, and a fetch runs only for a host the data
//! says is grounded.

use nethosts::{
    Bytes, Grants, Ground, Grounding, Host, MAX_TYPED, NoGrants, Origin, Source, Table, Ungrounded,
    Verdict, typed_hosts,
};

/// A small deterministic generator, so a failure repeats.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// Text from an alphabet of host characters, separators, and the characters
    /// a host is attacked with.
    fn text(&mut self, max: usize) -> String {
        const PARTS: &[&str] = &[
            "a",
            "b",
            "z",
            "A",
            "Z",
            "0",
            "9",
            "1",
            "255",
            "256",
            ".",
            ".",
            ".",
            "-",
            "_",
            "@",
            ":",
            "/",
            "?",
            "#",
            "\\",
            " ",
            "\t",
            "\n",
            "[",
            "]",
            "\"",
            "'",
            ",",
            "(",
            ")",
            "\u{e9}",
            "\u{443}",
            "\u{200b}",
            "\0",
            "http://",
            "https://",
            "www",
            "youtube",
            "weather",
            "example",
            "com",
            "xn--",
            "evil",
            "api",
            "md",
            "sh",
            "localhost",
            "8080",
        ];
        let len = self.below(max + 1);
        (0..len).map(|_| PARTS[self.below(PARTS.len())]).collect()
    }
}

fn table() -> Table {
    let mut table = Table::curated();
    table.add_app("weather", &["api.weather.example"]).unwrap();
    table
}

#[test]
fn whatever_the_text_a_host_is_in_its_normal_form_or_refused() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut parsed = 0;
    for _ in 0..20_000 {
        let text = rng.text(12);
        let Ok(host) = Host::parse(&text) else {
            continue;
        };
        parsed += 1;
        let normal = host.as_str();
        assert!(!normal.is_empty() && normal.len() <= 253, "{text:?}");
        assert!(
            normal
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'),
            "{text:?} became {normal:?}"
        );
        assert!(
            !normal.ends_with('.') && !normal.starts_with('.'),
            "{normal:?}"
        );
        assert!(!normal.contains(".."), "{normal:?}");
        assert_eq!(Host::parse(normal).as_ref(), Ok(&host), "{text:?}");
    }
    assert!(parsed > 100, "only {parsed} of the texts were hosts");
}

#[test]
fn the_hosts_a_line_names_are_bounded_normal_and_in_the_line() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for _ in 0..20_000 {
        let line = rng.text(30);
        let hosts = typed_hosts(&line);
        assert!(hosts.len() <= MAX_TYPED, "{line:?}");
        let lower = line.to_ascii_lowercase();
        for host in &hosts {
            assert_eq!(Host::parse(host.as_str()).as_ref(), Ok(host));
            // The name is there to be seen, as the line wrote it.
            assert!(lower.contains(host.as_str()), "{host} is not in {line:?}");
        }
        let mut unique = hosts.clone();
        unique.dedup();
        assert_eq!(unique.len(), hosts.len(), "{line:?}");
    }
}

#[test]
fn with_nothing_known_only_a_typed_host_runs() {
    let table = Table::curated();
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let mut ran = 0;
    for _ in 0..20_000 {
        // The call writes a real host a third of the time, and half the lines
        // hold what it writes, so some do run.
        let written = match rng.below(3) {
            0 => ["a.example", "www.youtube.com", "b.c.example", "10.0.0.1"][rng.below(4)]
                .to_string(),
            _ => rng.text(8),
        };
        let line = match rng.below(2) {
            0 => format!("{} {written} {}", rng.text(6), rng.text(6)),
            _ => rng.text(20),
        };
        let verdict = Grounding {
            table: &table,
            granted: &NoGrants,
            sources: &[],
        }
        .decide(&line, &written, Origin::Line, Bytes::Checked);
        match verdict {
            Verdict::Run(Ground::Typed) => {
                ran += 1;
                let host = Host::parse(&written).expect("a host that ran is a host");
                assert!(typed_hosts(&line).contains(&host), "{line:?} / {written:?}");
            }
            // The curated words may ground their hosts, and nothing else.
            Verdict::Run(Ground::Table(word)) => {
                let host = Host::parse(&written).unwrap();
                let means = nethosts::CURATED
                    .iter()
                    .any(|(w, h)| *w == word && *h == host.as_str());
                assert!(means, "{word} / {written:?}");
            }
            Verdict::Run(other) => panic!("{other:?} with nothing granted: {line:?} / {written:?}"),
            Verdict::Ask(_) => {}
        }
    }
    assert!(ran > 200, "only {ran} ran, so this tested little");
}

#[test]
fn a_host_content_gave_asks_whatever_the_data_holds() {
    let table = table();
    let mut grants = Grants::new();
    let sources = [Source {
        host: Host::parse("updates.example").unwrap(),
        what: "the signed repository",
    }];
    for host in [
        "www.youtube.com",
        "api.weather.example",
        "api.example",
        "updates.example",
    ] {
        grants.allow(Host::parse(host).unwrap());
    }
    let mut rng = Rng(0x0bad_cafe_0bad_cafe);
    let hosts = [
        "www.youtube.com",
        "api.weather.example",
        "api.example",
        "updates.example",
    ];
    for _ in 0..5_000 {
        let line = format!(
            "{} youtube weather {}",
            rng.text(10),
            hosts[rng.below(hosts.len())]
        );
        let written = if rng.below(2) == 0 {
            hosts[rng.below(hosts.len())].to_string()
        } else {
            rng.text(6)
        };
        let verdict = Grounding {
            table: &table,
            granted: &grants,
            sources: &sources,
        }
        .decide(&line, &written, Origin::Content, Bytes::Checked);
        assert!(
            matches!(
                verdict,
                Verdict::Ask(Ungrounded::FromContent | Ungrounded::BadHost(_))
            ),
            "{line:?} / {written:?}: {verdict:?}"
        );
    }
}

/// Every one-step change to a grounded host's name: it is grounded as itself, and
/// every change to it asks.
#[test]
fn a_lookalike_of_a_grounded_host_asks() {
    let table = table();
    let line = "play something on youtube and check the weather";
    for host in ["www.youtube.com", "api.weather.example"] {
        let grounded = Grounding {
            table: &table,
            granted: &NoGrants,
            sources: &[],
        };
        let runs = |written: &str| {
            matches!(
                grounded.decide(line, written, Origin::Line, Bytes::Unchecked),
                Verdict::Run(_)
            )
        };
        assert!(runs(host));
        let mut lookalikes: Vec<String> = Vec::new();
        for at in 0..=host.len() {
            for extra in ["x", "-", ".", "0", "w"] {
                let mut changed = host.to_string();
                changed.insert_str(at, extra);
                lookalikes.push(changed);
            }
            if at < host.len() {
                let mut dropped = host.to_string();
                dropped.remove(at);
                lookalikes.push(dropped);
                for swap in ["x", "-", "0"] {
                    let mut replaced = host.to_string();
                    replaced.replace_range(at..=at, swap);
                    lookalikes.push(replaced);
                }
            }
        }
        for tail in [
            ".evil.example",
            "@evil.example",
            ".com",
            ".xyz",
            ":80",
            "/x",
            "..",
        ] {
            lookalikes.push(format!("{host}{tail}"));
        }
        for head in ["evil.", "x.", "www.", "-", "@"] {
            lookalikes.push(format!("{head}{host}"));
        }
        let own = Host::parse(host).unwrap();
        for changed in lookalikes {
            // A change that leaves the same host (a dot at the end) is the host.
            let same = Host::parse(&changed).is_ok_and(|h| h == own);
            assert_eq!(runs(&changed), same, "{changed}");
        }
    }
}
