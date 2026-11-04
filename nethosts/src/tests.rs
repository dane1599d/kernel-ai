//! The host rules: what a host is, what a line names, the table, and when a fetch
//! runs.

use super::*;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

fn host(text: &str) -> Host {
    Host::parse(text).unwrap()
}

fn typed(line: &str) -> Vec<String> {
    typed_hosts(line).iter().map(ToString::to_string).collect()
}

/// A table with the curated words and Weather's.
fn table() -> Table {
    let mut table = Table::curated();
    table.add_app("weather", &["api.weather.example"]).unwrap();
    table
}

fn decide(
    table: &Table,
    grants: &dyn Granted,
    sources: &[Source],
    line: &str,
    host: &str,
    origin: Origin,
    bytes: Bytes,
) -> Verdict {
    Grounding {
        table,
        granted: grants,
        sources,
    }
    .decide(line, &LineWord::all(line), host, origin, bytes)
}

/// A fetch of a page the model reads, with the host written in the call.
fn fetch(table: &Table, grants: &dyn Granted, line: &str, host: &str) -> Verdict {
    decide(
        table,
        grants,
        &[],
        line,
        host,
        Origin::Line,
        Bytes::Unchecked,
    )
}

// ---- a host in one normal form

#[test]
fn a_host_has_one_normal_form() {
    for text in [
        "WWW.YouTube.com.",
        "www.youtube.com",
        "www.YOUTUBE.com",
        "Www.Youtube.Com.",
    ] {
        assert_eq!(host(text).as_str(), "www.youtube.com", "{text}");
    }
    assert_eq!(host("localhost").as_str(), "localhost");
    assert_eq!(host("a-b.c1.example").as_str(), "a-b.c1.example");
}

#[test]
fn a_text_that_is_not_plainly_a_host_is_refused_not_repaired() {
    let cases = [
        ("", HostError::Empty),
        (".", HostError::Empty),
        ("..", HostError::BadLabel),
        ("a..b", HostError::BadLabel),
        (".a.b", HostError::BadLabel),
        ("-a.com", HostError::BadLabel),
        ("a-.com", HostError::BadLabel),
        ("a_b.com", HostError::BadLabel),
        ("a b.com", HostError::BadLabel),
        ("a\tb.com", HostError::BadLabel),
        ("a\0b.com", HostError::BadLabel),
        ("www.youtube.com@evil.example", HostError::UserInfo),
        ("evil.example/x", HostError::Path),
        ("evil.example?x=1", HostError::Path),
        ("evil.example#f", HostError::Path),
        ("evil.example\\x", HostError::Path),
        ("evil.example:8080", HostError::Port),
        ("[::1]", HostError::Ipv6),
        ("\u{443}outube.com", HostError::NotAscii),
        ("youtube.c\u{43e}m", HostError::NotAscii),
        ("you\u{200b}tube.com", HostError::NotAscii),
        ("1.2.3", HostError::BadLabel),
        ("256.1.1.1", HostError::BadLabel),
        ("01.2.3.4", HostError::BadLabel),
        ("1.2.3.4.5", HostError::BadLabel),
        ("12345", HostError::BadLabel),
    ];
    for (text, why) in cases {
        assert_eq!(Host::parse(text), Err(why), "{text:?}");
    }
    let label = "a".repeat(64);
    assert_eq!(
        Host::parse(&format!("{label}.com")),
        Err(HostError::BadLabel)
    );
    let part = "a".repeat(60);
    let long = [part.as_str(); 5].join(".");
    assert_eq!(Host::parse(&long), Err(HostError::TooLong));
    assert!(Host::parse(&"a".repeat(63)).is_ok());
}

#[test]
fn an_address_is_four_plain_numbers() {
    let ip = host("192.168.1.10.");
    assert_eq!(ip.as_str(), "192.168.1.10");
    assert!(ip.is_ip());
    assert!(!host("www.youtube.com").is_ip());
    assert!(host("xn--80ak6aa92e.com").is_punycode());
    assert!(!host("www.youtube.com").is_punycode());
}

#[test]
fn a_host_read_back_is_the_same_host() {
    for text in [
        "WWW.YouTube.com.",
        "1.2.3.4",
        "xn--a.b",
        "a.b-c.d",
        "LOCALHOST",
    ] {
        let first = host(text);
        assert_eq!(host(first.as_str()), first);
        assert_eq!(first.to_string(), first.as_str());
    }
}

// ---- the hosts a line names

