//! `test=net-grant` (P5 task 13's exit case): an app's HTTPS through
//! `kai:net`, asked per host on the trusted path's card. The headless test
//! app `net-fixture` (`apps/net-fixture`, never shipped) runs under comphost
//! with a ceiling of `kai.test` and a route to `fetch`, and asks for a fixed
//! list of URLs on the HTTPS mock (`xtask/src/mock_https`). comphost asks the
//! shell through `desktop` for each host; the shell puts a card on
//! `desktop`'s ask box, which the kernel's terminal takes, naming the app and
//! the host, and the harness answers it with a key:
//!
//! 1. `updates.kai.test`: `y`. The request is made on a handle narrowed to
//!    that host and the body comes back.
//! 2. `releases.kai.test`: `y`. Its download redirects to `assets.kai.test`,
//!    which the handle does not reach, so `fetch` hands the redirect back,
//! 3. and comphost asks about `assets.kai.test` by its own name: `y`. The
//!    second call, with one hop counted, brings the body.
//! 4. `elsewhere.kai.test`: `n`. The app is told `refused`.
//!
//! Then `other.example`, outside the ceiling, is refused with no card, and
//! `updates.kai.test` again is granted with no card. The app's own log lines
//! say what each request came to; this check waits for the four cards, checks
//! that each names the app and its host, and that no fifth comes.

use crate::display::claim;
use crate::{ask_box, log, terminal, threads};

/// The shell with `files` and `state`, where it keeps the grants
/// (`/system/state/netgrants`), and its route to `desktop`.
pub const SHELL: &str = kabi::plan::SHELL_DESK_FILES;

/// The test app: comphost with NET (to hold its ceiling) and THREADS (for the
/// worker that calls `fetch`), its ceiling `kai.test` on 443, and routes to
/// `desktop` then `fetch`, after both. It starts at boot and asks at once.
pub const APP: &str = "net-fixture program=comphost caps=serve+component+call+net+threads uses=desktop+fetch netdest=proto:tcp,ports:443,hosts:kai.test pages=8192 stack_pages=512 burst_ms=20000 version=1";

/// The cards, in the order the app's requests raise them, and whether the
/// harness's key allows each (`y`) or not (`n`).
const CARDS: [(&str, bool); 4] = [
    ("updates.kai.test", true),
    ("releases.kai.test", true),
    ("assets.kai.test", true),
    ("elsewhere.kai.test", false),
];
/// How long a card may take to come: the network, a TLS handshake under
/// emulation, and comphost's next ask.
const CARD_MS: u64 = 120_000;
/// How long the app's last two requests may take once the last card is
/// answered, and the time a fifth card would have had to show.
const SETTLE_MS: u64 = 20_000;
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

pub fn check() -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    wait("desktop to read its ask box", || {
        claim::showing() && ask_box::alive()
    })?;
    for (host, allow) in CARDS {
        wait("a card for the app's next host", ask_box::shown)?;
        let question = ask_box::question().unwrap_or_default();
        if !question.contains("net-fixture") || !question.contains(host) {
            log!("net-grant: the card asked {question:?}, not about net-fixture and {host}");
            return Err("net-grant: a card did not name the app and its host");
        }
        let key = if allow { "allows" } else { "denies" };
        log!("net-grant: the card names net-fixture and {host}; the key {key} it");
        log!("net-grant: ready for keys");
        wait("the card's answer", || !ask_box::shown())?;
    }
    // `other.example` is refused with no card, and `updates.kai.test` again
    // is granted with none.
    for _ in 0..SETTLE_MS / POLL_MS {
        if ask_box::shown() {
            let question = ask_box::question().unwrap_or_default();
            log!("net-grant: a fifth card asked {question:?}");
            return Err("net-grant: a host outside the ceiling, or one allowed, was asked about");
        }
        threads::sleep_ms(POLL_MS);
    }
    log!(
        "net-grant: four cards named net-fixture and each host, a redirect's host was asked about by its own name, and no card came for a host outside the ceiling or one allowed"
    );
    Ok(())
}
