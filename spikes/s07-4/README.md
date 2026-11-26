# S07-4: a ring-3 NIC data path (spike/netd-ring3, never merged)

ADR 0007's gate for `netd`. One 1 GiB TCP download to a ring-3 client, five
boots each under KVM on a GitHub runner (`.github/workflows/netd-spike.yml`):

- **netd-kernel**: today's path. The kernel's virtio-net driver and smoltcp,
  and `netbench` reading a bulk stream with NET_RECV (8 KiB a call).
- **netd-ring3**: `netd` (user/src/bin/netd/), the same driver
  (`nic.rs` is kernel/src/net/virtio_net.rs, copied) and smoltcp in ring 3,
  with the card's BAR mapped uncached and a DMA arena below 4 GiB, both kernel
  `Shm` objects. It waits for the card's MSI-X vector with NETD_WAIT and
  serves the body to `netbench` over a KRNG v1 ring: 16 requests of 64 KiB out
  at a time, futex doorbells on the ring's heads.

The kernel's `netd-spike` case (kernel/src/netd_spike.rs) counts the CPU of
every CPU but its idle thread from the client's start to its exit; netbench
times the body from its first byte. `cargo xtask netd-blob` serves the bytes
over a TAP device at 10.0.2.2. Bars: throughput at least 0.8x the kernel
path's, CPU per GiB at most 1.5x. The runs are in the workflow's summary.

Throwaway kernel changes, all on this branch only: an uncached kernel `Shm`
object (shm.rs, memory/user.rs), the card's virtio structures found at probe
(virtio/pci.rs), `netd=ring3` leaving the kernel's stack down
(virtio/mod.rs), call 240 NETD_WAIT, and `net::wake` and
`sched_control::measure` made public.