#[test]
fn a_line_names_a_host_bare_with_a_port_or_in_an_address() {
    assert_eq!(typed("fetch www.youtube.com now"), ["www.youtube.com"]);
    assert_eq!(typed("go to WWW.YouTube.COM."), ["www.youtube.com"]);
    assert_eq!(
        typed("open https://api.weather.example/v1/leeds?q=1#top"),
        ["api.weather.example"]
    );
    assert_eq!(typed("get http://a.example:8080/x"), ["a.example"]);
    assert_eq!(
        typed("try api.weather.example:8443, please"),
        ["api.weather.example"]
    );
    assert_eq!(typed("(weather.example)"), ["weather.example"]);
    assert_eq!(
        typed("see \"weather.example\", then 'b.example'."),
        ["weather.example", "b.example"]
    );
    assert_eq!(typed("ping 10.0.0.1"), ["10.0.0.1"]);
}

#[test]
fn a_line_that_holds_no_host_names_none() {
    for line in [
        "what is 3.5 plus 1.2.3",
        "e.g. this one",
        "mail bob@example.com",
        "https://www.youtube.com@evil.example/",
        "the price is $5.00",
        "version 1.2.3.4.5",
        "weather",
        "localhost",
        "a..b",
        "",
    ] {
        assert!(typed(line).is_empty(), "{line:?} named {:?}", typed(line));
    }
}

#[test]
fn a_bare_word_that_ends_like_a_file_is_a_file_not_a_host() {
    assert!(typed("summarize readme.md and build.sh and notes.txt and x.zip").is_empty());
    // Written as an address, it is one.
    assert_eq!(typed("open https://readme.md/page"), ["readme.md"]);
    // A real name with another ending is a host.
    assert_eq!(typed("open example.org"), ["example.org"]);
}

#[test]
fn a_host_is_named_once_and_at_most_eight_are() {
    assert_eq!(typed("a.example a.example A.EXAMPLE."), ["a.example"]);
    let line: Vec<String> = (0..20).map(|n| format!("h{n}.example")).collect();
    assert_eq!(typed_hosts(&line.join(" ")).len(), MAX_TYPED);
}

// ---- the table

#[test]
fn the_table_ships_the_words_the_registry_holds_today() {
    let table = Table::curated();
    let pairs: Vec<(&str, &str)> = table
        .entries()
        .iter()
        .map(|e| (e.word.as_str(), e.host.as_str()))
        .collect();
    assert_eq!(pairs, CURATED);
    assert!(table.entries().iter().all(|e| e.by == Provider::Curated));
}

#[test]
fn an_app_adds_its_word_and_its_hosts() {
    let mut table = Table::curated();
    table
        .add_app(
            "weather",
            &[
                "api.weather.example",
                "API.Weather.Example.",
                "cdn.weather.example",
            ],
        )
        .unwrap();
    let mine: Vec<&str> = table
        .entries()
        .iter()
        .filter(|e| e.by == Provider::App(String::from("weather")))
        .map(|e| e.host.as_str())
        .collect();
    assert_eq!(mine, ["api.weather.example", "cdn.weather.example"]);
    assert!(table.entries().iter().all(|e| e.word.len() >= MIN_WORD));
}

#[test]
fn an_app_cannot_take_a_name_that_is_not_one_word_that_names_a_site() {
    let mut table = Table::curated();
    for name in [
        "Weather",
        "kai-weather",
        "ab",
        "x".repeat(MAX_WORD + 1).as_str(),
        "1weather",
        "weather!",
        "",
        "the",
        "file",
        "settings",
        "please",
    ] {
        assert_eq!(
            table.add_app(name, &["api.weather.example"]),
            Err(TableError::NotAWord),
            "{name:?}"
        );
    }
    assert_eq!(table.entries().len(), CURATED.len());
}

#[test]
fn an_app_cannot_shadow_a_shipped_word_or_share_another_apps() {
    let mut table = Table::curated();
    assert_eq!(
        table.add_app("youtube", &["evil.example"]),
        Err(TableError::Shadows)
    );
    assert_eq!(
        table.add_app("wikipedia", &["evil.example"]),
        Err(TableError::Shadows)
    );
    table.add_app("weather", &["api.weather.example"]).unwrap();
    assert_eq!(
        table.add_app("weather", &["other.example"]),
        Err(TableError::Taken(String::from("weather")))
    );
    // An upgrade removes the old entries first, then adds the new ones.
    assert_eq!(table.remove_app("weather"), 1);
    table.add_app("weather", &["api2.weather.example"]).unwrap();
}

