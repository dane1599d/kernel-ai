//! Per-app network grants (P5 task 13, host half): a new host asks once, naming
//! the app and the host; "Allow" keeps a session grant and "Always" a persistent
//! one; the app's `NetDest` narrows to its hosts; a revoke gives the grant back
//! for `undo`; nothing outside the package's `NetDest` is ever asked about.

use crate::netgrant::{ACTION, Ask, Decision, MAX_GRANTS, NetGrants};
use alloc::format;
use alloc::vec::Vec;
use kaigrants::Life;
use netdest::Dest;
use termcore::menu::Answer;

/// Weather's package: TCP to 443, names under `weather.example`.
fn ceiling() -> Dest {
    Dest::parse("proto:tcp,addrs:any,ports:443,hosts:weather.example").unwrap()
}

fn ask(grants: &NetGrants, host: &str) -> Ask {
    match grants.decide("weather", host, &ceiling()) {
        Decision::Ask(ask) => ask,
        other => panic!("{host}: {other:?}"),
    }
}

#[test]
fn a_new_host_asks_once_naming_the_app_and_the_host() {
    let mut grants = NetGrants::new();
    let asked = ask(&grants, "API.Weather.example.");
    assert_eq!(asked.host, "api.weather.example");
    assert_eq!(
        asked.question,
        "Let weather connect to api.weather.example?"
    );
    assert_eq!(
        asked.call,
        format!("{ACTION}(\"weather\", \"api.weather.example\")")
    );
    let id = grants
        .answer(&asked, &ceiling(), Answer::Allow, 12, 1_000)
        .unwrap()
        .unwrap();
    // Asked once: the next connection is granted.
    assert_eq!(
        grants.decide("weather", "api.weather.example", &ceiling()),
        Decision::Granted(id)
    );
    // Another host under the same suffix is its own ask: a grant is per host.
    assert!(matches!(
        grants.decide("weather", "eu.api.weather.example", &ceiling()),
        Decision::Ask(_)
    ));
    // Another app is its own too.
    assert!(matches!(
        grants.decide("news", "api.weather.example", &ceiling()),
        Decision::Ask(_)
    ));
}

#[test]
fn nothing_outside_the_packages_netdest_is_asked_about() {
    let grants = NetGrants::new();
    for host in [
        "evil.example",
        "evilweather.example",
        "weather.example.evil",
        "10.0.2.2",
        "a..weather.example",
        "",
        "api.weather.example..",
    ] {
        assert!(
            matches!(
                grants.decide("weather", host, &ceiling()),
                Decision::Refused(_)
            ),
            "{host:?}"
        );
    }
    assert!(matches!(
        grants.decide("Weather!", "api.weather.example", &ceiling()),
        Decision::Refused(_)
    ));
}

#[test]
fn allow_lasts_the_session_and_always_persists() {
    let mut grants = NetGrants::new();
    let a = ask(&grants, "a.weather.example");
    let b = ask(&grants, "b.weather.example");
    grants.answer(&a, &ceiling(), Answer::Allow, 1, 1).unwrap();
    grants
        .answer(&b, &ceiling(), Answer::AlwaysTarget, 2, 2)
        .unwrap();
    assert_eq!(grants.answer(&a, &ceiling(), Answer::Deny, 3, 3), Ok(None));
    assert!(
        grants
            .answer(&a, &ceiling(), Answer::AlwaysKind, 3, 3)
            .is_err()
    );
    let lives: Vec<Life> = grants.of("weather").iter().map(|g| g.life).collect();
    assert_eq!(lives, [Life::Session, Life::Persistent]);
    // A boot keeps only the persistent one.
    let mut rebooted = NetGrants::load(&grants.text().unwrap()).unwrap();
    rebooted.at_boot();
    let hosts: Vec<&str> = rebooted
        .of("weather")
        .iter()
        .map(|g| g.object.as_str())
        .collect();
    assert_eq!(hosts, ["b.weather.example"]);
    // "Always" on a session grant makes it persistent, under the same id.
    let again = grants
        .answer(&a, &ceiling(), Answer::AlwaysTarget, 4, 4)
        .unwrap();
    assert_eq!(again, Some(grants.of("weather")[0].id));
    assert_eq!(grants.of("weather")[0].life, Life::Persistent);
}

#[test]
fn a_deny_keeps_nothing_and_asks_again() {
    let mut grants = NetGrants::new();
    let asked = ask(&grants, "api.weather.example");
    assert_eq!(
        grants.answer(&asked, &ceiling(), Answer::Deny, 1, 1),
        Ok(None)
    );
    assert!(grants.of("weather").is_empty());
    assert!(matches!(
        grants.decide("weather", "api.weather.example", &ceiling()),
        Decision::Ask(_)
    ));
}

#[test]
fn the_apps_netdest_narrows_to_its_granted_hosts() {
    let mut grants = NetGrants::new();
    assert_eq!(grants.dest_for("weather", &ceiling()), None);
    let asked = ask(&grants, "api.weather.example");
    grants
        .answer(&asked, &ceiling(), Answer::Allow, 1, 1)
        .unwrap();
    let dest = grants.dest_for("weather", &ceiling()).unwrap();
    assert!(dest.within(&ceiling()));
    assert!(dest.allows_name("api.weather.example"));
    assert!(!dest.allows_name("www.weather.example"));
    // The ports and protocol stay the package's.
    assert_eq!(
        (dest.protocol, &dest.ports),
        (ceiling().protocol, &ceiling().ports)
    );
    // A package that no longer names the host gives nothing.
    let changed = Dest::parse("proto:tcp,addrs:any,ports:443,hosts:other.example").unwrap();
    assert_eq!(grants.dest_for("weather", &changed), None);
}

