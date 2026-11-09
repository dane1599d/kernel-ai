needed-by: P5 task 12's harness, built 2026-11-09 (N-t; the manager asked for it first, for every P5 case that fetches; K reviews)
decision: Boot cases reach xtask's HTTPS mock by name on port 443 and trust its test CA, and ring 3 gets one shared TLS client, `user::tls`
---
Four lanes' cases fetch from the mock (pkg-online, update-online, model-download, net-pf) and so does task 12's own. So the harness is built once.

**Names and port.** The mock's five hosts are in `netcore::fixture`'s zone, all at `MOCK_ADDRESS` 10.0.2.100, with a 300 s TTL. The mock listens on a free host port, but kainet, `kai:net` and `pkg`'s hosts take only 443. So QEMU's user network forwards 10.0.2.100:443 to it through `guestfwd=...-cmd:<xtask> relay 127.0.0.1:<port>`: one `cargo xtask relay` a connection, which copies bytes both ways. No root is needed on the runner, and the kernel's rebinding rule refuses no 10.0.2.0/24 answer.

**The test CA.** It goes in that case's boot bundle as `netroots/1.der`. BOOT_READ lets a holder of NET read `netroots/<n>.der` (n from 1 to 4) with no `bootfiles=`, since they hold only public certificates. A release bundle has none, so a shipped OS trusts Mozilla's roots alone.

**The date.** A new call, NET_TIME (197, lane N's block), needs NET. It gives Limine's date at boot plus the time since, as PKG_TIME and TRUST_TIME do, because rustls checks certificate dates and no general service had a wall clock. ABI 1.3: NET_TIME and the wider BOOT_READ are additions.

**`user::tls`.** kainet's `Dial` over the NET_* calls is `NetDial` and `Tcp`. A host is resolved on the handle with RESOLVE_ON, so a name the handle does not cover is `HttpError::Refused`, and a host is reached only at an address the handle resolved. It also gives the roots (Mozilla's plus `netroots/`), ENTROPY as rustls's randomness, and NET_TIME as its clock. `fetch`, `pkg`, `updated` and the model download share it. `user` now links kainet with `roots`, so cargo-about accepts webpki-roots' CDLA-Permissive-2.0, which the founder approved in P5 question 2.

**A case's API.** `TestCase::https: Some(Https { content, faults })`: a `fn() -> mock_https::Content` (files, redirects, release assets, models, a repository) and `(host, path, Fault)` pairs. xtask then starts the mock for that boot, adds `dns=`, the forward and the CA.

**The harness's own case,** `https-mock`: `httpscheck`, with NET and a `NetDest` for three of the hosts, gets a 200 over TLS. A host its handle leaves out is refused before anything is sent, and a wrong-name and an unknown-CA certificate are refused in the handshake.