#[test]
fn a_bad_host_or_too_many_refuses_the_whole_app() {
    let mut table = Table::curated();
    assert_eq!(table.add_app("weather", &[]), Err(TableError::Hosts));
    let five = [
        "a.example",
        "b.example",
        "c.example",
        "d.example",
        "e.example",
    ];
    assert_eq!(table.add_app("weather", &five), Err(TableError::Hosts));
    assert_eq!(
        table.add_app("weather", &["ok.example", "bad host"]),
        Err(TableError::BadHost(HostError::BadLabel))
    );
    assert_eq!(
        table.add_app("weather", &["ok.example", "evil.example:80"]),
        Err(TableError::BadHost(HostError::Port))
    );
    // Nothing of it was kept.
    assert_eq!(table, Table::curated());
}

#[test]
fn the_table_holds_at_most_sixty_four_entries() {
    let mut table = Table::curated();
    let hosts = ["a.example", "b.example", "c.example", "d.example"];
    let mut added = 0;
    for n in 0..20 {
        match table.add_app(&format!("zz{n}"), &hosts) {
            Ok(()) => added += 1,
            Err(why) => {
                assert_eq!(why, TableError::Full);
                break;
            }
        }
    }
    assert!(table.entries().len() <= MAX_ENTRIES);
    assert_eq!(added, (MAX_ENTRIES - CURATED.len()) / 4);
}

#[test]
fn removing_an_app_takes_its_word_and_hosts_and_only_its_own() {
    let mut table = table();
    table
        .add_app("maps", &["tiles.maps.example", "api.maps.example"])
        .unwrap();
    assert_eq!(table.remove_app("maps"), 2);
    assert_eq!(table.remove_app("maps"), 0);
    assert_eq!(table.remove_app("youtube"), 0);
    assert_eq!(table.entries().len(), CURATED.len() + 1);
}

// ---- the rule

#[test]
fn a_host_the_user_typed_is_grounded() {
    let table = table();
    for written in ["www.youtube.com", "WWW.YOUTUBE.COM.", "Www.YouTube.com"] {
        let verdict = fetch(&table, &NoGrants, "fetch www.youtube.com now", written);
        assert_eq!(verdict, Verdict::Run(Ground::Typed), "{written}");
    }
    // A host typed as an address, and one nothing in the table knows.
    let line = "get https://news.example/today";
    assert_eq!(
        fetch(&table, &NoGrants, line, "news.example"),
        Verdict::Run(Ground::Typed)
    );
}

#[test]
fn a_request_word_in_the_table_grounds_its_host() {
    let table = table();
    let line = "play something on youtube";
    assert_eq!(
        fetch(&table, &NoGrants, line, "www.youtube.com"),
        Verdict::Run(Ground::Table(String::from("youtube")))
    );
    let line = "what's the weather in Leeds?";
    assert_eq!(
        fetch(&table, &NoGrants, line, "api.weather.example"),
        Verdict::Run(Ground::Table(String::from("weather")))
    );
    // Words, not letters: a longer word does not hold it.
    let line = "weathering the storm";
    assert_eq!(
        fetch(&table, &NoGrants, line, "api.weather.example"),
        Verdict::Ask(Ungrounded::NotNamed)
    );
}

#[test]
fn a_label_match_is_not_enough() {
    let table = table();
    let line = "play something on youtube";
    for lookalike in [
        "youtube.xyz",
        "youtube.ru",
        "www.youtube.com.evil.example",
        "evil-www.youtube.com",
        "x.www.youtube.com",
        "youtube.com",
        "www.youtube.co",
        "wwww.youtube.com",
        "www.youtuhe.com",
        "xn--www-youtube.com",
    ] {
        let verdict = fetch(&table, &NoGrants, line, lookalike);
        assert!(
            matches!(verdict, Verdict::Ask(Ungrounded::NotNamed)),
            "{lookalike}: {verdict:?}"
        );
    }
    // A typed host grounds that host only: not a subdomain, not its parent. (No
    // app holds a word here: "weather" in a typed host is a word of the line.)
    let table = Table::curated();
    let line = "fetch weather.example";
    assert_eq!(
        fetch(&table, &NoGrants, line, "weather.example"),
        Verdict::Run(Ground::Typed)
    );
    for other in [
        "api.weather.example",
        "example",
        "weather.example.evil.example",
        "eweather.example",
    ] {
        let verdict = fetch(&table, &NoGrants, line, other);
        assert!(
            matches!(verdict, Verdict::Ask(Ungrounded::NotNamed)),
            "{other}"
        );
    }
}

