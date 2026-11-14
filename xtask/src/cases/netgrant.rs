//! P5 task 13, per-app network grants (lane C): an app's HTTPS through
//! `kai:net`, asked per host on the trusted path's card.

use super::{Https, TestCase, case};
use crate::mock_https::{ASSETS, Content, OFF, RELEASES, REPO};

/// What the mock serves the test app: `/hello` on each host it asks for, and
/// a download on the releases host that redirects to the assets host, as
/// GitHub's do.
fn content() -> Content {
    [REPO, ASSETS, OFF]
        .into_iter()
        .fold(Content::new(), |content, host| {
            content.file(host, "/hello", b"kai mock: hello\n".to_vec())
        })
        .redirect(RELEASES, "/dl", "https://assets.kai.test/hello")
}

pub(super) const CASES: &[TestCase] = &[
    // The headless `net-fixture` (`apps/net-fixture`, never shipped), with a
    // ceiling of `kai.test`, asks for its URLs in turn
    // (`kernel/src/net_grant_tests.rs`). Each new host waits on `desktop`'s
    // card, which names the app and the host. The keys allow two hosts, then
    // the releases host, whose download redirects to the second: each call
    // reaches its one host alone, so the redirect comes back and that host is
    // asked about by its own name (granted, with no card). A fourth host is
    // denied, and one outside the ceiling is refused with no card. Then
    // Settings' keys revoke the first grant on its Network page, the app's
    // next request asks again and is denied, and `undo` at the shell puts the
    // grant back, with no card.
    TestCase {
        dns: true,
        https: Some(Https {
            content,
            faults: &[],
        }),
        // The shell keeps the grants in `/system/state/netgrants`.
        data: Some(crate::blklog::Data {
            nvme: false,
            log: None,
            gpt: true,
        }),
        // As `app-settings`: the file system service, the shell, the desktop
        // and two comphost apps, here with `fetch` too, outgrow 512 MiB.
        memory_mib: Some(1024),
        keys: &[
            &["~600", "y", "y"],
            &["~600", "y", "y"],
            &["~600", "y", "y"],
            &["~600", "n"],
            // Settings' page tabs, Right six times to Network, its first
            // Revoke (pinned in `apps/settings-ui/tests/keys.rs`).
            &[
                "tab", "right", "right", "right", "right", "right", "right", "tab", "ret",
            ],
            &["~600", "n"],
        ],
        expect: &[
            "fetch: ready",
            "net-grant: the card names net-fixture and updates.kai.test; the key allows it",
            "net-fixture: https://updates.kai.test/hello: 200 from https://updates.kai.test/hello: kai mock: hello",
            "comphost: the first network answer reached the app",
            "net-grant: the card names net-fixture and assets.kai.test; the key allows it",
            "net-fixture: https://assets.kai.test/hello: 200 from https://assets.kai.test/hello: kai mock: hello",
            "net-grant: the card names net-fixture and releases.kai.test; the key allows it",
            "comphost: a redirect to assets.kai.test is asked about by its own name",
            "net-fixture: https://releases.kai.test/dl: 200 from https://assets.kai.test/hello: kai mock: hello",
            "net-grant: the card names net-fixture and elsewhere.kai.test; the key denies it",
            "net-fixture: https://elsewhere.kai.test/hello: refused elsewhere.kai.test",
            "net-fixture: https://other.example/hello: refused other.example",
            "net-fixture: again: net-fixture: https://updates.kai.test/hello: 200 from",
            "net-grant: no card came for a host outside the ceiling or one allowed",
            "settings: net 3 grants",
            "net-grant: the card names net-fixture and updates.kai.test; the key denies it",
            "net-fixture: again: net-fixture: https://updates.kai.test/hello: refused updates.kai.test",
            "net-grant: the Deny refused the app's request: net-fixture to updates.kai.test: refused ",
            "net-grant: undo put the revoked grant back",
            "net-fixture: done",
            "net-grant: each card named net-fixture and its host; a redirect's host was asked about by its own name; Settings' revoke was asked about again and undo gave it back with no card",
        ],
        ..case("net-grant", "test=net-grant")
    },
];
