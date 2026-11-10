//! The fixture as built for wasm32, run in `wasmhost` with no OS: it asks for
//! its first URL when it starts, and each answer the host gives is logged and
//! moves it on to the next. Skipped when `cargo build -p comp-net-fixture
//! --target wasm32-unknown-unknown --profile component` has not run (CI runs
//! it first).

use comp_net_fixture::URLS;
use livecore::Instance;
use livecore::txn::Storage;
use std::cell::RefCell;
use wasmhost::net::{NetError, NetRequest, NetResponse};
use wasmhost::{CoreInstance, Grants, Limits, load_core, load_core_granted};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn log(line: &str) {
    LOG.with(|l| l.borrow_mut().push(line.to_string()));
}

fn lines() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

fn wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/wasm32-unknown-unknown/component/comp_net_fixture.wasm"
    );
    let wasm = std::fs::read(path).ok();
    if wasm.is_none() {
        eprintln!("skipped: build comp-net-fixture for wasm32 first");
    }
    wasm
}

fn granted() -> Grants {
    Grants {
        net: true,
        ..Grants::default()
    }
}

/// The request the fixture queued, with its ticket.
fn taken(g: &mut CoreInstance) -> (u32, NetRequest) {
    let mut taken = g.take_net();
    assert_eq!(taken.len(), 1, "{taken:?}");
    taken.remove(0)
}

#[test]
fn without_the_grant_it_does_not_link() {
    let Some(wasm) = wasm() else { return };
    assert!(load_core(&wasm, LIMITS, None).is_err());
}

#[test]
fn it_asks_for_each_url_in_turn_and_logs_what_each_came_to() {
    let Some(wasm) = wasm() else { return };
    let mut g = load_core_granted(&wasm, LIMITS, Some(log), granted()).unwrap();
    g.view(&Storage::new());
    assert_eq!(g.init(), Ok(0));
    let (ticket, request) = taken(&mut g);
    assert_eq!(
        request,
        NetRequest {
            url: String::from(URLS[0]),
            head: false,
            range: None,
        }
    );
    let response = NetResponse {
        status: 200,
        url: String::from(URLS[0]),
        content_type: Some(String::from("text/plain")),
        retry_after: None,
        body: b"kai mock: hello\n".to_vec(),
    };
    g.net_response(ticket, Ok(response)).unwrap();
    assert_eq!(
        lines(),
        [
            "net-fixture: https://updates.kai.test/hello: 200 from https://updates.kai.test/hello: kai mock: hello"
        ]
    );
    for (at, url) in URLS.iter().enumerate().skip(1) {
        let (ticket, request) = taken(&mut g);
        assert_eq!(request.url, *url);
        let error = NetError::Refused(format!("host{at}.kai.test"));
        g.net_response(ticket, Err(error)).unwrap();
        assert_eq!(
            lines(),
            [format!("net-fixture: {url}: refused host{at}.kai.test")]
        );
    }
    // Then the first URL again, a line only when what comes changes, until a
    // response follows a refusal.
    let again = |g: &mut CoreInstance, outcome| {
        let (ticket, request) = taken(g);
        assert_eq!(request.url, URLS[0]);
        g.net_response(ticket, outcome).unwrap();
        lines()
    };
    let ok = || {
        Ok(NetResponse {
            status: 200,
            url: String::from(URLS[0]),
            content_type: None,
            retry_after: None,
            body: b"kai mock: hello".to_vec(),
        })
    };
    let refused = || Err(NetError::Refused(String::from("updates.kai.test")));
    assert_eq!(
        again(&mut g, ok()),
        [
            "net-fixture: again: net-fixture: https://updates.kai.test/hello: 200 from https://updates.kai.test/hello: kai mock: hello"
        ]
    );
    assert!(again(&mut g, ok()).is_empty());
    assert_eq!(
        again(&mut g, refused()),
        [
            "net-fixture: again: net-fixture: https://updates.kai.test/hello: refused updates.kai.test"
        ]
    );
    assert!(again(&mut g, refused()).is_empty());
    assert_eq!(
        again(&mut g, ok()).last().map(String::as_str),
        Some("net-fixture: done")
    );
    assert!(!g.net_waiting());
}