#[test]
fn a_host_that_is_not_plainly_one_asks_whatever_the_line_says() {
    let table = table();
    let line = "play something on youtube www.youtube.com";
    for raw in [
        "www.youtube.com@evil.example",
        "www.youtube.com:8080",
        "www.youtube.com/watch",
        "\u{443}outube.com",
        "",
        "www.youtube.com\n",
        "[::1]",
    ] {
        let verdict = fetch(&table, &NoGrants, line, raw);
        assert!(
            matches!(verdict, Verdict::Ask(Ungrounded::BadHost(_))),
            "{raw:?}: {verdict:?}"
        );
    }
}

#[test]
fn a_host_the_user_said_yes_to_is_grounded_and_a_revoke_takes_it_back() {
    let table = table();
    let mut grants = Grants::new();
    grants.allow(host("api.example"));
    let line = "what is new";
    assert_eq!(
        fetch(&table, &grants, line, "api.example"),
        Verdict::Run(Ground::Granted)
    );
    // For that exact host, not the ones beside it.
    for other in [
        "x.api.example",
        "example",
        "api.example.evil.example",
        "api2.example",
    ] {
        assert_eq!(
            fetch(&table, &grants, line, other),
            Verdict::Ask(Ungrounded::NotNamed),
            "{other}"
        );
    }
    grants.revoke(&host("api.example"));
    assert_eq!(
        fetch(&table, &grants, line, "api.example"),
        Verdict::Ask(Ungrounded::NotNamed)
    );
}

#[test]
fn the_oss_own_list_grounds_only_bytes_that_are_checked() {
    let table = table();
    let sources = [Source {
        host: host("updates.example"),
        what: "the signed repository",
    }];
    let run = |bytes| {
        decide(
            &table,
            &NoGrants,
            &sources,
            "install notes",
            "updates.example",
            Origin::Line,
            bytes,
        )
    };
    assert_eq!(
        run(Bytes::Checked),
        Verdict::Run(Ground::Source("the signed repository"))
    );
    assert_eq!(run(Bytes::Unchecked), Verdict::Ask(Ungrounded::NotNamed));
    // Only the hosts on the list.
    let other = decide(
        &table,
        &NoGrants,
        &sources,
        "install notes",
        "evil.example",
        Origin::Line,
        Bytes::Checked,
    );
    assert_eq!(other, Verdict::Ask(Ungrounded::NotNamed));
}

#[test]
fn a_host_that_content_gave_is_never_grounded() {
    let table = table();
    let mut grants = Grants::new();
    grants.allow(host("api.example"));
    let sources = [Source {
        host: host("updates.example"),
        what: "the signed repository",
    }];
    // Typed, in the table, granted, on the OS's list: none of it matters.
    for (line, written) in [
        ("fetch www.youtube.com", "www.youtube.com"),
        ("play something on youtube", "www.youtube.com"),
        ("what is new", "api.example"),
        ("install notes", "updates.example"),
    ] {
        let verdict = decide(
            &table,
            &grants,
            &sources,
            line,
            written,
            Origin::Content,
            Bytes::Checked,
        );
        assert_eq!(verdict, Verdict::Ask(Ungrounded::FromContent), "{written}");
    }
    // A host that is not a host is that first.
    let bad = decide(
        &table,
        &grants,
        &sources,
        "x",
        "a b",
        Origin::Content,
        Bytes::Checked,
    );
    assert!(matches!(bad, Verdict::Ask(Ungrounded::BadHost(_))));
}

#[test]
fn a_host_is_grounded_by_the_first_thing_that_names_it() {
    let table = table();
    let mut grants = Grants::new();
    grants.allow(host("www.youtube.com"));
    grants.allow(host("only-granted.example"));
    let typed = fetch(&table, &grants, "fetch www.youtube.com", "www.youtube.com");
    assert_eq!(typed, Verdict::Run(Ground::Typed));
    let word = fetch(
        &table,
        &grants,
        "play something on youtube",
        "www.youtube.com",
    );
    assert_eq!(word, Verdict::Run(Ground::Table(String::from("youtube"))));
    let granted = fetch(&table, &grants, "what is new", "only-granted.example");
    assert_eq!(granted, Verdict::Run(Ground::Granted));
}

