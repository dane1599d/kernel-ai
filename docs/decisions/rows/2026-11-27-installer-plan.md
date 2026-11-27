needed-by: P8, built 2026-11-27 (row G42; S-io)
decision: The installer is a no_std `installer` crate that plans an install as ordered steps, the primary GPT header written last
---
P8 row 2 needs an installer that writes ADR 0008 §1's layout with GUIDs of its own. The host half is the no_std `installer` crate. `installer::plan` takes four inputs: the target's size and sector size, the target's current table, the running disk's table and committed slot, and an install id. It returns ordered steps, and the table those steps will wipe, which the device shows the user before it asks to go on. `installer::apply` writes those steps from the running disk to the target. The device step will call `apply` over its two block devices. The layout is xtask's release layout, with kai-data taking the rest of the disk in whole kaifs blocks. It is built from the same constants (`installer::PARTITION_MIB`) and the same GUID derivation (`installer::unique`, which xtask's `gpt.rs` now re-exports). So an install with id X lays out exactly as `release-image --image-id X`, GUIDs included. kai-data is formatted by `installer::format_data`, which `cargo xtask fs mkfs` and the release image now call too: kaifs's three volumes and P2 decision 5's folders.

**Order.** First, both old GPT headers and arrays and the protective MBR are zeroed, then flushed. Next come the contents, then a flush:
- the ESP and kai-r copied, and the running system's committed slot (A or B) copied as the install's kai-a, each as many sectors as espfat's checked parse of its FAT32 boot sector gives;
- kai-b's first MiB zeroed;
- kai-data's block 0 zeroed, so no old `kai-modelhint 1` names a model the disk lacks;
- kai-data formatted.

Once those are durable, each copy is read back. The ESP and kai-r must hash as the running disk's. kai-a must hash to the committed slot's SHA-256, which comes from the release's signed metadata, and its `boot/slot` must name the committed `release_seq`. A copy that fails stops the install before any table is written. Then the MBR and both entry arrays are written, then flushed. Last come the backup header and the primary header, each flushed. Until the backup header lands, no GPT on the disk is valid, so no firmware finds a partition on it. Once it lands, every partition is whole. Before any write, the target must still hold the table the plan was made over, and every volume to copy is checked. So a target that changed, or a running disk that cannot be copied, is left as it was. A target whose disk GUID is the running disk's is refused.

**Limits.**
- 512-byte sectors only, like `diskmap`. A 4Kn target is refused.
- kai-data needs at least 128 kaifs blocks (512 KiB). kaifs formats 64 blocks, but the folders need 90. That is a floor for the format, not a floor for a usable install: room for a model is a product question.
- The device step asks the user before wiping what `Plan::wipes` names. The GUID check does not catch every case of the running disk, for example a stick written from the same image, so the device also compares block devices. A boot record a previous install left in NVRAM is the device step's too.
- A slot copied from B to kai-a keeps its boot sector's hidden-sectors field and volume id. espfat ignores both. Whether Limine's FAT driver reads the hidden-sectors field is not checked yet; P8's boot case will show it.
