# Plan: Kai builds apps on request

Approved by the founder 2026-11-03, with the defaults. It was proposed after spike S12-2 passed on Qwen3-4B, which scored 78% first try and 92% after one repair; the figures are in `docs/decisions.md`. Qwen3-4B stays the model, with no Granite port. These pieces run beside P4 close and don't gate rc.1. "Est." marks a judgement, not a measurement.

**Status (2026-11-09):** all five pieces are on main. Piece 5's gate builds and opens 19 of 20 requests on main (run 42649186109; the bar is 16), with the Rhai and actbench gates unchanged. No boot case drives `app_build` with the mock model yet: host tests and the gate cover it.

**The goal.** A user asks for a program in plain words: "make me a timer app", say. There is no extra button. Kai writes it, builds it, asks once whether to install it, then opens it. One undo removes it.

## The flow

1. **Ask.** The act model reads the request as an app request and calls the new `app_build(name, request)` row. If it is unclear, it asks one question first, as it does for ambiguous requests today: "A countdown or a stopwatch?" Plain requests stay on today's tagged calls and Rhai.
2. **Write.** Kai's model gets the `c-app-1` card. It is `c-1`'s rules, plus an app template that the model fills in rather than writing from scratch:
   - `struct state` (what the app remembers);
   - `init` (the first state);
   - `on_event` (a key, a click, a tick);
   - `view` (the window's tree);
   - the kai.h calls the app may use, each with its Use line.
   
   The card is measured against the 980 bar on its own, as `c-1` was.
3. **Check and build.**
   - **Check:** the S12-2 pre-check runs first: includes, names, `Print` formats.
   - **Build:** then `cc build`, as ADR 0012 has it. It targets the `kai:app/component` world, with no actions of the app's own, which is v0.1's scope.
   - **Repair:** up to 3 repairs, each fed the exact compiler errors.
   - **Trial:** a build that compiles gets a trial run on stub imports, as task 14 has it, and a failing trial is repaired too.
4. **Install ask card.** One card shows:
   - the app's name and its one-line summary;
   - what it may use: the grants its world imports;
   - its size;
   - "Show code".
   
   **Yes** installs it from ADR 0011's local repository, signed by the device key. The source is kept in `/home/user/apps/<name>/`.
5. **Open.** It opens at once in a window, like any installed app.
6. **Undo.** `undo` (or "remove it") uninstalls it, using the package transaction from ADR 0011, and keeps the source. A rebuild after an edit is a new version, so undo goes back one version.

## Rules

- **Tiers:** `app_build` writes, builds and trials at the run tier. Installing is always an ask, because a new app is a new program on the machine. The app's own calls keep their tiers, and every `kai` import goes through the one door.
- **Records:** the log and `:why` record the request, the source's hash and the build, under the ledger's `built-by cc` provenance (ADR 0012).
- **No new grants:** a model-written app gets no grant that a hand-written app could not, and nothing the user did not see on the card.

## Work and order

| # | Piece | Who | Size (est.) |
|---|---|---|---|
| 1 | P4 task 14 as planned: `script` gains `lang`, and `c-1`, the pre-check and the repairs move from the spike into `llmproto` and the shell | A-core and model lane, with T | S (~600) |
| 2 | `c-app-1` card and app template; a host test builds every template example with `cc build` | Model lane, T | S (~500) |
| 3 | The `app_build` row: write to `/home/user/apps/<name>/`, build, trial, repair loop | A-core, T | M (~900) |
| 4 | Install ask card, open, undo through the local repository | A-core, U | S (~500) |
| 5 | **Gate:** 20 app requests on a GitHub runner (timer, counter, notes list, unit converter, and so on). It must build and open in at least 80% of requests after repairs; installed apps pass a scripted click test; the Rhai and actbench gates are unchanged | Model lane | S (~400) |

That comes to about 2,900 lines with tests (est.). Pieces 1 and 2 can start at once, and pieces 3 and 4 follow piece 1. Nothing runs a model on the founder's PC.

## Questions for the founder

Answered 2026-11-03: all three defaults stand.

1. **Install tier.** Always ask before installing a model-written app? **Default: yes, always ask.**
2. **Where the source lives.** **Default: `/home/user/apps/<name>/`, shown by "Show code" and kept after removal.**
3. **Changing an app later** ("make the timer beep"). **Default: yes. It rebuilds as a new version, with the same ask before it replaces the old one.**
