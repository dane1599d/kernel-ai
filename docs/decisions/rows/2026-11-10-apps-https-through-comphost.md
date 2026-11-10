needed-by: P5 tasks 12 and 13, built 2026-11-10 (lane C; K reviews)
decision: An app's HTTPS is `kai:net/http`, granted by a `netdest=` ceiling on its plan line; comphost asks the shell per host through `desktop`, narrows the ceiling to that one host for each call and calls `fetch` from a worker thread
---
**The interface.** `kai:net@0.1.0` (`wit/kai-net/http.wit`, frozen) moves from the draft in `kainet/wit/`. It changes only in its words: grants are per host and asked by name, not suffixes.
- `fetch` queues a request, as `kai:models`' `read` does.
- The answer comes in a later call, `on-response`, to the instance that asked. An answer for a version no longer shown is dropped.
- A denied host is `error.refused(<host>)`, as is a host outside the ceiling.

**The grant.** `wasmhost::Grants::net` links the import. comphost sets it when three things hold:
- its plan line names a ceiling (`netdest=`), which needs NET to hold;
- its routes are `desktop` then `fetch`;
- it is not the System monitor or the model manager, whose second route is their own.

Only the image's own versions get it, as with every grant until package grants exist. So the `kai-app` manifest's `net` line, which would put `netdest=` on a package's line, waits for package grants.

**The ceiling's text.**
- The supervisor now passes a line's `netdest=` to its program as an argument, because a process cannot read a handle's destination back.
- comphost names that text to the shell as the app's ceiling. The kernel holds every narrowing to the handle whatever the text says.
- NET also starts a process with NET's own handle to the LLM server (CONNECT only). comphost closes it and keeps the handle with RESOLVE.

**The ask's transport.** `deskcore::net_wire` carries the ask, because comphost has no route to the shell.
- comphost sends `net ask <host>\n<ceiling>` and gets `wait`, or the shell's answer taken once.
- The shell sends `net take` and `net done`, each pass. `desktop` holds at most 16 asks and 16 answers.
- The app is `desktop`'s record of the caller, never a word in the request.
- While the answer is `wait` or `asked`, comphost asks again every 500 ms, for at most 120 s, then answers `timeout`.

**A Deny.** The person's Deny holds for a minute: the app's connections to that host are refused, naming it, with no card. After that the host may be asked about again.
- An app that asks again at once does not raise the card again at once.
- A grant comes first: a grant `undo` puts back is used even within the minute.

**The call.**
- comphost narrows its ceiling, for each call, to that call's one host (`net_wire::call_dest`, `match:exact`), with RESOLVE, CONNECT and TRANSFER. It never narrows to the app's whole list of granted hosts (the audit of tasks 12 and 13).
- It moves that handle to `fetch` with the request line. `fetch` takes one call at a time and blocks up to 30 s, so comphost makes the call from one worker thread (THREADS), and its main thread keeps serving the app.
- So a redirect to another host comes back even when the app holds a grant for that host. It is asked about by its new host's name and called again with the hops so far, so one limit of five holds across calls.

**The case.** `net-grant` runs the headless test app `net-fixture` with a ceiling of `kai.test`. Keys answer the cards, each checked to name the app and its host:
- allow `updates`;
- allow `assets`;
- allow `releases`, whose download redirects to `assets`: it comes back from the call and is granted with no card;
- deny `elsewhere`.

`other.example`, outside the ceiling, is refused with no card. Then:
1. Settings starts and its keys revoke `updates` on the Network page.
2. The app's next request asks again, and is denied.
3. `undo`, typed at the shell, puts the grant back, and the app is answered with no card.
