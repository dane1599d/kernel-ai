"""Spike S07-4's figures (spike/netd-ring3 only): each boot of netd-kernel and
netd-ring3 in DIR, their medians, and the ratios against the bars."""

import re
import statistics
import sys
from pathlib import Path

RATE = re.compile(r"netbench: (\d+) bytes in (\d+) ms = ([0-9.]+) MB/s")
CPU = re.compile(r"netd-spike: mode=(\w+) bytes=(\d+) wall_ms=(\d+) busy_ms=(\d+) cpus=(\d+) cpu_ms_per_gib=(\d+) interrupts=(\d+)")


def main(folder):
    rows = {"kernel": [], "ring3": []}
    print("### Runs\n")
    print("| path | run | MB/s | wall ms | busy CPU ms | CPUs | CPU ms per GiB | interrupts |")
    print("|---|---|---|---|---|---|---|---|")
    for path in rows:
        for log in sorted(Path(folder).glob(f"netd-{path}-*.log")):
            run = log.stem.rsplit("-", 1)[1]
            text = log.read_text(errors="replace")
            rate, cpu = RATE.search(text), CPU.search(text)
            if not (rate and cpu):
                print(f"| {path} | {run} | failed | | | | | |")
                continue
            mbs, per_gib = float(rate.group(3)), int(cpu.group(6))
            rows[path].append((mbs, per_gib))
            print(f"| {path} | {run} | {mbs} | {cpu.group(3)} | {cpu.group(4)} | {cpu.group(5)} | {per_gib} | {cpu.group(7)} |")
    print()
    if not rows["kernel"] or not rows["ring3"]:
        print("Not enough runs for the medians.")
        return 1
    med = {p: (statistics.median(r[0] for r in v), statistics.median(r[1] for r in v)) for p, v in rows.items()}
    speed = med["ring3"][0] / med["kernel"][0]
    cpu = med["ring3"][1] / med["kernel"][1]
    print("### Medians\n")
    print("| path | runs | MB/s | CPU ms per GiB |")
    print("|---|---|---|---|")
    for p, (m, c) in med.items():
        print(f"| {p} | {len(rows[p])} | {m:.1f} | {c:.0f} |")
    print()
    print(f"Throughput ring3/kernel: **{speed:.2f}x** (bar: at least 0.8x): {'pass' if speed >= 0.8 else 'fail'}")
    print()
    print(f"CPU per GiB ring3/kernel: **{cpu:.2f}x** (bar: at most 1.5x): {'pass' if cpu <= 1.5 else 'fail'}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
