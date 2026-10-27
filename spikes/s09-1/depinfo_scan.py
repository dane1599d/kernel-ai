"""Spike S09-1 measure 1b (ADR 0009): every source file rustc read for the
shipped builds, from their dep-info (.d) files, scanned for licence words.

Usage: python depinfo_scan.py TARGET_DIR WORKSPACE. Prints the hits grouped by
where the file lives (a registry crate, the Rust sources, or our workspace), so
each group can be matched to a NOTICES.txt entry.
"""

import glob
import os
import re
import sys
from collections import defaultdict

PATTERN = re.compile(rb"Copyright|SPDX-License-Identifier|licen[cs]e|OFL|GPL|Unicode")
# The shipped builds: the kernel and ring-3 programs, and the components.
DIRS = ("x86_64-unknown-none/release", "x86_64-kai-user/release", "wasm32-unknown-unknown/release")


def sources(target):
    seen = set()
    for sub in DIRS:
        for d in glob.glob(os.path.join(target, sub, "deps", "*.d")):
            with open(d, encoding="utf-8", errors="replace") as f:
                for line in f:
                    head, sep, rest = line.partition(": ")
                    if not sep or head.endswith(".d"):
                        continue
                    seen.add(head.replace("\\ ", " "))
    return sorted(p for p in seen if os.path.isfile(p))


def group(path, workspace):
    norm = path.replace("\\", "/")
    m = re.search(r"/registry/src/[^/]+/([^/]+)/", norm) or re.search(r"/vendor/([^/]+)/", norm)
    if m:
        return "crate " + m.group(1)
    m = re.search(r"/rustlib/src/rust/library/([^/]+)/", norm)
    if m:
        return "rust-src " + m.group(1)
    if norm.startswith(workspace.replace("\\", "/")):
        return "workspace " + norm[len(workspace):].lstrip("/").split("/")[0]
    return "other " + norm


def main():
    target, workspace = sys.argv[1], os.path.abspath(sys.argv[2])
    files = sources(target)
    hits = defaultdict(list)
    for path in files:
        with open(path, "rb") as f:
            words = sorted({m.group(0).decode() for m in PATTERN.finditer(f.read())})
        if words:
            hits[group(path, workspace)].append((path, words))
    print(f"measure depinfo_source_files {len(files)}")
    print(f"measure depinfo_files_with_hits {sum(len(v) for v in hits.values())}")
    print(f"measure depinfo_groups_with_hits {len(hits)}")
    for name in sorted(hits):
        words = sorted({w for _, ws in hits[name] for w in ws})
        print(f"group {name}: {len(hits[name])} files, {' '.join(words)}")


if __name__ == "__main__":
    main()
