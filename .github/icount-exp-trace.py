# #796 experiment v3, throwaway: per boot, which QEMU threads emitted which
# trace events. The vCPU thread is the one with the most apic_* events; every
# event from another thread is a host-timed source that could reach the guest.
import collections
import glob
import re
import sys

LINE = re.compile(r"^(\d+)@(\d+\.\d+):(\w+)")

for path in sorted(glob.glob(sys.argv[1] + "/trace-*.log")):
    events = []
    with open(path, errors="replace") as f:
        for line in f:
            m = LINE.match(line)
            if m:
                events.append((int(m.group(1)), float(m.group(2)), m.group(3)))
    if not events:
        print(f"{path}: no events")
        continue
    apic = collections.Counter(t for t, _, e in events if e.startswith("apic_"))
    total = collections.Counter(t for t, _, _ in events)
    vcpu = (apic or total).most_common(1)[0][0]
    start, end = events[0][1], events[-1][1]
    # The kernel's first HPET access marks its start (the firmware uses none).
    hpet = [ts for t, ts, e in events if t == vcpu and e.startswith("hpet_")]
    kstart = hpet[0] if hpet else None
    other = collections.defaultdict(lambda: [0, 0, None, None])
    for t, ts, e in events:
        if t == vcpu:
            continue
        row = other[(t, e)]
        row[0] += 1
        if kstart is not None and ts >= kstart:
            row[1] += 1
        row[2] = ts if row[2] is None else row[2]
        row[3] = ts
    ks = f"{kstart - start:.3f}" if kstart is not None else "?"
    print(f"{path}: {len(events)} events, {end - start:.3f} s, vcpu tid {vcpu}, kernel at +{ks} s")
    for (t, e), (n, after, first, last) in sorted(other.items(), key=lambda kv: -kv[1][0]):
        print(f"  tid {t} {e}: {n} ({after} after kernel start) "
              f"first +{first - start:.3f} last +{last - start:.3f}")
