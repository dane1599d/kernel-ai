needed-by: the founder's first VirtualBox test (2026-11-14, lane K; the manager's bug report)
decision: the runaway check counts at most 50 ms of one stretch between two charges on a CPU, so a host stall or a clock jump is not run time
---
**What happened:** the founder was typing in the Terminal under VirtualBox (8 vCPUs, `hz=250`, the TSC as the clock). The supervisor (2 s limit), `fs` (5 s), `desktop` (10 s) and `terminal-window` (20 s) were all killed as runaways within moments. Two restarted supervisors were killed right after starting. In the same moment `terminal-window` napped as "unused", although it was in use. That needs the clock to have moved on by its 2-minute nap time.

**The cause:** the kernel charges the running thread with the clock's time since the last charge on that CPU, at every tick and every switch. If the host does not run a vCPU, or the clock jumps forward, the next charge on every CPU gets the whole gap. `hpet::nanos` returns the latest time any CPU has seen, so one vCPU's jump becomes every CPU's. The thread running on each CPU then has a burst past its limit, and the next tick kills it. The kill line names the limit, not the burst, so the log could not show the gap. The one late-tick line (465 ms, CPU 1) is only logged for a CPU with threads waiting.

**The rule (`sched::burst::counted_since`, `MAX_STRETCH_NS`):**
- At most 50 ms of one stretch between two charges on a CPU counts as time on it, for the burst and for METRICS' CPU time.
- Ticks come every 1 to 10 ms (`hz=` 100 to 1000), so a real runaway is charged every tick and still dies at its limit. A 30 s stall halfway through a 2 s limit delays the kill by at most 12.5 ticks' worth (host test).
- What is not counted is logged on the tick once it reaches 100 ms: `burst: cpu N: X ms passed between two charges, not counted as run time: the vCPU did not run, the clock jumped, or interrupts were off that long`. The next such report shows the gap's size.
- The cost is kernel code that keeps interrupts off for more than 50 ms: those stretches count only 50 ms each. That is a kernel bug of its own, and the late-tick line still shows it when threads wait.

**The case:** `runaway-stall` (`test=runaway-stall`) moves the clock on 5 s on every CPU at once (`hpet::skip_for_test`) while a runaway with a 2 s limit computes. It must die as a runaway after computing 2 s, less the 50 ms one stretch of the jump may add, not at the jump, and within 5 times its limit.

**Not changed here:** user programs read the same clock, so a jump still ends their timed waits early, and the app nap still counts it as time unused. Whether the founder's jump was a real stall or one vCPU's TSC is for a follow-up.
