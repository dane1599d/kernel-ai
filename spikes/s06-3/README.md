# Spike S06-3: our hypervisor (ADR 0006)

Throwaway, on `spike/our-hypervisor` only, never merged. About 2,000 lines:
`kernel/src/vm/` (one vCPU on AMD SVM with nested paging; COM1, PIT, PIC and
CMOS by port I/O; an MP table; no ACPI, PCI or virtio) and this directory.
It boots Debian's stock `linux-image-amd64` bzImage with an initramfs made in
CI (`build-guest.sh`, in a `debian:trixie` container). Nothing of Debian's is
committed or shipped: the guest is a 3-day workflow artifact.

- `guest/init`: the initramfs's `/init` (busybox sh).
- `guest/s063probe.c`: markers on port 0x5E0, the port-I/O round-trip bench,
  and timing `dav1d`.
- `.github/workflows/spike-s06-3.yml`: TCG on every runner; KVM (nested SVM)
  and the plain-KVM baseline on the AMD runners.

The kernel logs `s06-3: measure ...` lines: boot to the kernel's "Run /init"
line and to `/init`'s marker, `dav1d`'s decode, and the mean port-I/O round
trip; `s063probe` logs the median. ADR 0006's bars: (a) boot to init at most
5 s under KVM and 180 s under TCG; (b) `dav1d` at least 0.85 of plain KVM's
speed; (c) the median round trip at most 20 us under nested KVM.

Shortcuts a real `vmm` must not take: the guest runs `nolapic noapic`, and the
LAPIC and IOAPIC pages are plain RAM holding the two registers Linux reads; no
instruction emulator (string I/O and MMIO kill the guest); without NRIPS (TCG)
RIP moves by the bare opcode's length; XCR0 stays the host's.
