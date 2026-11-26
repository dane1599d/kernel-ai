#!/usr/bin/env python3
"""Spike G13's runs: each PDF of corpus/index.tsv rendered by hayro-render
and by pdftoppm at its dpi (at most 50 pages), each under `/usr/bin/time`
for wall time and peak memory and `timeout` for a hang, then the pages
compared by pdfdiff. Writes out/results.tsv and out/results.md (also to the
job's summary when $GITHUB_STEP_SUMMARY is set).

Usage: drive.py <corpus-dir> <out-dir> <hayro-render> <pdfdiff>
"""

import os
import shutil
import subprocess
import sys

MAX_PAGES = 50
TIMEOUT = 300


def timed(cmd, log):
    """Runs `cmd` under time and timeout: (exit, seconds, peak MiB, stdout)."""
    full = ["/usr/bin/time", "-f", "TIME %e %M", "timeout", str(TIMEOUT), *cmd]
    p = subprocess.run(full, capture_output=True, text=True, errors="replace")
    secs, mib = None, None
    for line in p.stderr.splitlines():
        if line.startswith("TIME "):
            _, e, m = line.split()
            secs, mib = float(e), int(m) / 1024
    with open(log, "w") as f:
        f.write(p.stdout + "\n--- stderr ---\n" + p.stderr)
    return p.returncode, secs, mib, p.stdout


def main():
    corpus, out, hayro, pdfdiff = sys.argv[1:5]
    rows = []
    for line in open(os.path.join(corpus, "index.tsv")):
        name, kind, dpi = line.rstrip("\n").split("\t")
        pdf = os.path.join(corpus, name)
        stem = name.removesuffix(".pdf")
        ours, theirs = os.path.join(out, "hayro", stem), os.path.join(out, "poppler", stem)
        for d in (ours, theirs):
            shutil.rmtree(d, ignore_errors=True)
            os.makedirs(d)
        code, h_s, h_mib, stdout = timed([hayro, pdf, ours, dpi, str(MAX_PAGES)], ours + ".log")
        status = "exit %d" % code
        page_ms = []
        for l in stdout.splitlines():
            if l.startswith("status "):
                status = l[len("status "):]
            elif l.startswith("page "):
                page_ms.append(float(l.split()[2]))
        if code == 124:
            status = "timeout"
        elif code != 0 and status == "ok":
            status = "exit %d" % code
        cmd = ["pdftoppm", "-r", dpi, "-l", str(MAX_PAGES), "-png", pdf, os.path.join(theirs, "p")]
        p_code, p_s, p_mib, _ = timed(cmd, theirs + ".log")
        p_status = {0: "ok", 124: "timeout"}.get(p_code, "exit %d" % p_code)
        diff = subprocess.run([pdfdiff, ours, theirs], capture_output=True, text=True).stdout
        with open(os.path.join(out, stem + ".diff"), "w") as f:
            f.write(diff)
        summary = [l for l in diff.splitlines() if l.startswith("summary ")]
        both, only_h, only_p, mismatch, mae, pct, worst = summary[0].split()[1:] if summary else ["0"] * 7
        rows.append({
            "pdf": name, "kind": kind,
            "hayro": status, "h_pages": len(page_ms),
            "h_s": h_s, "h_mib": h_mib,
            "ms_page": sum(page_ms) / len(page_ms) if page_ms else None,
            "poppler": p_status, "p_pages": int(both) + int(only_p),
            "p_s": p_s, "p_mib": p_mib,
            "compared": int(both), "mismatch": int(mismatch),
            "mae": float(mae), "pct": float(pct), "worst": float(worst),
        })
        print(name, rows[-1], flush=True)

    fmt = lambda v, f="%.2f": "–" if v is None else f % v
    keys = list(rows[0].keys())
    with open(os.path.join(out, "results.tsv"), "w") as f:
        f.write("\t".join(keys) + "\n")
        for r in rows:
            f.write("\t".join(str(r[k]) for k in keys) + "\n")
    md = [
        "| PDF | kind | hayro | pages | hayro s | ms/page | hayro MiB | pdftoppm | pages | pdftoppm s | pdftoppm MiB | compared | size diff | mean % px differing | worst page % | MAE |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        md.append("| " + " | ".join([
            r["pdf"], r["kind"], r["hayro"], str(r["h_pages"]), fmt(r["h_s"]), fmt(r["ms_page"], "%.0f"),
            fmt(r["h_mib"], "%.0f"), r["poppler"], str(r["p_pages"]), fmt(r["p_s"]), fmt(r["p_mib"], "%.0f"),
            str(r["compared"]), str(r["mismatch"]),
            fmt(r["pct"]) if r["compared"] else "–", fmt(r["worst"]) if r["compared"] else "–",
            fmt(r["mae"]) if r["compared"] else "–",
        ]) + " |")
    ok = [r for r in rows if r["compared"]]
    tot_h = sum(r["h_s"] or 0 for r in ok)
    tot_p = sum(r["p_s"] or 0 for r in ok)
    md.append("")
    md.append(f"Rendered by both and compared: {len(ok)} of {len(rows)} PDFs, "
              f"{sum(r['compared'] for r in ok)} pages. Wall time over those: hayro {tot_h:.1f} s, "
              f"pdftoppm {tot_p:.1f} s. Peak memory, the most of any PDF: hayro "
              f"{max((r['h_mib'] or 0) for r in rows):.0f} MiB, pdftoppm {max((r['p_mib'] or 0) for r in rows):.0f} MiB.")
    text = "\n".join(md) + "\n"
    open(os.path.join(out, "results.md"), "w").write(text)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        open(os.environ["GITHUB_STEP_SUMMARY"], "a").write("## hayro 0.9 vs pdftoppm\n\n" + text)
    print(text)


if __name__ == "__main__":
    main()
