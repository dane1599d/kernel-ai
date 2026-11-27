# IVRS fixtures

QEMU 11.0.5's own IVRS tables, as `-device amd-iommu` builds them, for
`acpitables/tests/ivrs.rs` (plan P6 G61). They were captured on a GitHub
runner (ubuntu-24.04) by the throwaway branch `spike/ivrs-capture` at
c50ff665, run 45985251317. That branch was never for merging. Each variant
booted the runner's own Linux kernel with a busybox initramfs, which printed
`/sys/firmware/acpi/tables/IVRS` to the serial port as base64:

```
qemu-system-x86_64 -display none -no-reboot -m 512 \
  -kernel vmlinuz -initrd initrd.cpio \
  -append "console=ttyS0 rdinit=/init amd_iommu=off iommu=off panic=-1 quiet" \
  -serial file:<variant>.log -net none <variant's flags>
```

| File | Flags | SHA-256 |
|---|---|---|
| `q35.bin` | `-machine q35 -device amd-iommu` | `8dbfb491…3d698` |
| `q35-devices.bin` | `-machine q35 -device amd-iommu -device pcie-root-port,id=rp1,bus=pcie.0,chassis=1 -device virtio-net-pci,bus=rp1,netdev=n0 -netdev user,id=n0 -device qemu-xhci -device pci-bridge,id=br1,bus=pcie.0,chassis_nr=2 -device virtio-rng-pci,bus=br1` | `5be9de82…d2afb` |
| `q35-xtsup.bin` | `-machine q35,kernel-irqchip=split -device amd-iommu,intremap=on,xtsup=on` | `655dc16d…b3ba8` |

Each table has one IOMMU at 00:02.0, base 0xfed80000, described twice: once
by an IVHD 10h block and once by an 11h block. Its device entries are Selects
for each function, an I/O APIC (48h) as 00:14.0, and, in `q35-devices`, a range for the
root port's bus and an alias range for the PCI bridge's bus. `q35-xtsup`
differs from `q35` only in the 10h block's feature field and the 11h block's
EFR image, which gains XTSup (bit 2).

The capture also ran `-machine q35,kernel-irqchip=split -device
amd-iommu,intremap=on`. Its table came out byte for byte the same as `q35.bin`
(QEMU's IVRS does not depend on intremap), so it is not kept.

To capture again, push to a branch carrying that workflow; QEMU here is never
run locally.