#[test]
fn a_revoke_gives_the_grant_back_for_undo() {
    let mut grants = NetGrants::new();
    let asked = ask(&grants, "api.weather.example");
    let id = grants
        .answer(&asked, &ceiling(), Answer::Allow, 7, 7)
        .unwrap()
        .unwrap();
    let revoked = grants.revoke(id).unwrap();
    // Revoked: the next connection asks again.
    assert!(matches!(
        grants.decide("weather", "api.weather.example", &ceiling()),
        Decision::Ask(_)
    ));
    assert_eq!(grants.revoke(id), None);
    // `undo` puts it back under its own id.
    grants.restore(revoked.clone()).unwrap();
    assert_eq!(
        grants.decide("weather", "api.weather.example", &ceiling()),
        Decision::Granted(id)
    );
    assert!(grants.restore(revoked.clone()).is_err(), "held again");
    // Granted again since the revoke: the undo is refused, not doubled.
    let back = grants.revoke(id).unwrap();
    let fresh = ask(&grants, "api.weather.example");
    let new_id = grants
        .answer(&fresh, &ceiling(), Answer::Allow, 8, 8)
        .unwrap()
        .unwrap();
    assert_ne!(new_id, id, "ids are not reused while the shell runs");
    assert!(grants.restore(back).is_err());
}

#[test]
fn the_file_is_read_as_hostile_and_holds_only_host_grants() {
    let mut grants = NetGrants::new();
    let asked = ask(&grants, "api.weather.example");
    grants
        .answer(
            &asked,
            &ceiling(),
            Answer::AlwaysTarget,
            3,
            1_762_300_000_000_000_000,
        )
        .unwrap();
    let text = grants.text().unwrap();
    assert_eq!(NetGrants::load(&text).unwrap(), grants);
    let dir = "kai-grants 1\ngrant 1 app=notes kind=dir object=/home/user/notes rights=read from=picker life=persistent signer=- made=2026-11-02T10:14:00Z\n";
    assert!(NetGrants::load(dir).is_err());
    let rule =
        "kai-grants 1\nrule 1 action=proc_stop target=- from=prompt:1 made=2026-11-02T10:14:00Z\n";
    assert!(NetGrants::load(rule).is_err());
    let upper = text.replace("api.weather.example", "API.weather.example");
    assert!(NetGrants::load(&upper).is_err());
    assert!(NetGrants::load("not a grants file").is_err());
}

#[test]
fn a_name_too_long_for_the_card_is_refused_not_cut() {
    let grants = NetGrants::new();
    let label = "a".repeat(60);
    let long = format!("{label}.{label}.weather.example");
    assert!(matches!(
        grants.decide("weather", &long, &ceiling()),
        Decision::Refused(why) if why.contains("too long")
    ));
}

#[test]
fn the_store_is_bounded() {
    let mut grants = NetGrants::new();
    for i in 0..MAX_GRANTS {
        let asked = ask(&grants, &format!("h{i}.weather.example"));
        grants
            .answer(&asked, &ceiling(), Answer::Allow, 1, 1)
            .unwrap();
    }
    let one_more = ask(&grants, "last.weather.example");
    assert!(
        grants
            .answer(&one_more, &ceiling(), Answer::Allow, 1, 1)
            .is_err()
    );
}

#[test]
fn a_grant_names_its_host_exactly_and_no_address() {
    let mut grants = NetGrants::new();
    // A ceiling with literal addresses too: a host grant opens none of them.
    let wide = Dest::parse("proto:tcp,addrs:10.0.2.0/24,ports:443,hosts:weather.example").unwrap();
    let asked = match grants.decide("weather", "api.weather.example", &wide) {
        Decision::Ask(ask) => ask,
        other => panic!("{other:?}"),
    };
    grants.answer(&asked, &wide, Answer::Allow, 1, 1).unwrap();
    let dest = grants.dest_for("weather", &wide).unwrap();
    assert!(dest.exact);
    assert!(dest.allows_name("api.weather.example"));
    assert!(
        !dest.allows_name("x.api.weather.example"),
        "no names under it"
    );
    assert!(!dest.allows_literal(dest.protocol, "10.0.2.2".parse().unwrap(), 443));
    assert!(dest.within(&wide));
}

#[test]
fn a_grant_the_package_dropped_leaves_the_others() {
    let mut grants = NetGrants::new();
    for host in ["api.weather.example", "maps.weather.example"] {
        let asked = ask(&grants, host);
        grants
            .answer(&asked, &ceiling(), Answer::Allow, 1, 1)
            .unwrap();
    }
    let narrower = Dest::parse("proto:tcp,addrs:any,ports:443,hosts:api.weather.example").unwrap();
    let dest = grants.dest_for("weather", &narrower).unwrap();
    assert_eq!(dest.hosts, ["api.weather.example"]);
}

#[test]
fn an_answer_is_checked_against_the_ceiling_again() {
    let mut grants = NetGrants::new();
    let asked = ask(&grants, "api.weather.example");
    let changed = Dest::parse("proto:tcp,addrs:any,ports:443,hosts:other.example").unwrap();
    assert!(
        grants
            .answer(&asked, &changed, Answer::Allow, 1, 1)
            .is_err()
    );
    assert!(grants.of("weather").is_empty());
}
