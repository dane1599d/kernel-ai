needed-by: P8, built 2026-11-27 (row G42; S-io)
decision: The installer is a no_std `installer` crate that plans an install as ordered steps, the primary GPT header written last
---
P8 row 2 needs an installer that writes ADR 0008 §1's layout with GUIDs of its own. The host half is the no_std `installer` crate. `installer::plan` takes the target's size and sector size, the running disk's table and an install id, and returns ordered steps. `installer::apply` writes those steps from the running disk to the target. The device step will call `apply` over its two block devices. The layout is xtask's release layout, with kai-data taking the rest of the disk in whole kaifs blocks. It is built from the same constants (`installer::PARTITION_MIB`) and the same GUID derivation (`installer::unique`, which xtask's `gpt.rs` now re-exports). So an install with id X lays out exactly as `release-image --image-id X`, GUIDs included. kai-data is formatted by `installer::format_data`, which `cargo xtask fs mkfs` and the release image now call too: kaifs's three volumes and P2 decision 5's folders.

**Order.** First, both old GPT headers and arrays and the protective MBR are zeroed, then flushed. Next come the contents, then a flush:
- the ESP, kai-r and kai-a copied, each as many sectors as its FAT boot sector gives;
- kai-b's first MiB zeroed;
- kai-data's block 0 zeroed, so no old `kai-modelhint 1` names a model the disk lacks;
- kai-data formatted.

Then the MBR and both entry arrays are written, then flushed. Last come the backup header and the primary header, each flushed. Until the backup header lands, no GPT on the disk is valid, so no firmware finds a partition on it. Once it lands, every partition is whole. Before any write, every volume to copy is checked, so a running disk that cannot be copied leaves the target as it was.

**Limits.**
- 512-byte sectors only, like `diskmap`. A 4Kn target is refused.
- kai-data needs at least 128 kaifs blocks (512 KiB). kaifs formats 64 blocks, but the folders need 90. That is a floor for the format, not a floor for a usable install: room for a model is a product question.
- The installer does not decide whether the target may be wiped, and does not check that it is not the running disk. Both are the device step's job, as is the boot record a previous install left in NVRAM.
- Copies are not read back before the GPT is written.
