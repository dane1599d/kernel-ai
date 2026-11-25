#!/usr/bin/env python3
"""A throughput floor for a boot case's log (throughput.yml).

    throughput_floor.py LOG CASE FLOOR REGEX

LOG is a `cargo xtask test` console log, or several joined by commas: the
same case booted more than once, whose median is floored, so one slow boot
on a noisy runner does not fail it while a slow driver still does.

REGEX is matched against each line of each LOG.
Each named group in it is one direction (`write`, `read`, `down`) and holds a
figure in MB/s. Every figure from the line that matches last goes to the job
summary ($GITHUB_STEP_SUMMARY, when it is set) as a table row. The script fails
when a figure is under FLOOR, or when no line matches at all, so a case that
stops printing its figures fails rather than passing quietly.
"""

import os
import re
import statistics
import sys


def figures(log_path: str, pattern: re.Pattern[str]) -> dict[str, float] | None:
    """The named groups of the last line matching `pattern`, as floats."""
    found = None
    with open(log_path, encoding="utf-8", errors="replace") as log:
        for line in log:
            match = pattern.search(line)
            if match:
                found = match
    if found is None:
        return None
    return {name: float(value) for name, value in found.groupdict().items()}


def summary(case: str, floor: float, measured: dict[str, float]) -> str:
    """The job summary's rows: one for each direction, with its verdict."""
    rows = [
        f"### {case}: throughput floor {floor:g} MB/s",
        "",
        "| direction | MB/s | floor | |",
        "|---|---:|---:|---|",
    ]
    for name, mbs in measured.items():
        verdict = "ok" if mbs >= floor else "**under the floor**"
        rows.append(f"| {name} | {mbs:g} | {floor:g} | {verdict} |")
    return "\n".join(rows) + "\n"


def write_summary(text: str) -> None:
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as out:
            out.write(text + "\n")
    print(text)


def main(argv: list[str]) -> int:
    if len(argv) != 5:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    log_path, case, floor_text, regex = argv[1:]
    try:
        floor = float(floor_text)
        pattern = re.compile(regex)
    except (ValueError, re.error) as error:
        print(f"throughput_floor: bad floor or pattern: {error}", file=sys.stderr)
        return 2
    if not pattern.groupindex:
        print("throughput_floor: the pattern names no direction", file=sys.stderr)
        return 2
    runs = [figures(path, pattern) for path in log_path.split(",")]
    if any(run is None for run in runs):
        write_summary(f"### {case}: a log has no figures; the floor cannot pass\n")
        return 1
    if len(runs) > 1:
        boots = "; ".join(
            ", ".join(f"{name} {mbs:g}" for name, mbs in run.items()) for run in runs
        )
        write_summary(f"{case}: {len(runs)} boots: {boots}; the median is floored\n")
    measured = {
        name: statistics.median(run[name] for run in runs) for name in runs[0]
    }
    write_summary(summary(case, floor, measured))
    under = [name for name, mbs in measured.items() if mbs < floor]
    if under:
        print(f"{case}: {', '.join(under)} under {floor:g} MB/s", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
