//! P5 task 12's harness (lane N-t): xtask's HTTPS mock in the boot cases, which
//! every P5 case that fetches builds on (`pkg-online`, `update-online`,
//! `model-download`, `net-pf`, fetch-from-host). A case sets
//! [`TestCase::https`] to what the mock serves and the faults it starts with;
//! xtask then starts the mock for that boot, adds `dns=`, forwards the guest's
//! connections to `netcore::fixture::MOCK_ADDRESS` on port 443 to it, and puts
//! its test CA in the boot bundle as `netroots/1.der`, which a holder of NET
//! trusts through `user::tls`.

use super::{Https, TestCase, case};
use crate::mock_https::{ASSETS, Content, Fault, OFF, RELEASES, REPO};

/// What every host of the harness's own case serves at `/hello`.
const HELLO: &[u8] = b"kai mock: hello\n";

fn hello() -> Content {
    [REPO, RELEASES, ASSETS, OFF]
        .into_iter()
        .fold(Content::new(), |content, host| {
            content.file(host, "/hello", HELLO.to_vec())
        })
}

pub const CASES: &[TestCase] = &[
    // The harness itself, from ring 3 (`kernel/src/https_tests.rs`): a GET over
    // TLS with the test CA, a host the handle leaves out refused, and a wrong
    // name and an unknown CA refused in the handshake.
    TestCase {
        dns: true,
        https: Some(Https {
            content: hello,
            faults: &[
                (RELEASES, "/hello", Fault::WrongName),
                (ASSETS, "/hello", Fault::UnknownCa),
            ],
        }),
        expect: &[
            "[https-get] httpscheck: GET https://updates.kai.test/hello: 200: kai mock: hello",
            "[https-refused] httpscheck: GET https://elsewhere.kai.test/hello: refused, the handle does not name elsewhere.kai.test",
            "[https-name] httpscheck: GET https://releases.kai.test/hello: the handshake was refused",
            "[https-ca] httpscheck: GET https://assets.kai.test/hello: the handshake was refused",
            "https-mock: the mock's hosts resolve and answer over TLS with its test CA",
        ],
        ..case("https-mock", "test=https-mock")
    },
];

#[cfg(test)]
mod tests {
    use crate::mock_https::HOSTS;

    /// The guest resolves every name the mock serves, to the forwarded address.
    #[test]
    fn every_mock_host_is_in_the_dns_fixture() {
        assert_eq!(HOSTS, netcore::fixture::MOCK_HOSTS);
    }
}
