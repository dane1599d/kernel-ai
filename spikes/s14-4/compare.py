"""S14-4: compares two capture runs and writes the report.

A site passes when its text is the same on both runs. Text is compared line
by line, after trimming each line and dropping empty ones: the same words in
the same reading order. For a site that differs, the report gives:
- how many lines changed;
- how much of its word set the two runs share;
- a diff file in the artifact.

That says whether the harness wavered, or the page itself changed between
runs (a news front page, a forecast).

Usage: python3 compare.py RUN1 RUN2 OUT
Writes OUT/report.md and OUT/diffs/<name>.diff.
"""

import difflib
import json
import os
import sys


def lines(path):
    try:
        with open(path, encoding="utf-8") as f:
            return [l.strip() for l in f.read().splitlines() if l.strip()]
    except FileNotFoundError:
        return None


def pages(run):
    with open(os.path.join(run, "pages.json"), encoding="utf-8") as f:
        records = json.load(f)
    firefox = next((r["firefox"] for r in records if "firefox" in r), "?")
    return {r["name"]: r for r in records if "name" in r}, firefox


def main(run1, run2, out):
    a, firefox = pages(run1)
    b, _ = pages(run2)
    os.makedirs(os.path.join(out, "diffs"), exist_ok=True)
    rows, same = [], 0
    for name, first in a.items():
        second = b.get(name, {})
        x = lines(os.path.join(run1, f"{name}.txt"))
        y = lines(os.path.join(run2, f"{name}.txt"))
        if x is None or y is None:
            why = first.get("error") or second.get("error") or "not captured"
            rows.append((name, first.get("words", "-"), second.get("words", "-"), "ERROR", why))
            continue
        if x == y:
            same += 1
            rows.append((name, first["words"], second["words"], "same", ""))
            continue
        diff = list(difflib.unified_diff(x, y, "run-1", "run-2", lineterm="", n=1))
        changed = sum(1 for d in diff if d[:1] in "+-" and d[:3] not in ("+++", "---"))
        wx, wy = set(" ".join(x).split()), set(" ".join(y).split())
        shared = len(wx & wy) / max(1, len(wx | wy))
        with open(os.path.join(out, "diffs", f"{name}.diff"), "w", encoding="utf-8") as f:
            f.write("\n".join(diff) + "\n")
        rows.append((name, first["words"], second["words"], "DIFFERS", f"{changed} lines changed, {shared:.1%} of words shared"))
    total = len(rows)
    out_md = [
        "# S14-4 `web-compare`",
        "",
        f"Firefox {firefox}, headless, 1280x800, every cookie refused, read-only. Two runs over plan §4.4's tier 1, one after the other.",
        "",
        f"**Same text on both runs: {same} of {total} sites ({same / max(1, total):.0%}).**",
        "",
        "| Site | Words, run 1 | Words, run 2 | Text | Note |",
        "|---|---|---|---|---|",
    ]
    out_md += [f"| {r[0]} | {r[1]} | {r[2]} | {r[3]} | {r[4]} |" for r in rows]
    with open(os.path.join(out, "report.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(out_md) + "\n")
    print("\n".join(out_md))


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
