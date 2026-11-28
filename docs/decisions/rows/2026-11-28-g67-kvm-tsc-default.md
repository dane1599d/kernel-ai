needed-by: P6 G67, built 2026-11-28 (K, change 1 of G67, with figures from lane A-core's measuring branch; the manager reviews)
decision: Under KVM the TSC is the default clock even when it is neither invariant nor reported, kept only if it settles and passes the drift check; the HPET everywhere else
---
**Why.** GitHub's Intel Xeon 6975P-C runners (4 vCPUs, KVM) show neither CPUID bit, so the 2026-11-04 rule kept the HPET there: `clock: the HPET (KVM, but no invariant or reported TSC rate)`. On `dane/g67-net-throughput`, five 1 GiB downloads over TAP each: the default (HPET) gave 24.2 MB/s at 81,094 CPU ms per GiB, with about 1.9 M HPET reads per GiB at about 12 µs each (run 45987653730); `clock=tsc` gave 65.5 MB/s at 28,549 ms per GiB with no HPET reads (run 45987665847). With #1623's fewer kicks as well, 589.6 MB/s at 2,570 ms per GiB (run 46067850711).

**The rule** (`cpufeat::default_clock`). With no `clock=` word, KVM always tries the TSC. The boot log gives the reason: invariant, the rate reported, or neither (`kept only on the drift check`).

**Kept only if it holds.** The two-window calibration against the HPET must settle, and then two windows of 100 ms each must agree within 100 ppm; otherwise the HPET stays the clock. Before #1628's review the check compared the 25 ms rate that settled with one 100 ms window, so the 25 ms window's HPET jitter (about 40 ppm) counted against it, and 19 of 345 KVM boots fell back to the HPET on a steady TSC. The two 100 ms windows add 100 ms to a boot that tries the TSC.

**The risk** is a host whose TSC changes rate after those windows, which the check does not see. KVM gives a guest a fixed-rate TSC, scaled where the host's differs, so that would be a host fault, and `clock=hpet` keeps the HPET.

**Unchanged:** `clock=tsc`, `clock=hpet`, `clock=icount`, TCG, bare metal and other hypervisors. `cargo xtask vbox` boots with `clock=tsc`.

**Follow-up:** re-baseline `desk-perf` and the throughput rows once main has run on the TSC on these runners.
