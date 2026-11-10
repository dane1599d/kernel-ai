needed-by: P4, built 2026-11-09 (T; the manager asked for it after #685's app gate, run 41880887951, and the model lane chose the srand line)
decision: wcc's hints name what to write for `atoi` and `strtol`, and for a window app's `srand(time(NULL))`
---
After #685, the gate's one miss, `guess`, read its field with `atoi` and got only the generic hint (define it). No library here has `atoi`, `atol`, `atoll`, `strtol`, `strtoll`, `strtoul` or `strtoull`, at `c>` or in a build. So `ccserve::hints` now says to read the digits, `n = n * 10 + (c - '0')` for each digit after an optional `-`. It says so for a call to any of them, and for the link's `Unresolved`.

`rand`, the same app's other miss, is #701's (merged): `<kaiapp.h>`'s `int rand(void)` draws from the call's seed, and it is kept off the card. So in a window app, `srand`, `srandom` and `time`, which `srand(time(NULL))` calls, get one hint: `rand()` needs no `srand`, since it draws from the call's own seed, so remove both. The model lane chose a hint over new API: a no-op `srand` or a `time()` would make an app's start depend on the wall clock. Elsewhere `srand` is still told to keep a seed.

Model input changes, so the app gate and C fluency run again on it.
