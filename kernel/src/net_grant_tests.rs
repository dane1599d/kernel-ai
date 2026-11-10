//! `test=net-grant` (P5 task 13's exit case): an app's HTTPS through
//! `kai:net`, asked per host on the trusted path's card. The headless test
//! app `net-fixture` (`apps/net-fixture`, never shipped) runs under comphost
//! with a ceiling of `kai.test` and a route to `fetch`, and asks for a fixed
//! list of URLs on the HTTPS mock (`xtask/src/mock_https`). comphost asks the
//! shell through `desktop` for each host; the shell puts a card on
//! `desktop`'s ask box, which the kernel's terminal takes, naming the app and
//! the host, and the harness answers it with a key:
//!
//! 1. `updates.kai.test`: `y`, and the body comes back.
//! 2. `assets.kai.test`: `y`, and the body comes back.
//! 3. `releases.kai.test`: `y`. Its download redirects to `assets.kai.test`,
//!    which the app holds a grant for, but each call reaches its one host
//!    alone: `fetch` hands the redirect back, comphost asks about
//!    `assets.kai.test` by its own name, it is granted with no card, and the
//!    second call brings the body.
//! 4. `elsewhere.kai.test`: `n`. The app is told `refused`.
//!
//! `other.example`, outside the ceiling, is then refused with no card, and the
//! app asks for `updates.kai.test` over and over. The case starts Settings,
//! whose keys revoke that grant on its Network page; the app's next request
//! raises a card for `updates.kai.test` again, which `n` denies (the app is
//! told `refused`, and with no card while the Deny holds). `undo`, typed at
//! the shell as a person would, puts the revoked grant back, and the app is
//! answered again with no card. The app's own lines say what each request
//! came to; this check follows the cards, checks each names the app and its
//! host, and that no other comes.

use crate::agent_desk_tests::{START, click};
use crate::display::claim;
use crate::{ask_box, ipc, log, terminal, threads};

/// The shell with `files` and `state`, where it keeps the grants
/// (`/system/state/netgrants`), and its route to `desktop`.
pub const SHELL: &str = kabi::plan::SHELL_DESK_FILES;

/// The test app: comphost with NET (to hold its ceiling), its ceiling
/// `kai.test` on 443, and routes to
/// `desktop` then `fetch`, after both. It starts at boot and asks at once.
/// Then Settings, started on demand once the grants exist: it reads them when
/// it starts.
pub const APPS: &str = "net-fixture program=comphost caps=serve+component+call+net uses=desktop+fetch netdest=proto:tcp,ports:443,hosts:kai.test pages=8192 stack_pages=512 burst_ms=20000 version=1\n\
     settings-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand";

/// The cards, in the order the app's requests raise them, and whether the
/// harness's key allows each (`y`) or not (`n`).
const CARDS: [(&str, bool); 4] = [
    ("updates.kai.test", true),
    ("assets.kai.test", true),
    ("releases.kai.test", true),
    ("elsewhere.kai.test", false),
];
/// The host whose grant Settings revokes, the first listed.
const REVOKED: &str = "updates.kai.test";
/// How long a card may take to come: the network, a TLS handshake under
/// emulation, and comphost's next ask.
const CARD_MS: u64 = 120_000;
/// Time with no card expected: the last list request and the first of the
/// requests again; then the time the app's answers have after the `undo`.
const QUIET_MS: u64 = 10_000;
/// How long the Deny is left to reach the app before the `undo`: many of
/// comphost's 500 ms asks, well inside the Deny's minute.
const DENIED_MS: u64 = 5_000;
/// Settings' start: comphost's, the component's compile, and its reads.
const OPEN_MS: u64 = 60_000;
const SETTINGS_READS_MS: u64 = 5_000;
/// Settings' title bar: its frame is the first, at 48,52 (the app has no
/// window), and the click lands 8 px down and 60 px across.
const SETTINGS_TITLE: (i32, i32) = (108, 60);
const POLL_MS: u64 = 50;

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..CARD_MS / POLL_MS {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(POLL_MS);
    }
    log!("net-grant: timed out waiting for {what}");
    Err(what)
}

/// The next card, which must name the app and `host`, answered by the
/// harness's next keys (`allow` says which).
fn card(host: &str, allow: bool) -> Result<(), &'static str> {
    wait("a card for the app's next host", ask_box::shown)?;
    let question = ask_box::question().unwrap_or_default();
    if !question.contains("net-fixture") || !question.contains(host) {
        log!("net-grant: the card asked {question:?}, not about net-fixture and {host}");
        return Err("net-grant: a card did not name the app and its host");
    }
    let key = if allow { "allows" } else { "denies" };
    log!("net-grant: the card names net-fixture and {host}; the key {key} it");
    log!("net-grant: ready for keys");
    wait("the card's answer", || !ask_box::shown())
}

/// No card comes for `ms`.
fn quiet(ms: u64) -> Result<(), &'static str> {
    for _ in 0..ms / POLL_MS {
        if ask_box::shown() {
            let question = ask_box::question().unwrap_or_default();
            log!("net-grant: a card no step expects asked {question:?}");
            return Err("net-grant: a host was asked about that should not have been");
        }
        threads::sleep_ms(POLL_MS);
    }
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    wait("desktop to read its ask box", || {
        claim::showing() && ask_box::alive()
    })?;
    for (host, allow) in CARDS {
        card(host, allow)?;
    }
    // `other.example` is refused with no card, and the first host again is
    // granted with none.
    quiet(QUIET_MS)?;
    log!("net-grant: no card came for a host outside the ceiling or one allowed");
    // Settings starts with the first call to it, and reads the grants.
    ipc::call(
        "settings-window",
        livecore::rows::REQUEST.as_bytes(),
        OPEN_MS,
    )
    .map_err(|_| "net-grant: Settings did not start")?;
    threads::sleep_ms(SETTINGS_READS_MS);
    let mut at = START;
    click(&mut at, SETTINGS_TITLE);
    log!("net-grant: Settings is open; its keys revoke the first grant");
    log!("net-grant: ready for keys");
    // The revoke holds: the app's next request asks again, and is denied.
    card(REVOKED, false)?;
    // The Deny reaches the app at comphost's next ask (every 500 ms), and holds
    // while the app asks again, with no card. Typed sooner, the `undo` would
    // put the grant back before the app was told (#771's run 42839655660).
    quiet(DENIED_MS)?;
    // The person's `undo` puts the revoked grant back.
    let undone = ipc::call("shell", b"undo", CARD_MS)
        .map_err(|_| "net-grant: the shell did not answer undo")?;
    let undone = alloc::string::String::from_utf8_lossy(&undone);
    if !undone.contains("may connect to updates.kai.test again") {
        log!("net-grant: undo answered {undone:?}");
        return Err("net-grant: undo did not put the grant back");
    }
    log!("net-grant: undo put the revoked grant back: {undone}");
    quiet(QUIET_MS)?;
    log!(
        "net-grant: each card named net-fixture and its host; a redirect's host was asked about by its own name; Settings' revoke was asked about again and undo gave it back with no card"
    );
    Ok(())
}
