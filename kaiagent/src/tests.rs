//! A manifest is read as hostile: each refusal has its test, and the ADR's own
//! example parses.

use super::*;
use agentabi::{ROWS, Tier};
use alloc::format;
use std::string::String;

/// ADR 0013 section 6's example, with an id.
const GOOD: &str = "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass background\n\
    model tokens/day=50000 kv=1024 pool=64m\nactions fs_list fs_read notes_*\n\
    memory read write\nnet none\n";

/// `GOOD` with the line for `key` holding `value` instead.
fn with(key: &str, value: &str) -> String {
    let mut out = String::new();
    for line in GOOD.split_inclusive('\n') {
        match line.strip_prefix(key).and_then(|r| r.strip_prefix(' ')) {
            Some(_) => out.push_str(&format!("{key} {value}\n")),
            None => out.push_str(line),
        }
    }
    out
}

/// Why `text` is refused.
fn refused(text: &str) -> Refusal {
    Manifest::parse(text).expect_err(text)
}

fn refused_with(text: &str, rows: &[Action]) -> Refusal {
    Manifest::parse_with(text, rows).expect_err(text)
}

#[test]
fn the_adrs_example_parses_and_writes_back_as_it_was() {
    let m = Manifest::parse(GOOD).unwrap();
    assert_eq!(m.id, "notes-agent");
    assert_eq!(m.abi, (1, 0));
    assert_eq!(m.class, Class::Background);
    assert_eq!(
        m.budgets,
        Budgets {
            tokens_per_day: 50_000,
            kv_positions: 1_024,
            pool_mib: 64
        }
    );
    assert_eq!(m.budgets.pool_bytes(), 64 << 20);
    assert_eq!(
        m.actions,
        [
            Pattern::Row("fs_list".into()),
            Pattern::Row("fs_read".into()),
            Pattern::Group("notes".into())
        ]
    );
    assert_eq!(
        m.memory,
        Memory {
            read: true,
            write: true
        }
    );
    assert_eq!(m.format(), GOOD);
    assert_eq!(Manifest::parse(&m.format()), Ok(m));
}

#[test]
fn a_budget_left_out_is_question_tens_default() {
    assert_eq!(DEFAULT_TOKENS_PER_DAY, 100_000);
    assert_eq!(DEFAULT_KV_POSITIONS, 2_048);
    assert_eq!(DEFAULT_POOL_MIB, 128);
    let all = Manifest::parse(&with("model", "-")).unwrap();
    assert_eq!(all.budgets, Budgets::DEFAULT);
    assert_eq!(all.budgets.pool_bytes(), 128 << 20);
    // Any of the three, in order, and the rest is the default.
    let some = Manifest::parse(&with("model", "kv=512")).unwrap().budgets;
    assert_eq!((some.tokens_per_day, some.kv_positions), (100_000, 512));
    assert_eq!(some.pool_mib, 128);
    let two = Manifest::parse(&with("model", "tokens/day=7 pool=1m"))
        .unwrap()
        .budgets;
    assert_eq!(
        (two.tokens_per_day, two.kv_positions, two.pool_mib),
        (7, 2_048, 1)
    );
    // The ceilings themselves are allowed.
    let top = format!("tokens/day={MAX_TOKENS_PER_DAY} kv={MAX_KV_POSITIONS} pool={MAX_POOL_MIB}m");
    assert!(Manifest::parse(&with("model", &top)).is_ok());
}

#[test]
fn an_agent_with_no_actions_and_no_memory_is_a_model_client() {
    let text = with("actions", "-").replace("memory read write", "memory none");
    let m = Manifest::parse(&text).unwrap();
    assert!(m.actions.is_empty() && m.memory == Memory::default());
    assert!(!m.allows("fs_list"));
    assert_eq!(Manifest::parse(&m.format()), Ok(m));
}

