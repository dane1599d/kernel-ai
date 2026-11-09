needed-by: P5, built 2026-11-09 (K; for A-core's task 11 part 2, after the founder's decision that Kai may reach any website the user names)
decision: A `NetDest` may name `hosts:any`, every website name, on the shell's plan line only, TCP to port 443 only, and its handle never leaves the shell
---
The shell's network ceiling (#757's `SHELL_NET`) had to list its hosts: at most 8 suffixes, with no "any host". So a website the user typed outside the list was refused by the kernel even where consent would run it. On 2026-11-09 the founder decided that Kai may reach any website the user names, so `netdest` gains `hosts:any`.

`hosts:any` names any normalised name of two labels or more whose last label is not all digits. A one-label name such as `localhost`, an address, or `1.2` is not a website. It is never `match:exact`, and a handle without it can never be narrowed into one. A `hosts:any` handle narrows, as any handle does, to one exact host or a suffix on its own ports.

The kernel keeps it where it was made (C's review): NETDEST_NARROW never makes a `hosts:any` record, even from one, and `handle::take_for`, which every move goes through (JOB_SPAWN, CALL_H, REPLY_H), refuses a handle to a `hosts:any` record. The shell keeps DUPLICATE and TRANSFER on it, because narrowing needs DUPLICATE and the narrowed handle it moves to `fetch` needs TRANSFER, but neither lets the ceiling itself leave. Which records are `hosts:any` is an atomic bitmap beside the records, since a move holds the handle table's lock and making a record takes the two locks the other way round. The `net-dest` case checks both the ring-3 narrowing and the move refusal.

Who may hold it is `supervise`'s rule, which the kernel and the supervisor both parse the plan with: only the line named `shell` running the program `shell`, with `proto:tcp`, no `addrs`, and `ports:443` alone. Any other line, an app's, a package's or a service's, fails. An agent manifest's `net` line refuses it (`kaiagent`), and so does a package ceiling handed to the shell's per-app grants (`NetGrants::decide`). An app always names its hosts.

The gates are what they were. Consent (ADR 0002 D3, D6) decides each call from what the user's line names. The shell narrows the ceiling to the one grounded host, `match:exact`, before it hands a handle to `fetch`. TLS checks that host's name. An answer that points at loopback, link-local, broadcast or multicast addresses is still refused unless the record names that address literally.

Not covered: a public name whose answer is a private address (RFC 1918) can be connected to, as with any suffix destination today. TLS name checks are what stop it reaching a device on the user's network.

The ABI does not move. No call, struct or number changes. The text form gains one value, which an in-tree plan line uses, and which NETDEST_NARROW can only receive from a handle that already has it.
