needed-by: P3 host gate (the plan's sections 1 and 6); closes `2026-11-09-p3-fuzz-gate-open.md`; written 2026-11-18 by lane G
decision: P3's fuzz gate is closed: a fuzz target exists for each of the four new P3 parsers and has run clean on a GitHub runner
---
`2026-11-09-p3-fuzz-gate-open.md` said the plan's "a fuzz target for every new parser" was unmet for `surfabi`'s message decoder, the header page, the tree records and `kai-theme 1`. `da5d2993` ("test(fuzz): P3's four fuzz targets, seeded from the hostile tests, run weekly") added them to `fuzz/fuzz_targets/` (`surfabi_stream`, `header_page`, `tree_records`, `kai_theme`), seeded from the hostile tests' samples and run by `.github/workflows/fuzz.yml` on a GitHub runner, never on a developer's PC.

**The runs, each 300 s a target, all `success`:**
- At `da5d2993`, one run per target, on 2026-11-11: `surfabi_stream` [43089893319](https://github.com/dane1599d/kernel-ai/actions/runs/43089893319), `header_page` [43089897983](https://github.com/dane1599d/kernel-ai/actions/runs/43089897983), `tree_records` [43089902528](https://github.com/dane1599d/kernel-ai/actions/runs/43089902528) and `kai_theme` [43089907367](https://github.com/dane1599d/kernel-ai/actions/runs/43089907367).
- At `121b7a8f`, the weekly schedule with every target (`all`), on 2026-11-16: [43859178821](https://github.com/dane1599d/kernel-ai/actions/runs/43859178821).

Fuzzing of these parsers at scale stays in P8, as the plan's section 9 says. The weekly run keeps them under fuzz until then.
