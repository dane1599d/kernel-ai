"""G67's figures: each net-path boot in DIR (net-path-N.log), MB/s, CPU ms
per GiB and each stage's share of the wall time, and their medians."""

import re
import statistics
import sys
from pathlib import Path

RATE = re.compile(r"net-throughput: (\d+) bytes in (\d+) ms = ([0-9.]+) MB/s")
CPU = re.compile(r"net-throughput: cpu bytes=(\d+) wall_ms=(\d+) busy_ms=(\d+) cpus=(\d+) cpu_ms_per_gib=(\d+) interrupts=(\d+)")
STAGE = re.compile(r"net-throughput: stage (\w+) count=(\d+) ms=(\d+) ns_each=(\d+) share_of_wall_pct=(\d+)")


def main(folder):
    runs = []
    print("### Runs\n")
    print("| run | MB/s | wall ms | busy CPU ms | CPUs | CPU ms per GiB | interrupts |")
    print("|---|---|---|---|---|---|---|")
    stages = {}
    for log in sorted(Path(folder).glob("net-path-*.log")):
        run = log.stem.rsplit("-", 1)[1]
        text = log.read_text(errors="replace")
        rate, cpu = RATE.search(text), CPU.search(text)
        if not (rate and cpu):
            print(f"| {run} | failed | | | | | |")
            continue
        runs.append((float(rate.group(3)), int(cpu.group(5))))
        print(f"| {run} | {rate.group(3)} | {cpu.group(2)} | {cpu.group(3)} | {cpu.group(4)} | {cpu.group(5)} | {cpu.group(6)} |")
        for m in STAGE.finditer(text):
            stages.setdefault(m.group(1), []).append((int(m.group(2)), int(m.group(3)), int(m.group(4)), int(m.group(5))))
    print()
    if not runs:
        print("No run finished.")
        return 1
    print("### Medians\n")
    print(f"- MB/s: **{statistics.median(r[0] for r in runs):.1f}** over {len(runs)} runs")
    print(f"- CPU ms per GiB: **{statistics.median(r[1] for r in runs):.0f}**")
    print()
    print("### Stages (medians over the runs)\n")
    print("| stage | count | ms | ns each | % of wall |")
    print("|---|---|---|---|---|")
    for name, rows in stages.items():
        med = [statistics.median(r[i] for r in rows) for i in range(4)]
        print(f"| {name} | {med[0]:.0f} | {med[1]:.0f} | {med[2]:.0f} | {med[3]:.0f} |")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
