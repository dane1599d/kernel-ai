needed-by: P5, built 2026-11-09 (lane N-t, P5 task 6's boot case; the manager gave it to N-t for the P5 close, under the founder's "Yes, start")
decision: `net-download` runs one 64 MiB download across two boots, with two cuts and a reboot inside a chunk. TCG uses the same 32 chunks at 64 KiB. The job list is pinned in `netcore::fixture`. The partial sits beside its target
---
**What it shows.** This is the roadmap's P5 exit, "a cut connection resumes, and a bad certificate or bad hash is refused", in a boot case.
- `dlcheck` runs `kainet::download` over `user::tls` into kaifs through `fs`.
- **Boot 1:** a target of 32 chunks is cut by the mock at 40% and resumed. The resume fetches only chunk 12 again. The download then stops halfway through chunk 16, as a reboot inside it would.
- **Boot 2,** on the same disk:
  - it resumes at the mark;
  - it re-checks chunks 0-15 from kaifs without fetching them;
  - the mock cuts it at 80% and it resumes, fetching only chunk 25 again;
  - the whole target's SHA-256 checks out before the rename.
- **Boot 1 also refuses:**
  - a flipped byte, by its chunk (2), writing none of that chunk;
  - a list whose whole-file hash is wrong, which is never moved in;
  - an expired certificate, one for another name and one from an unknown CA, each before a byte is stored.

**One download over both boots.** The plan's section 1 lists the cuts and the reboot as separate checks. Here they share one target, so the case moves 64 MiB once rather than three times, and `kai-data` holds one target.

**TCG.** Under TCG the target is the same 32 chunks at 64 KiB, 2 MiB in all. The cuts at 40% and 80%, the reboot inside chunk 16, and every expected line are the same as under KVM. xtask adds `accel=tcg` or `accel=kvm` to every HTTPS case's command line, and the kernel picks the job list from it. The PR has both wall times.

**Trust.** `dlcheck` gets a job list's URL and its SHA-256 (`jobs=`, `pin=`), pinned in `netcore::fixture`, as signed metadata pins a target. The job list names each `kai-chunks 1` list by its SHA-256. The targets are made by xtask at test time from fixed seeds, and none is committed. An xtask test fails with the new pins if what the mock serves changes.

**Where the bytes go.** A target grows as `downloads/<name>.partial`, beside `<name>.mark`. Each chunk and its mark are one kaifs action, so the mark never names a chunk that is not on disk. The target is renamed to `<name>` at the end. ADR 0011 §5 says `.partial/`, but `fs` renames only within one folder today, so the partial sits beside its target. `pkg` and `updated` keep their own layouts.

**The harness.**
- `mock_https::Fault::CutsAt(&[percent])` cuts a file once at each point, so one fault set before the boot gives several cuts.
- The expired-certificate fault (`Fault::Expired`) was already there.
- `TestCase::data_mib` gives a case a larger GPT `kai-data`. This case uses 192 MiB; every other case keeps 64 MiB.
- The second boot uses `then`: a boot on the same data disk with its own mock and faults. `update-online` can resume across a reboot the same way.
