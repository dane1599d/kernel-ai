"""S14-1's report from measure.sh's raw lines, as Markdown.

Bars (ADR 0014, S14-1):
- ahead-of-time code at most 2x native (geometric mean of warm medians, and
  each fixture's ratio shown);
- the Wikipedia-shaped `article` to first paint in at most 400 ms on AOT: a
  fresh process's load + instantiate + its first render;
- peak guest memory at most 256 MiB.
wasmi's and Pulley's figures are recorded, not judged.
"""

import math
import re
import statistics
import sys
from collections import defaultdict

FIELD = re.compile(r"(\w+)=(\S+)")
MODES = ["native", "aot", "aot-scalar", "pulley", "wasmi"]


def pixel_cell(groups, failed):
    cell = "all same" if len(groups) == 1 else "; ".join(", ".join(ms) for ms in groups.values())
    if failed:
        cell += " (failed: " + ", ".join(failed) + ")"
    return cell


def main(path):
    runs = defaultdict(list)  # (mode, fixture) -> [fields]
    host = {}  # (mode, fixture) -> fields
    compiled = {}
    for line in open(path):
        words = line.split()
        if len(words) < 2:
            continue
        mode = words[0]
        fields = dict(FIELD.findall(line))
        if mode.startswith("compile-"):
            compiled[mode[len("compile-"):]] = fields
        elif words[1] == "s14":
            runs[(mode, words[2])].append(fields)
        elif words[1] == "s14-host":
            host[(mode, fields.get("fixture"))] = fields
    fixtures = []
    for _, f in runs:
        if f not in fixtures:
            fixtures.append(f)
    modes = [m for m in MODES if any((m, f) in runs for f in fixtures)]

    def warm(mode, f):
        rs = runs.get((mode, f), [])
        rest = rs[1:] or rs
        return statistics.median(int(r["total_us"]) for r in rest) if rest else None

    out = ["# S14-1 `engine-in-wasm` results", ""]
    out.append("Warm median of runs 2..N, total of parse + style/layout + paint, in ms (ratio to native).")
    out.append("")
    out.append("| Fixture | HTML bytes | " + " | ".join(modes) + " | Pixels (modes that agree) |")
    out.append("|---" * (len(modes) + 3) + "|")
    ratios = defaultdict(list)
    for f in fixtures:
        base = warm("native", f)
        cells = []
        for m in modes:
            w = warm(m, f)
            if w is None:
                cells.append("-")
                continue
            if base and m != "native":
                ratios[m].append(w / base)
                cells.append(f"{w / 1000:.1f} ({w / base:.2f}x)")
            else:
                cells.append(f"{w / 1000:.1f}")
        groups = defaultdict(list)
        for m in modes:
            hashes = {r["pixels"] for r in runs.get((m, f), [])}
            if hashes:
                groups["/".join(sorted(hashes))].append(m)
        failed = [m for m in modes if "error" in host.get((m, f), {})]
        size = runs[(modes[0], f)][0]["bytes"] if runs.get((modes[0], f)) else "-"
        out.append(f"| {f} | {size} | " + " | ".join(cells) + f" | {pixel_cell(groups, failed)} |")
    out.append("")
    out.append("## Phases, warm medians in ms")
    out.append("")
    out.append("| Fixture | Mode | parse | style + layout | paint |")
    out.append("|---|---|---|---|---|")
    for f in fixtures:
        for m in modes:
            rs = runs.get((m, f), [])
            rest = rs[1:] or rs
            if not rest:
                continue
            ph = [statistics.median(int(r[k]) for r in rest) / 1000 for k in ("parse_us", "layout_us", "paint_us")]
            out.append(f"| {f} | {m} | {ph[0]:.1f} | {ph[1]:.1f} | {ph[2]:.1f} |")
    out.append("")
    out.append("## Bars")
    out.append("")
    verdict = []
    if "aot" in ratios:
        gm = math.exp(statistics.mean(math.log(r) for r in ratios["aot"]))
        worst = max(ratios["aot"])
        ok = gm <= 2.0
        verdict.append(ok)
        out.append(f"- AOT at most 2x native: geometric mean **{gm:.2f}x**, worst fixture {worst:.2f}x: {'met' if ok else 'NOT met'}.")
    for m in ("aot-scalar", "pulley", "wasmi"):
        if m in ratios:
            gm = math.exp(statistics.mean(math.log(r) for r in ratios[m]))
            out.append(f"- {m} (recorded only): geometric mean {gm:.2f}x native, worst {max(ratios[m]):.2f}x.")
    for m in modes:
        rs = runs.get((m, "article"))
        if not rs:
            continue
        h = host.get((m, "article"), {})
        first = (int(h.get("load_us", 0)) + int(h.get("instantiate_us", 0)) + int(rs[0]["total_us"])) / 1000
        line = f"- `article` first paint on {m}, fresh process (load + instantiate + first render): **{first:.0f} ms**"
        if m == "aot":
            ok = first <= 400
            verdict.append(ok)
            line += f": {'met' if ok else 'NOT met'} (bar 400 ms)"
        out.append(line + ".")
    peaks = {m: max((int(h["peak_guest_mib"]) for (hm, _), h in host.items() if hm == m and "peak_guest_mib" in h), default=None) for m in modes}
    rss = {m: max((int(h["peak_rss_mib"]) for (hm, _), h in host.items() if hm == m and "peak_rss_mib" in h), default=None) for m in modes}
    guest = [p for m, p in peaks.items() if p is not None]
    if guest:
        ok = max(guest) <= 256
        verdict.append(ok)
        out.append(f"- Peak guest linear memory at most 256 MiB: **{max(guest)} MiB** (largest over fixtures and runtimes): {'met' if ok else 'NOT met'}.")
    out.append("- Peak RSS of the whole process, MiB: " + ", ".join(f"{m} {v}" for m, v in rss.items() if v is not None) + ".")
    for m, c in compiled.items():
        out.append(f"- CI's compile for {m}: {c.get('compile_ms')} ms, `.cwasm` {int(c.get('cwasm_bytes', 0)) / 1048576:.1f} MiB.")
    out.append("")
    out.append(f"**Verdict: {'pass' if verdict and all(verdict) else 'fail'}** on this machine.")
    print("\n".join(out))


if __name__ == "__main__":
    main(sys.argv[1])