#[test]
fn a_manifest_that_is_not_in_shape_is_refused() {
    assert_eq!(refused(&GOOD[..GOOD.len() - 1]), Refusal::NoNewline);
    assert_eq!(refused(&"x".repeat(MAX_BYTES + 1)), Refusal::TooLong);
    assert_eq!(
        refused(&GOOD.replace("notes-agent", "notes\u{0}x")),
        Refusal::Control
    );
    assert_eq!(
        refused(&GOOD.replace("notes-agent", "notes\ragent")),
        Refusal::Control
    );
    assert_eq!(
        refused(&GOOD.replace("kai-app 1", "kai-app 2")),
        Refusal::NotKaiApp
    );
    assert_eq!(refused(""), Refusal::NoNewline);
    assert_eq!(refused("\n"), Refusal::NotKaiApp);
}

#[test]
fn each_key_is_needed_in_its_place_and_nothing_follows_the_last() {
    for key in [
        "id", "abi", "agent", "class", "model", "actions", "memory", "net",
    ] {
        // Missing.
        let missing: String = GOOD
            .split_inclusive('\n')
            .filter(|l| !l.starts_with(&format!("{key} ")))
            .collect();
        assert!(matches!(refused(&missing), Refusal::Expected(_)), "{key}");
        // Empty, or with no space before the value.
        assert_eq!(
            refused(&with(key, "")),
            Refusal::Expected(static_key(key)),
            "{key}"
        );
    }
    // Out of order: class before agent.
    let swapped = GOOD.replace("agent 1\nclass background\n", "class background\nagent 1\n");
    assert_eq!(refused(&swapped), Refusal::Expected("agent"));
    // Repeated, or anything after the last: a route is an app's.
    let twice = format!("{GOOD}net none\n");
    assert_eq!(refused(&twice), Refusal::Trailing("net none".into()));
    let route = format!("{GOOD}use kai:files/types@0.1 scope=app-data\n");
    assert!(matches!(refused(&route), Refusal::Trailing(_)));
    let early = GOOD.replace("agent 1\n", "use kai:files/types@0.1\nagent 1\n");
    assert_eq!(refused(&early), Refusal::Expected("agent"));
}

fn static_key(key: &str) -> &'static str {
    [
        "id", "abi", "agent", "class", "model", "actions", "memory", "net",
    ]
    .into_iter()
    .find(|k| *k == key)
    .unwrap()
}

#[test]
fn the_id_is_a_package_name_that_is_not_an_os_group_a_window_or_a_built_in_agent() {
    for id in [
        "a",
        "Notes",
        "1notes",
        "-notes",
        "notes_agent",
        "has space",
        "waytoolongname1",
        "näme",
    ] {
        assert!(
            matches!(refused(&with("id", id)), Refusal::BadId(_)),
            "{id}"
        );
    }
    // An OS row group is a reserved package name, and `<x>-window` is an app's.
    for id in ["fs", "pkg", "memory", "shell", "notes-window"] {
        assert!(
            matches!(refused(&with("id", id)), Refusal::BadId(_)),
            "{id}"
        );
    }
    for id in BUILTIN {
        assert_eq!(
            refused(&with("id", id)),
            Refusal::BuiltinName(id.into()),
            "{id}"
        );
    }
    for id in ["ab", "a1", "twelve-chars", "pdf-view"] {
        assert!(Manifest::parse(&with("id", id)).is_ok(), "{id}");
    }
}

#[test]
fn the_abi_is_the_oldest_this_agent_needs_and_not_newer_than_the_os() {
    // Against the OS's own ABI, so a minor bump on main needs no edit here.
    let (major, minor) = kabi::handle::ABI;
    assert_eq!(major, 1);
    assert!(Manifest::parse(&with("abi", "1.0")).is_ok());
    let now = std::format!("{major}.{minor}");
    assert!(Manifest::parse(&with("abi", &now)).is_ok(), "{now}");
    for abi in [
        "1", "1.", ".0", "a.b", "1.0.0", "01.0", "1.00", "-1.0", "+1.0", "1 .0", "99999.0",
    ] {
        assert_eq!(refused(&with("abi", abi)), Refusal::BadAbi, "{abi}");
    }
    let next = std::format!("{major}.{}", minor + 1);
    for abi in [next.as_str(), "2.0", "0.0", "0.9"] {
        assert_eq!(refused(&with("abi", abi)), Refusal::AbiUnsupported, "{abi}");
    }
}

