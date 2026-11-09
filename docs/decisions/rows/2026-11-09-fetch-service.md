needed-by: P5 task 12, built 2026-11-09 (N-t; the seam agreed with C for task 13 and A-core for task 11, and the manager OK'd it; K reviews the service, C reviews kainet)
decision: The `fetch` service makes each request on the caller's own `NetDest`, holds none of its own and no grants, follows only redirects within that handle, and hands any other back
---
**Who decides.**
- comphost (task 13) and the shell (task 11) decide whether a host may be reached: a grant, or grounding. Then they move in a `NetDest` narrowed to exactly that host, with RESOLVE+CONNECT+TRANSFER, one per call.
- `fetch` keeps no list. It closes NET's own startup handle at once, and each caller's handle when the call ends.
- The kernel decides what the handle reaches. Before following a redirect, `fetch` asks it with RESOLVE_ON on that handle.
- The certificate is checked for the URL's host, which resolved on that handle.

**Redirects** (C, #750's row).
- A redirect within the handle is followed.
- One to another host comes back unfollowed, as its 3xx with no body and a `location` made absolute (`kainet::fetch::serve_within`, on `Fetch::run_within`). The caller asks about that host, narrows a handle for it, and calls again with `hops` + 1.
- One limit of 5 (`MAX_HOPS`) holds across the calls.
- A request whose own host is off the handle is `err refused <host>`.

**The messages** (`kainet::wire`, one parser for `fetch`, comphost and the shell).
- A request is one line: `get <url> [range=a-[b]] [hops=n]` or `head <url> [hops=n]`. https only, port 443, URL at most 2,048 bytes.
- An answer is `err <kind> [why]`. The kinds are refused, bad-url, tls, http, too-large, timeout and truncated, with why at most 200 printable bytes. Otherwise it is `ok <status>` and one field a line: `len`, then `type` and `retry` when the server sent them, then `url`, and `location` on an unfollowed redirect. A Content-Type can hold spaces, so it gets a line of its own.

**The body** (at most 64 KiB) comes in an `Shm` that `fetch` makes (16 pages) and moves back with the answer. `fetch` never writes into memory a caller sized: no call reports an `Shm`'s size, and a short one would fault the service.

**One call at a time.** Each call has 30 s, and callers queue in the kernel. So comphost and the shell call `fetch` from a thread of their own.

**Where it runs.** `fetch` is on the float target, where `kainet` runs its AES-NI paths. `kabi::plan::FETCH` is its line: on demand, with SERVE+NET. The first caller's PR puts that line in the system plan, ahead of its own `uses=…+fetch`.

**The case.** `fetch-service` starts `fetch` with SERVE+NET and runs `httpscheck` (`via=fetch`) as a caller with a route to it. The caller narrows one exact host per call. It gets:
- a 200 with its body in the `Shm`;
- a same-host redirect followed;
- a releases-to-assets redirect handed back, then fetched on an assets handle with `hops=1`;
- a host off the handle refused.

The app-level case, an app fetching from its host and refused another, comes with comphost's side (C, task 13 part 2), since it needs the `kai:net` import in wasmhost and the grant check.