#[test]
fn only_what_the_line_names_is_a_destination_the_user_chose() {
    assert!(Ground::Typed.names_destination());
    assert!(Ground::Table(String::from("youtube")).names_destination());
    assert!(!Ground::Granted.names_destination());
    assert!(!Ground::Source("the signed repository").names_destination());
}

#[test]
fn removing_an_app_stops_its_word_grounding_its_host() {
    let mut table = table();
    let line = "what's the weather in Leeds?";
    assert!(matches!(
        fetch(&table, &NoGrants, line, "api.weather.example"),
        Verdict::Run(_)
    ));
    table.remove_app("weather");
    assert_eq!(
        fetch(&table, &NoGrants, line, "api.weather.example"),
        Verdict::Ask(Ungrounded::NotNamed)
    );
}

// ---- a clause that says not to

/// The line's words with the ones in `negated` marked as not counting, as consent
/// marks the words of a clause that negates.
fn marked(line: &str, negated: &[&str]) -> Vec<LineWord> {
    LineWord::all(line)
        .into_iter()
        .map(|w| {
            let counts = !negated.contains(&w.text.as_str());
            LineWord::new(&w.text, counts)
        })
        .collect()
}

fn fetch_marked(
    table: &Table,
    grants: &dyn Granted,
    line: &str,
    words: &[LineWord],
    host: &str,
) -> Verdict {
    Grounding {
        table,
        granted: grants,
        sources: &[],
    }
    .decide(line, words, host, Origin::Line, Bytes::Unchecked)
}

#[test]
fn a_table_word_in_a_clause_that_says_not_to_grounds_nothing() {
    let table = table();
    let line = "don't use weather";
    let words = marked(line, &["don't", "use", "weather"]);
    let verdict = fetch_marked(&table, &NoGrants, line, &words, "api.weather.example");
    assert_eq!(verdict, Verdict::Ask(Ungrounded::Negated));
    // The same line with every word counting would have run: the mark is what
    // stops it.
    assert!(matches!(
        fetch(&table, &NoGrants, line, "api.weather.example"),
        Verdict::Run(Ground::Table(_))
    ));
}

#[test]
fn a_clause_that_says_not_to_does_not_reach_the_next() {
    let table = table();
    let line = "don't use weather but play something on youtube";
    let words = marked(line, &["don't", "use", "weather"]);
    assert_eq!(
        fetch_marked(&table, &NoGrants, line, &words, "api.weather.example"),
        Verdict::Ask(Ungrounded::Negated)
    );
    assert_eq!(
        fetch_marked(&table, &NoGrants, line, &words, "www.youtube.com"),
        Verdict::Run(Ground::Table(String::from("youtube")))
    );
}

#[test]
fn a_typed_host_in_a_clause_that_says_not_to_asks_even_if_it_was_allowed_before() {
    let table = table();
    let mut grants = Grants::new();
    grants.allow(host("www.youtube.com"));
    let line = "never fetch www.youtube.com";
    let words = marked(line, &["never", "fetch", "www", "youtube", "com"]);
    for granted in [&NoGrants as &dyn Granted, &grants] {
        let verdict = fetch_marked(&table, granted, line, &words, "www.youtube.com");
        assert_eq!(verdict, Verdict::Ask(Ungrounded::Negated));
    }
    // A host the clause does not name is not barred by it.
    grants.allow(host("api.example"));
    let verdict = fetch_marked(&table, &grants, line, &words, "api.example");
    assert_eq!(verdict, Verdict::Run(Ground::Granted));
}

#[test]
fn a_word_that_counts_somewhere_counts() {
    let table = table();
    let line = "don't use weather, what's the weather in Leeds?";
    // Only the first clause negates; the word is in both.
    let words = {
        let mut words = marked(line, &[]);
        for w in words.iter_mut().take(3) {
            w.counts = false;
        }
        words
    };
    assert_eq!(
        fetch_marked(&table, &NoGrants, line, &words, "api.weather.example"),
        Verdict::Run(Ground::Table(String::from("weather")))
    );
}

#[test]
fn a_question_counts_when_the_caller_says_it_does() {
    // Reads are asked as questions: the P5 plan's own example.
    let table = table();
    let line = "what's the weather in Leeds?";
    assert_eq!(
        fetch(&table, &NoGrants, line, "api.weather.example"),
        Verdict::Run(Ground::Table(String::from("weather")))
    );
    // A caller that does not want a question to ground marks its words.
    let words = marked(line, &["what's", "the", "weather", "in", "leeds"]);
    assert_eq!(
        fetch_marked(&table, &NoGrants, line, &words, "api.weather.example"),
        Verdict::Ask(Ungrounded::Negated)
    );
}