#[test]
fn only_agent_one_with_a_known_class() {
    assert_eq!(
        refused(&with("agent", "2")),
        Refusal::AgentVersion("2".into())
    );
    assert_eq!(
        refused(&with("agent", "1 x")),
        Refusal::AgentVersion("1 x".into())
    );
    assert!(Manifest::parse(&with("class", "interactive")).is_ok());
    for class in [
        "Background",
        "foreground",
        "daemon",
        "background interactive",
    ] {
        assert_eq!(
            refused(&with("class", class)),
            Refusal::BadClass(class.into()),
            "{class}"
        );
    }
}

#[test]
fn a_budget_is_a_whole_number_above_zero_within_the_ceilings() {
    for (value, key) in [
        ("tokens/day=0", "tokens/day"),
        ("kv=0", "kv"),
        ("pool=0m", "pool"),
    ] {
        assert_eq!(
            refused(&with("model", value)),
            Refusal::BudgetZero(key),
            "{value}"
        );
    }
    for (value, key) in [
        ("tokens/day=5x", "tokens/day"),
        ("tokens/day=+5", "tokens/day"),
        ("tokens/day=05", "tokens/day"),
        ("tokens/day=", "tokens/day"),
        ("tokens/day=-1", "tokens/day"),
        ("tokens/day=1.5", "tokens/day"),
        ("kv=99999999999", "kv"),
        ("pool=m", "pool"),
        ("pool=1_0m", "pool"),
    ] {
        assert_eq!(
            refused(&with("model", value)),
            Refusal::BudgetNumber(key),
            "{value}"
        );
    }
    // The pool's unit is MiB.
    for value in ["pool=64", "pool=64g", "pool=64M", "pool=64k"] {
        assert_eq!(
            refused(&with("model", value)),
            Refusal::BudgetUnit,
            "{value}"
        );
    }
    // A day's tokens have a ceiling: a number that large is no budget at all.
    for value in [
        "tokens/day=2000001",
        "tokens/day=4294967295",
        "tokens/day=4294967296",
    ] {
        assert_eq!(
            refused(&with("model", value)),
            Refusal::BudgetOver {
                key: "tokens/day",
                max: u64::from(MAX_TOKENS_PER_DAY)
            },
            "{value}"
        );
    }
    assert_eq!(
        refused(&with("model", "kv=2049")),
        Refusal::BudgetOver {
            key: "kv",
            max: u64::from(MAX_KV_POSITIONS)
        }
    );
    assert_eq!(
        refused(&with("model", "pool=513m")),
        Refusal::BudgetOver {
            key: "pool",
            max: u64::from(MAX_POOL_MIB)
        }
    );
}

// A default is within its ceiling.
const _: () = assert!(DEFAULT_TOKENS_PER_DAY < MAX_TOKENS_PER_DAY);
const _: () = assert!(DEFAULT_POOL_MIB < MAX_POOL_MIB);
const _: () = assert!(DEFAULT_KV_POSITIONS <= MAX_KV_POSITIONS);

/// The ceilings are the engine's, not a number of ours: `llm` holds 2,048 positions a
/// slot, and a day of its decoding is far under two million tokens.
#[test]
fn the_ceilings_are_the_engines() {
    assert_eq!(MAX_KV_POSITIONS as usize, kabi::model::CONTEXT_CAP);
    assert_eq!(MAX_KV_POSITIONS, DEFAULT_KV_POSITIONS);
    assert_eq!(MAX_TOKENS_PER_DAY, 2_000_000);
    // One under a ceiling is allowed, and one over is not.
    let under = format!(
        "tokens/day={} kv={}",
        MAX_TOKENS_PER_DAY - 1,
        MAX_KV_POSITIONS - 1
    );
    assert!(Manifest::parse(&with("model", &under)).is_ok());
    let kv = format!("kv={}", MAX_KV_POSITIONS + 1);
    assert!(matches!(
        refused(&with("model", &kv)),
        Refusal::BudgetOver { key: "kv", .. }
    ));
}

