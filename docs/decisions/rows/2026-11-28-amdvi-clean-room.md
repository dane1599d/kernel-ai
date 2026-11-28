needed-by: v0.3 (ADR 0007 section 5), built 2026-11-28 (lane A-core, #1699; the manager reviews)
decision: `amdvi` is written clean-room from AMD's IOMMU specification (doc 48882 rev 3.10), not ported from FreeBSD's `sys/x86/iommu/amd_*.c`
---
**Why.** ADR 0007 section 5 and its reuse table say `amdvi` is ported from FreeBSD's BSD-2-Clause AMD-Vi code. Whether copied files are covered by the licence rules waits on the founder's ADR 0009 question 4, so v0.3's groundwork copies nothing: not from FreeBSD, not from Linux. Every format (the device table entry, the host I/O page table, the commands and the event log) is written from the specification, and the crate's module docs give each layout as a table.

**Checks.** The layouts were cross-checked against 48882 from knowledge in #1699's review (round 116), as the PDF could not be fetched. #1702 checks the command and event-log formats under QEMU's `amd-iommu`: a COMPLETION_WAIT's store, an ILLEGAL_COMMAND_ERROR from a bad command, invalidations that complete without an event, and the Status bits. The device table entry's and the page table's real check comes with DMA translation, after spike S07-2.

**Unchanged:** the ADR's text, which still says "ported"; it is corrected if the founder's answer to ADR 0009 question 4 makes the port the plan again.
