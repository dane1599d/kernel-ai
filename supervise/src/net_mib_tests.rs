//! `net_mib=` (P5 task 14): the MiB a day a line's `netdest=` may move, which
//! the kernel counts on the handle's record.

use super::*;

fn errors(text: &str) -> Vec<String> {
    parse_plan(text).errors
}

const DEST: &str = "netdest=proto:tcp,ports:443,hosts:example.org";

#[test]
fn a_lines_bytes_a_day_go_with_its_destination() {
    let plan = parse_plan(&format!("agentnet caps=net {DEST} net_mib=10"));
    assert_eq!(plan.errors, Vec::<String>::new());
    let grants = plan.grants("agentnet", "agentnet").unwrap();
    assert_eq!(grants.net_mib, Some(10));
    assert!(grants.netdest.is_some());
    // A line that says none is not counted.
    let plan = parse_plan(&format!("other caps=net {DEST}"));
    assert_eq!(plan.grants("other", "other").unwrap().net_mib, None);
}

#[test]
fn net_mib_needs_a_destination_and_a_whole_number_within_the_most() {
    for (line, why) in [
        ("a caps=net net_mib=10", "a: net_mib= needs a netdest="),
        (
            &format!("a caps=net {DEST} net_mib=0") as &str,
            "a: net_mib=0 is not 1 to 512 MiB a day",
        ),
        (
            &format!("a caps=net {DEST} net_mib=513"),
            "a: net_mib=513 is not 1 to 512 MiB a day",
        ),
        (
            &format!("a caps=net {DEST} net_mib=1m"),
            "a: net_mib=1m is not 1 to 512 MiB a day",
        ),
        (
            &format!("a caps=net {DEST} net_mib=1 net_mib=2"),
            "a: net_mib= given twice",
        ),
    ] {
        assert_eq!(errors(line), [why], "{line}");
    }
    assert!(errors(&format!("a caps=net {DEST} net_mib=512")).is_empty());
}

#[test]
fn the_most_is_an_agent_manifests_most() {
    assert_eq!(MAX_NET_MIB, kaiagent::MAX_NET_MIB_PER_DAY);
}
