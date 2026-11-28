needed-by: v0.3 (P7) HD Audio, built 2026-11-28 (lane M, the manager's hand-out; the manager reviews)
decision: HD Audio is brought up in ring 0 first, on the kernel's devkit host and MSI, before audiod exists; the tone plays only under test=hda or hda=tone
---
**Why ring 0 now.** ADR 0007 puts HD Audio in ring 3 under `audiod`, and `audiod` waits on S07-1's teardown result. The `hda` crate is written against `devkit::Host`, as the ADR's section 2 asks, so it can be proved on QEMU's `intel-hda` now. Moving it to `audiod` later swaps only its host (`KernelHost` for `UserHost`), not the crate. The kernel module, `kernel/src/hda.rs`, is the bring-up's host and nothing more: it opens the output path, queues a test tone, and answers the self-test.

**Interrupts.** MSI, else polled; never INTx (ADR 0007 section 4). `pci::msix` gains the MSI capability, and the device record says `MSI`. The handler only acknowledges the stream's buffer completion (SDnSTS, write 1 to clear) and counts it.

**Silent by default.** The class row (04/03) matches a real PC's onboard audio and a GPU's HDMI audio too. On every boot the kernel resets each controller, finds the first codec's output path and logs it, but leaves it silent. A tone at every boot would sound from the founder's speakers, so it plays only under `test=hda` or `hda=tone`. An HDMI codec has only digital pins, so it logs `NoOutputPath` and is left alone.

**What it does not do yet.** No mixing, no format other than 48 kHz 16-bit stereo, no input, no jack sensing (unsolicited responses are passed over), no power-down on shutdown, and only one stream. All of that is `audiod`'s.