#[test]
fn the_model_line_takes_each_key_once_in_order_and_no_other() {
    for value in [
        "kv=1 tokens/day=1",
        "kv=1 kv=2",
        "tokens/day=1 tokens/day=1",
        "pool=1m kv=1",
        "ctx=1",
        "tokens=1",
        "kv",
        "kv=1  pool=1m",
        "-1",
        "- kv=1",
        "default",
    ] {
        assert!(
            matches!(refused(&with("model", value)), Refusal::BudgetKey(_)),
            "{value}"
        );
    }
}

#[test]
fn an_actions_entry_is_a_row_name_or_a_group_and_nothing_else() {
    for entry in [
        "*",
        "_*",
        "fs_*x",
        "fs*",
        "f*_list",
        "FS_list",
        "fs-list",
        "fs_list_",
        "_fs",
        "1fs",
        "notes_add_",
        "fs_l!st",
        "a_*",
        "notes-agent_*",
        "notes_window_*",
        "fs_list,fs_read",
    ] {
        assert!(
            matches!(refused(&with("actions", entry)), Refusal::ActionShape(_)),
            "{entry}"
        );
    }
    // A row name is at most 24 bytes, as the registry holds it.
    let long = format!("notes_{}", "a".repeat(19));
    assert!(matches!(
        refused(&with("actions", &long)),
        Refusal::ActionShape(_)
    ));
    let fits = format!("notes_{}", "a".repeat(18));
    assert!(Manifest::parse(&with("actions", &fits)).is_ok());
    // Two spaces make an empty entry.
    assert!(matches!(
        refused(&with("actions", "fs_list  fs_read")),
        Refusal::ActionShape(_)
    ));
}

#[test]
fn a_row_or_group_listed_twice_is_refused_even_by_its_group() {
    for value in [
        "fs_list fs_list",
        "notes_* notes_*",
        "notes_add notes_*",
        "notes_* notes_add",
        "fs_* fs_list",
        "fs_list fs_*",
    ] {
        assert!(
            matches!(refused(&with("actions", value)), Refusal::ActionTwice(_)),
            "{value}"
        );
    }
    // `notes_*` does not hold `notesx_add`, nor `notes` as a prefix of another word.
    assert!(Manifest::parse(&with("actions", "notes_* notesx_add")).is_ok());
}

