//! A clause that says not to: a host or a table word inside it grounds nothing.

use super::*;

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

/// A host named only in a clause that says no is not grounded because the same
/// labels count in another host the line writes ("example.com" inside
/// "www.example.com"): it asks, as it does alone.
#[test]
fn a_host_named_only_in_a_negated_clause_asks_though_its_labels_count_in_another_host() {
    let table = table();
    let line = "never fetch example.com, fetch www.example.com instead";
    let words = marked(line, &["never", "fetch", "example", "com"]);
    // `marked` negates by text, so mark by place: the first clause's four
    // words, the second clause's counting.
    let words: Vec<LineWord> = words
        .into_iter()
        .enumerate()
        .map(|(i, w)| LineWord::new(&w.text, i >= 4))
        .collect();
    assert_eq!(
        fetch_marked(&table, &NoGrants, line, &words, "example.com"),
        Verdict::Ask(Ungrounded::Negated)
    );
    assert_eq!(
        fetch_marked(&table, &NoGrants, line, &words, "www.example.com"),
        Verdict::Run(Ground::Typed)
    );
    // Allowed before, still no: the line says not to.
    let mut grants = Grants::new();
    grants.allow(host("youtube.com"));
    let line = "don't open youtube.com, open youtube.com.au";
    let words: Vec<LineWord> = LineWord::all(line)
        .into_iter()
        .enumerate()
        .map(|(i, w)| LineWord::new(&w.text, i >= 4))
        .collect();
    assert_eq!(
        fetch_marked(&table, &grants, line, &words, "youtube.com"),
        Verdict::Ask(Ungrounded::Negated)
    );
    // Written twice, once counting: it runs.
    let line = "never fetch example.com, fetch example.com instead";
    let words: Vec<LineWord> = LineWord::all(line)
        .into_iter()
        .enumerate()
        .map(|(i, w)| LineWord::new(&w.text, i >= 4))
        .collect();
    assert_eq!(
        fetch_marked(&table, &NoGrants, line, &words, "example.com"),
        Verdict::Run(Ground::Typed)
    );
}
