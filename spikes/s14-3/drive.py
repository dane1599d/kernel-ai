#!/usr/bin/env python3
"""Spike S14-3 js-engines: the driver around the two runners.

  drive.py manifest <test262> <out.tsv> --shard K --of N
  drive.py run <out-results.tsv> <manifest.tsv> -- <runner command...>
  drive.py summarize <out.md> <results.tsv>...
  drive.py octane <octane-dir> <out.tsv> <label> -- <runner command...>
  drive.py start <out.tsv> <label> -- <runner command...>

`run` feeds the runner its manifest from a start line, and restarts it past
a test that hangs (no line for TIMEOUT s) or traps, which counts as failed.
"""

import os
import re
import statistics
import subprocess
import sys
import tempfile
import threading
import queue
import time


DIRS = ["test/built-ins", "test/language", "test/annexB"]
TIMEOUT = float(os.environ.get("S14_TIMEOUT", "20"))


def frontmatter(text):
    import yaml  # PyYAML, only the manifest needs it

    m = re.search(r"/\*---(.*?)---\*/", text, re.S)
    if not m:
        return {}
    try:
        return yaml.safe_load(m.group(1)) or {}
    except yaml.YAMLError:
        return {}


def manifest(root, out, shard, of):
    tests = []
    for d in DIRS:
        for dirpath, _, files in os.walk(os.path.join(root, d)):
            for f in files:
                if f.endswith(".js") and "_FIXTURE" not in f:
                    tests.append(os.path.relpath(os.path.join(dirpath, f), root))
    tests.sort()
    lines = []
    for i, path in enumerate(tests):
        if i % of != shard:
            continue
        with open(os.path.join(root, path), encoding="utf-8") as fh:
            meta = frontmatter(fh.read())
        flags = meta.get("flags") or []
        neg = (meta.get("negative") or {}).get("type") or "-"
        includes = ",".join(meta.get("includes") or []) or "-"
        is_async = "1" if "async" in flags else "0"
        if "module" in flags:
            modes = ["module"]
        elif "raw" in flags:
            modes = ["raw"]
        elif "onlyStrict" in flags:
            modes = ["strict"]
        elif "noStrict" in flags:
            modes = ["sloppy"]
        else:
            modes = ["sloppy", "strict"]
        for mode in modes:
            lines.append("\t".join([path, mode, is_async, neg, includes]))
    with open(out, "w") as fh:
        fh.write("\n".join(lines) + "\n")
    print(f"{len(lines)} runs of {sum(1 for i in range(len(tests)) if i % of == shard)} tests")


def run(out, manifest_path, cmd):
    with open(manifest_path) as fh:
        lines = fh.read().splitlines()
    verdict = {}
    start = 0
    restarts = 0
    began = time.time()
    while start < len(lines):
        err = tempfile.TemporaryFile(mode="w+")
        proc = subprocess.Popen(
            cmd + [str(start)],
            stdout=subprocess.PIPE,
            stderr=err,
            text=True,
            errors="replace",
        )
        said = False
        q = queue.Queue()

        def pump():
            for line in proc.stdout:
                q.put(line.rstrip("\n"))
            q.put(None)

        threading.Thread(target=pump, daemon=True).start()
        current = None
        ended = False
        while True:
            try:
                line = q.get(timeout=TIMEOUT)
            except queue.Empty:
                proc.kill()
                if current is not None:
                    verdict[current] = "fail\ttimed out"
                break
            if line is None:
                proc.wait()
                if not said:
                    err.seek(0)
                    sys.exit(f"the runner printed nothing (exit {proc.returncode}): {err.read()[-2000:]}")
                if current is not None and current not in verdict:
                    verdict[current] = f"fail\ttrapped (exit {proc.returncode})"
                ended = current is None or current >= len(lines) - 1
                break
            said = True
            if line.startswith("S "):
                current = int(line[2:])
            elif line.startswith("R "):
                _, i, rest = line.split(" ", 2)
                verdict[int(i)] = rest.replace(" ", "\t", 1)
                current = None
            elif line.startswith("D "):
                current = None
        proc.wait()
        done = [i for i in verdict if i >= start]
        nxt = max(done) + 1 if done else start + 1
        if ended or nxt >= len(lines):
            break
        start = nxt
        restarts += 1
    with open(out, "w") as fh:
        for i, line in enumerate(lines):
            path, mode = line.split("\t")[:2]
            v = verdict.get(i, "fail\tnot run")
            fh.write(f"{path}\t{mode}\t{v}\n")
    passed = sum(1 for v in verdict.values() if v.startswith("pass"))
    print(f"{passed}/{len(lines)} runs pass, {restarts} restarts, {time.time() - began:.0f} s")