#[test]
fn at_most_thirty_two_actions() {
    let many = |n: usize| {
        (0..n)
            .map(|i| format!("pkg{i}_x"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(Manifest::parse(&with("actions", &many(MAX_ACTIONS))).is_ok());
    assert_eq!(
        refused(&with("actions", &many(MAX_ACTIONS + 1))),
        Refusal::TooManyActions
    );
}

#[test]
fn a_row_or_group_in_an_os_group_must_be_an_os_row() {
    // The Terminal's `shell_line` is not a row, and no package `shell` can exist.
    assert_eq!(
        refused(&with("actions", "shell_line")),
        Refusal::NoSuchOsRow("shell_line".into())
    );
    assert_eq!(
        refused(&with("actions", "fs_nothing")),
        Refusal::NoSuchOsRow("fs_nothing".into())
    );
    assert_eq!(
        refused(&with("actions", "cpus_more")),
        Refusal::NoSuchOsRow("cpus_more".into())
    );
    // Real rows, one-word ones and groups are in.
    assert!(agentabi::find(&ROWS, "fs_list").is_some());
    let m = Manifest::parse(&with("actions", "fs_list cpus fs_read notes_add")).unwrap();
    assert_eq!(m.actions.len(), 4);
    assert!(Manifest::parse(&with("actions", "fs_*")).is_ok());
    // A registry without the group has no rows to name.
    assert_eq!(
        refused_with(&with("actions", "fs_*"), &[]),
        Refusal::GroupWithoutRows("fs".into())
    );
    assert_eq!(
        refused_with(&with("actions", "fs_list"), &[]),
        Refusal::NoSuchOsRow("fs_list".into())
    );
    // A package's row or group is for install to check: nothing here to see.
    assert!(Manifest::parse_with(&with("actions", "notes_add pdf_view_*"), &[]).is_ok());
}

#[test]
fn a_row_an_agent_may_never_call_is_refused_alone_and_by_its_group() {
    let mut never = *agentabi::find(&ROWS, "fs_list").unwrap();
    never.min_tier = Tier::Never;
    let rows = [never];
    assert_eq!(
        refused_with(&with("actions", "fs_list"), &rows),
        Refusal::NeverRow("fs_list".into())
    );
    assert_eq!(
        refused_with(&with("actions", "fs_*"), &rows),
        Refusal::GroupHoldsNever("fs".into())
    );
    // The same registry, with the row as it is, takes both.
    assert!(
        Manifest::parse_with(
            &with("actions", "fs_list"),
            &[*agentabi::find(&ROWS, "fs_list").unwrap()]
        )
        .is_ok()
    );
}

#[test]
fn what_the_manifest_allows_is_a_row_listed_or_inside_a_group_listed() {
    let m = Manifest::parse(GOOD).unwrap();
    for row in ["fs_list", "fs_read", "notes_add", "notes_remove", "notes"] {
        assert!(m.allows(row), "{row}");
    }
    for row in [
        "fs_write",
        "fs_listx",
        "notesx_add",
        "note_add",
        "",
        "pkg_install",
    ] {
        assert!(!m.allows(row), "{row}");
    }
}

#[test]
fn memory_is_none_read_write_or_both_in_that_order() {
    for (word, read, write) in [
        ("none", false, false),
        ("read", true, false),
        ("write", false, true),
        ("read write", true, true),
    ] {
        let m = Manifest::parse(&with("memory", word)).unwrap();
        assert_eq!((m.memory.read, m.memory.write), (read, write), "{word}");
    }
    for word in [
        "write read",
        "read  write",
        "read read",
        "all",
        "read, write",
        "none read",
        "Read",
        "forget",
        "-",
    ] {
        assert_eq!(
            refused(&with("memory", word)),
            Refusal::Memory(word.into()),
            "{word}"
        );
    }
}

#[test]
fn the_net_line_is_none_or_a_destination_an_agent_may_hold() {
    assert_eq!(Manifest::parse(&with("net", "none")).unwrap().net, None);
    let weather = "proto:tcp,ports:443,hosts:api.weather.example";
    let parsed = Manifest::parse(&with("net", weather)).unwrap();
    let dest = parsed.net.clone().expect("a destination");
    assert_eq!(dest.hosts, ["api.weather.example"]);
    // It reads back as it was written.
    assert_eq!(Manifest::parse(&parsed.format()).unwrap(), parsed);
    let few = with("net", "proto:tcp,ports:8000-8015,hosts:a.example");
    assert!(
        Manifest::parse(&few).is_ok(),
        "a range of 16 ports is named"
    );
    for (value, why) in [
        ("any", "netdest"),
        ("example.com", "netdest"),
        ("proto:udp,ports:53,hosts:dns.example", "tcp"),
        (
            "proto:tcp,addrs:10.0.0.1,ports:443,hosts:a.example",
            "never addresses",
        ),
        (
            "proto:tcp,addrs:any,ports:443,hosts:a.example",
            "never addresses",
        ),
        ("proto:tcp,ports:any,hosts:a.example", "ports"),
        ("proto:tcp,hosts:a.example", "ports"),
        ("proto:tcp,ports:1-65535,hosts:a.example", "ports"),
        ("proto:tcp,ports:8000-8016,hosts:a.example", "ports"),
        ("proto:tcp,ports:443", "at least one host"),
        ("proto:tcp,ports:443,hosts:com", "netdest"),
        ("proto:tcp,ports:443,hosts:any", "the shell's alone"),
        ("None", "netdest"),
    ] {
        let refusal = refused(&with("net", value));
        let Refusal::Net(got, words) = &refusal else {
            panic!("{value}: {refusal:?}");
        };
        assert_eq!(got, value);
        assert!(why == "netdest" || words.contains(why), "{value}: {words}");
    }
}

#[test]
fn a_manifest_with_net_always_asks_at_install() {
    let net = with("net", "proto:tcp,ports:443,hosts:api.weather.example");
    let interactive = net.replace("class background", "class interactive");
    let manifest = Manifest::parse(&interactive).unwrap();
    assert_eq!(manifest.class, Class::Interactive);
    assert!(manifest.asks_at_install());
}

#[test]
fn a_background_agent_always_asks_at_install_and_an_interactive_one_follows_the_package_rules() {
    assert!(
        Manifest::parse(&with("class", "background"))
            .unwrap()
            .asks_at_install()
    );
    assert!(
        !Manifest::parse(&with("class", "interactive"))
            .unwrap()
            .asks_at_install()
    );
}

#[test]
fn every_refusal_says_one_sentence() {
    for text in [
        with("id", "fs"),
        with("abi", "2.0"),
        with("class", "x"),
        with("model", "kv=0"),
        with("actions", "fs_nothing"),
        with("memory", "x"),
        with("net", "x"),
        format!("{GOOD}x\n"),
    ] {
        let why = refused(&text).to_string();
        assert!(!why.is_empty() && !why.contains('\n'), "{why}");
    }
}

const WEATHER: &str = "proto:tcp,ports:443,hosts:api.weather.example";

#[test]
fn a_net_line_says_the_bytes_a_day_or_takes_the_default_and_none_moves_nothing() {
    use crate::{DEFAULT_NET_MIB_PER_DAY, MAX_NET_MIB_PER_DAY, NetBudget};
    let none = Manifest::parse(&with("net", "none")).unwrap();
    assert_eq!(none.net_budget, NetBudget::NONE);
    assert_eq!(none.net_budget.bytes_per_day(), 0);
    // A destination that says no more: 10 MiB.
    let plain = Manifest::parse(&with("net", WEATHER)).unwrap();
    assert_eq!(plain.net_budget, NetBudget::DEFAULT);
    assert_eq!(plain.net_budget.mib_per_day, DEFAULT_NET_MIB_PER_DAY);
    assert_eq!(plain.net_budget.bytes_per_day(), 10 << 20);
    // One that says it, up to the ceiling.
    for mib in [1, 64, MAX_NET_MIB_PER_DAY] {
        let said = with("net", &format!("{WEATHER} bytes/day={mib}m"));
        let parsed = Manifest::parse(&said).unwrap();
        assert_eq!(parsed.net_budget.mib_per_day, mib);
        assert_eq!(parsed.net_budget.bytes_per_day(), u64::from(mib) << 20);
        // It reads back as it was written, and a written manifest always says it.
        assert_eq!(Manifest::parse(&parsed.format()).unwrap(), parsed);
        assert!(parsed.format().contains(&format!("bytes/day={mib}m\n")));
    }
    // The destination is written canonically, then the budget.
    let dest = plain.net.clone().expect("a destination");
    assert!(
        plain
            .format()
            .contains(&format!("net {dest} bytes/day=10m\n"))
    );
}

#[test]
fn a_net_budget_is_held_to_the_same_shape_as_the_model_budgets() {
    let net = |word: &str| with("net", &format!("{WEATHER} {word}"));
    let key = |word: &str| Refusal::NetBudgetKey(word.into());
    let over = |max| Refusal::BudgetOver {
        key: "net bytes/day",
        max,
    };
    let number = Refusal::BudgetNumber("net bytes/day");
    assert_eq!(
        refused(&with("net", "none bytes/day=1m")),
        Refusal::Net("none bytes/day=1m".into(), "a budget needs a destination")
    );
    assert_eq!(
        refused(&with("net", "none ")),
        Refusal::Net("none ".into(), "a budget needs a destination")
    );
    // The word after the destination is `bytes/day=Nm` and nothing else.
    for word in [
        "megabytes/day=1m",
        "BYTES/DAY=1m",
        "bytes/day",
        "bytes",
        "tokens/day=1m",
        "pool=1m",
        "1m",
        "x",
        " bytes/day=1m",
        "",
    ] {
        assert_eq!(refused(&net(word)), key(word), "{word:?}");
    }
    // The unit is a lower-case `m`, and the key is not repeated or followed.
    for word in [
        "bytes/day=1",
        "bytes/day=5M",
        "bytes/day=5mb",
        "bytes/day=1g",
        "bytes/day=",
        "bytes/day=1m x",
        "bytes/day=1m ",
    ] {
        assert_eq!(refused(&net(word)), Refusal::NetBudgetUnit, "{word:?}");
    }
    // The number is a whole number without a sign, a leading zero or a space.
    for word in [
        "bytes/day=m",
        "bytes/day=+5m",
        "bytes/day=-5m",
        "bytes/day=01m",
        "bytes/day=5.5m",
        "bytes/day=5 m",
        "bytes/day=\u{ff15}m",
        "bytes/day=99999999999m",
        "bytes/day=18446744073709551616m",
        "bytes/day=1m bytes/day=2m",
    ] {
        assert_eq!(refused(&net(word)), number, "{word:?}");
    }
    assert_eq!(
        refused(&net("bytes/day=0m")),
        Refusal::BudgetZero("net bytes/day")
    );
    // A number that fits a u32 and is past the ceiling, and one at the ceiling.
    for word in ["bytes/day=513m", "bytes/day=4294967296m"] {
        assert_eq!(refused(&net(word)), over(512), "{word:?}");
    }
    assert!(Manifest::parse(&net("bytes/day=512m")).is_ok());
    // A tab is a control character, refused before any line is read.
    assert_eq!(
        refused(&with("net", &format!("{WEATHER}\tbytes/day=1m"))),
        Refusal::Control
    );
}

#[test]
fn a_destination_is_judged_before_its_budget_word() {
    // A bad destination with a bad budget, and with a good one: the destination's
    // refusal, as it was before the budget existed.
    for value in [
        "proto:udp,ports:53,hosts:dns.example bytes/day=0m",
        "proto:udp,ports:53,hosts:dns.example bytes/day=1m",
        "proto:tcp, ports:443,hosts:a.example",
        "bytes/day=1m proto:tcp,ports:443,hosts:a.example",
    ] {
        let refusal = refused(&with("net", value));
        assert!(
            matches!(refusal, Refusal::Net(ref got, _) if got == value),
            "{value}: {refusal:?}"
        );
    }
}

/// A host of 251 bytes: four labels of 60 and `.example`.
fn long_host(c: char) -> String {
    let label = c.to_string().repeat(60);
    format!("{label}.{label}.{label}.{label}.example")
}

#[test]
fn a_destination_that_is_too_long_once_written_in_full_is_refused() {
    // Short form: no `proto:` or `addrs:`, which a manifest may leave out, and which
    // are 21 bytes when they are written.
    let hosts = |n: usize| {
        ['a', 'b', 'c', 'd', 'e']
            .iter()
            .take(n)
            .map(|c| long_host(*c))
            .collect::<Vec<_>>()
            .join("+")
    };
    // Three hosts fit either way, and read back.
    let three = format!("ports:443,hosts:{}", hosts(3));
    let parsed = Manifest::parse(&with("net", &three)).unwrap();
    assert_eq!(Manifest::parse(&parsed.format()).unwrap(), parsed);
    // Four are under netdest's limit as written short (1,023 bytes) and over it once
    // written in full (1,044): refused, not written as a manifest that cannot be read.
    let four = format!("ports:443,hosts:{}", hosts(4));
    assert!(four.len() <= netdest::MAX_TEXT, "{}", four.len());
    assert!(netdest::Dest::parse(&four).is_ok());
    let refusal = refused(&with("net", &four));
    let Refusal::Net(got, why) = &refusal else {
        panic!("{refusal:?}");
    };
    assert_eq!(got, &four);
    assert!(why.contains("too long"), "{why}");
    // The same four, written in full, are over the limit for netdest itself.
    let full = format!("proto:tcp,addrs:none,{four}");
    assert!(netdest::Dest::parse(&full).is_err());
}
