needed-by: v0.3 (ADR 0007 section 5), built 2026-11-28 (lane A-core, #1699; the manager reviews)
decision: `amdvi` is written clean-room from AMD's IOMMU specification (doc 48882 rev 3.10), not ported from FreeBSD's `sys/x86/iommu/amd_*.c`
---
**Why.** ADR 0007 section 5 and its reuse table say `amdvi` is ported from FreeBSD's BSD-2-Clause AMD-Vi code. Whether copied files are covered by the licence rules waits on the founder's ADR 0009 question 4, so v0.3's groundwork copies nothing: not from FreeBSD, not from Linux. Every format (the device table entry, the host I/O page table, the commands and the event log) is written from the specification, and the crate's module docs give each layout as a table.

**Checks.** The layouts were cross-checked against 48882 from knowledge in #1699's review (round 116), as the PDF could not be fetched. #1702 checks the command and event-log formats under QEMU's `amd-iommu`: a COMPLETION_WAIT's store, an ILLEGAL_COMMAND_ERROR from a bad command, invalidations that complete without an event, and the Status bits. The device table entry's and the page table's real check comes with DMA translation, after spike S07-2.

**Opt-in until S07-2** (the manager's ruling on #1702). By default the kernel reads the IVRS and logs `iommu: N unit(s) in the IVRS`, and touches no IOMMU register: on the founder's PC a wrong device table bit with IommuEn on could block disk or network DMA, or interrupts, and the layouts have not been checked against the PDF. `iommu=on` runs the bring-up (pass-through entries with IV clear, the command buffer, IommuEn and CmdBufEn, one COMPLETION_WAIT, the unit turned off again on a failure); the `iommu` boot case sets it. This default is reconsidered with S07-2, before ADR 0007's `iommu=prefer` default lands.

**Unchanged:** the ADR's text, which still says "ported"; it is corrected if the founder's answer to ADR 0009 question 4 makes the port the plan again.
