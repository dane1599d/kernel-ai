//! `test=blk`: the disk with serial `kai-model` reads back exactly the file xtask
//! attached. xtask passes the file's length and FNV-1a hash as `model_len=` and
//! `model_fnv=`; the guest hashes every byte through the driver, including the
//! partial last sector, and the zero padding QEMU adds after it must read as zero.
//! The disk must be read-only, and every read must reach the staging buffer in
//! place, not through a bounce copy.

use crate::virtio::{blk, hal};
use crate::{hpet, log};
use kabi::model::{Attached, Fnv1a, SERIAL};

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let attached = Attached::from_cmdline(cmdline);
    let (Some(len), Some(expected)) = (attached.len, attached.fnv) else {
        return Err("blk: the command line has no model_len= and model_fnv=");
    };
    let started = hpet::nanos();
    let shared_before = hal::in_place_shares();
    let mut reads = 0;
    let (sectors, read_only, hash) = blk::with_disk(SERIAL, |disk| {
        let mut hash = Fnv1a::new();
        disk.stream(len, |_, bytes| {
            reads += 1;
            hash.update(bytes);
            Ok(())
        })
        .map(|()| (disk.sectors(), disk.read_only(), hash.finish()))
    })
    .ok_or("blk: no disk with serial kai-model")??;
    let in_place = hal::in_place_shares() - shared_before;
    log!(
        "blk: read {len} bytes of {sectors} sectors in {} ms, fnv1a64 {hash:016x}",
        (hpet::nanos() - started) / 1_000_000
    );
    // QEMU rounds a raw file up to whole sectors; the test file is not a whole
    // number of them, so this also proves the last sector was partial.
    if sectors != mm::dma::sectors_for(len) || len.is_multiple_of(mm::dma::SECTOR) {
        return Err("blk: the disk is not the file rounded up to a partial last sector");
    }
    if !read_only {
        return Err("blk: the kai-model disk is writable");
    }
    log!("blk: {reads} reads, {in_place} buffers shared in place");
    if in_place < reads {
        return Err("blk: a read went through a bounce buffer");
    }
    if hash != expected {
        log!("blk: the host's fnv1a64 is {expected:016x}");
        return Err("blk: the guest's hash differs from the host's");
    }
    log!("blk: kai-model reads back byte for byte, partial last sector included");
    Ok(())
}