def summarize(out, results):
    per = {}
    reasons = {}
    for r in results:
        with open(r) as fh:
            for line in fh:
                path, mode, verdict, *why = line.rstrip("\n").split("\t")
                ok = verdict == "pass"
                per[path] = per.get(path, True) and ok
                if not ok and why:
                    key = re.sub(r"[0-9]+", "N", why[0])[:60]
                    reasons[key] = reasons.get(key, 0) + 1
    by = {}
    for path, ok in per.items():
        top = "/".join(path.split("/")[:2])
        p, t = by.get(top, (0, 0))
        by[top] = (p + ok, t + 1)
    total_p = sum(per.values())
    with open(out, "w") as fh:
        fh.write(f"total {total_p}/{len(per)} = {100 * total_p / max(len(per), 1):.2f}%\n")
        for top in sorted(by):
            p, t = by[top]
            fh.write(f"{top} {p}/{t} = {100 * p / t:.2f}%\n")
        fh.write("\ntop failure reasons:\n")
        for key, n in sorted(reasons.items(), key=lambda kv: -kv[1])[:15]:
            fh.write(f"{n}\t{key}\n")
    print(open(out).read())


OCTANE = ["richards", "deltablue", "crypto", "raytrace"]
RUN_JS = """
function PrintResult(name, result) { print(name + ': ' + result); }
function PrintScore(score) { print('Score: ' + score); }
BenchmarkSuite.RunSuites({ NotifyResult: PrintResult, NotifyError: PrintResult, NotifyScore: PrintScore });
"""


def timed(cmd):
    """Runs `cmd` under /usr/bin/time: (stdout, wall s, peak RSS KiB)."""
    began = time.time()
    p = subprocess.run(["/usr/bin/time", "-f", "RSS %M"] + cmd, capture_output=True, text=True)
    wall = time.time() - began
    rss = re.search(r"RSS (\d+)", p.stderr)
    if not p.stdout.strip():
        sys.exit(f"{cmd[0]} printed nothing (exit {p.returncode}): {p.stderr[-2000:]}")
    return p.stdout, wall, int(rss.group(1)) if rss else 0


def octane(octane_dir, out, label, cmd):
    run_js = os.path.join(octane_dir, "s14-run.js")
    with open(run_js, "w") as fh:
        fh.write(RUN_JS)
    rows = []
    for name in OCTANE:
        scores, mems, rsss = [], [], []
        for _ in range(3):
            stdout, _, rss = timed(cmd + [os.path.join(octane_dir, "base.js"), os.path.join(octane_dir, f"{name}.js"), run_js])
            m = re.search(r"^Score: ([\d.]+)", stdout, re.M)
            scores.append(float(m.group(1)) if m else 0.0)
            g = re.search(r"mem (\d+)", stdout)
            mems.append(int(g.group(1)) if g else 0)
            rsss.append(rss)
            if not m:
                print(f"{label} {name}: no score: {stdout[-400:]}")
        rows.append(f"{label}\t{name}\t{statistics.median(scores):.0f}\t{max(mems) // 1024}\t{max(rsss)}")
        print(rows[-1])
    with open(out, "a") as fh:
        fh.write("\n".join(rows) + "\n")


def start(out, label, cmd):
    walls, inits, rsss, mems = [], [], [], []
    for _ in range(20):
        stdout, wall, rss = timed(cmd + ["empty"])
        walls.append(wall * 1000)
        m = re.search(r"init_us (\d+) eval_us (\d+) mem (\d+)", stdout)
        if m:
            inits.append(int(m.group(1)) / 1000)
            mems.append(int(m.group(3)))
        rsss.append(rss)
    row = (
        f"{label}\t{statistics.median(walls):.1f}\t"
        f"{statistics.median(inits) if inits else 0:.2f}\t"
        f"{max(mems) // 1024 if mems else 0}\t{statistics.median(rsss):.0f}"
    )
    print(row)
    with open(out, "a") as fh:
        fh.write(row + "\n")


def main():
    args = sys.argv[1:]
    if "--" in args:
        cut = args.index("--")
        args, cmd = args[:cut], args[cut + 1 :]
    else:
        cmd = []
    what = args[0]
    if what == "manifest":
        manifest(args[1], args[2], int(args[4]), int(args[6]))
    elif what == "run":
        run(args[1], args[2], cmd)
    elif what == "summarize":
        summarize(args[1], args[2:])
    elif what == "octane":
        octane(args[1], args[2], args[3], cmd)
    elif what == "start":
        start(args[1], args[2], cmd)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