#[test]
fn words_are_read_as_consent_reads_them() {
    let table = table();
    // An apostrophe stays in a word: "weather's" is not "weather".
    assert_eq!(
        fetch(&table, &NoGrants, "weather's down", "api.weather.example"),
        Verdict::Ask(Ungrounded::NotNamed)
    );
    let texts: Vec<String> = LineWord::all("What's the Weather in Leeds, 'ok'?")
        .into_iter()
        .map(|w| w.text)
        .collect();
    assert_eq!(texts, ["what's", "the", "weather", "in", "leeds", "ok"]);
    assert_eq!(LineWord::new("Weather", true).text, "weather");
}

#[test]
fn a_typed_host_the_caller_gave_no_counting_words_for_asks() {
    let table = table();
    let line = "fetch www.youtube.com";
    let verdict = fetch_marked(&table, &NoGrants, line, &[], "www.youtube.com");
    assert_eq!(verdict, Verdict::Ask(Ungrounded::Negated));
}

// ---- what the person reads

#[test]
fn the_ask_names_the_host_as_the_os_would_connect_to_it_and_says_why() {
    assert_eq!(
        Ungrounded::NotNamed.ask("EVIL.example."),
        "Fetch from evil.example? The request did not name that site."
    );
    assert_eq!(
        Ungrounded::FromContent.ask("evil.example"),
        "Fetch from evil.example? A page or file gave that address, not your request."
    );
    assert_eq!(
        Ungrounded::Negated.ask("api.weather.example"),
        "Fetch from api.weather.example? The request says not to use that site."
    );
    let bad = Ungrounded::BadHost(HostError::UserInfo).ask("www.youtube.com@evil.example");
    assert_eq!(
        bad,
        "Fetch from www.youtube.com@evil.example? That is not a plain host name: it has a user name before an @."
    );
    let punycode = Ungrounded::NotNamed.ask("xn--80ak6aa92e.com");
    assert!(
        punycode.ends_with("which can look like another site."),
        "{punycode}"
    );
}

#[test]
fn a_host_that_is_not_one_is_shown_safely() {
    let ask = Ungrounded::BadHost(HostError::BadLabel).ask("bad\nhost\u{7f}\u{e9}");
    assert!(ask.starts_with("Fetch from bad?host??? "), "{ask}");
    let long = Ungrounded::BadHost(HostError::BadLabel).ask(&"x ".repeat(100));
    assert!(long.contains("..."), "{long}");
    assert!(long.len() < 200, "{long}");
    assert!(long.is_ascii());
}

#[test]
fn the_note_says_why_a_fetch_ran() {
    assert_eq!(Ground::Typed.note(), "you typed it");
    assert_eq!(
        Ground::Table(String::from("weather")).note(),
        "\"weather\" is in the host table"
    );
    assert_eq!(Ground::Granted.note(), "you allowed it before");
    assert_eq!(
        Ground::Source("the signed repository").note(),
        "the signed repository; what it sends is checked before use"
    );
    assert_eq!(
        texts::fetched(
            &host("www.youtube.com"),
            &Ground::Table(String::from("youtube"))
        ),
        "Fetched from www.youtube.com (\"youtube\" is in the host table)."
    );
}

#[test]
fn every_way_a_host_asks_has_words() {
    for why in [
        Ungrounded::NotNamed,
        Ungrounded::FromContent,
        Ungrounded::Negated,
        Ungrounded::BadHost(HostError::Empty),
        Ungrounded::BadHost(HostError::TooLong),
        Ungrounded::BadHost(HostError::NotAscii),
        Ungrounded::BadHost(HostError::UserInfo),
        Ungrounded::BadHost(HostError::Path),
        Ungrounded::BadHost(HostError::Port),
        Ungrounded::BadHost(HostError::Ipv6),
        Ungrounded::BadHost(HostError::BadLabel),
    ] {
        let words = why.why();
        assert!(!words.is_empty() && words.is_ascii(), "{why:?}");
        assert!(why.ask("x.example").starts_with("Fetch from x.example? "));
    }
    for why in [
        TableError::NotAWord,
        TableError::Hosts,
        TableError::BadHost(HostError::Port),
        TableError::Shadows,
        TableError::Taken(String::from("weather")),
        TableError::Full,
    ] {
        assert!(!why.words().is_empty());
    }
    let _ = vec![0u8; 0];
}
