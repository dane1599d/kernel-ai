//! `cargo xtask kv-preempt --local ID [--store DIR] [--accel kvm] [--cpus N]
//! [--mem MIB] [--timeout SECS]`: spike S13-1's guest half (ADR 0013 section 2).
//! Boots `test=kv-preempt` with the catalog model on the `kai-model` disk and a
//! fresh `kai-cache`: the kernel waits for llm to cache its prompts and asks it to
//! snapshot a KV slot into RAM, restore it, and spill it to `kai-cache` and back
//! at 512, 1,024 and 1,950 positions, each timed by the guest's clock; the
//! `kv-preempt:` lines are the result. It types nothing. `tune-model.yml` runs it
//! on a KVM runner (mode `kv-preempt`); never on a PC without the founder's OK.

use crate::cases::{TestCase, case};
use crate::disk::ModelDisk;
use crate::tune::boot::{Booter, split};
use std::time::Duration;

/// Room for the model to load, both prompts to be cached, and three prefills of up to
/// 2,000 tokens at the runners' 26 tokens/s, with the follow-ups.
const LEAST: Duration = Duration::from_secs(5_400);

/// Guest memory: the model, the engine's two KV slots and a snapshot of a full slot
/// need more than the model disk's own figure.
const MEMORY_MIB: u64 = 8_192;

const USAGE: &str = "usage: cargo xtask kv-preempt --local ID [--store DIR] [--accel kvm] \
                     [--cpus N] [--mem MIB] [--timeout SECS]";

pub fn main(args: &[String]) -> Result<(), String> {
    let (own, rest) = split(args, &["--local", "--store"]);
    let value = |name: &str| own.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let id = value("--local").ok_or(USAGE)?;
    let path = crate::models::stored(&id, value("--store").as_deref())?;
    let disk = ModelDisk::open(&path)?.with_sha256(crate::models::sha256(&id)?);
    println!("xtask: {}", disk.describe());
    let bench = TestCase {
        expect: &["kv-preempt: done"],
        server: false,
        ..case("kv-preempt", "test=kv-preempt")
    };
    let booter = Booter::build(&rest)?;
    let cmdline = format!("{} {}", bench.cmdline, disk.cmdline());
    let firmware = booter.prepare(&cmdline)?;
    let monitor = crate::qemu::free_port()?;
    let mut sizing = crate::qemu::run_sizing(booter.accel, booter.options.sizing, true);
    sizing.mem_mib = Some(
        sizing
            .mem_mib
            .map_or(MEMORY_MIB, |asked| asked.max(MEMORY_MIB)),
    );
    let mut qemu_args = crate::qemu::args(&firmware, booter.accel, true, Some(&disk), sizing);
    // A fresh GPT data disk with an empty kai-cache: the spill files go there.
    let data = crate::blklog::Data {
        nvme: false,
        log: None,
        gpt: true,
    };
    crate::blklog::prepare(booter.target(), data)?;
    qemu_args.extend(crate::blklog::qemu_args(data));
    qemu_args.push("-monitor".into());
    qemu_args.push(format!("tcp:127.0.0.1:{monitor},server=on,wait=off"));
    println!(
        "xtask: kv-preempt: {} {}",
        booter.qemu.display(),
        qemu_args.join(" ")
    );
    let run = crate::qemu::TestRun {
        case: &bench,
        accel: booter.accel,
        timeout: booter.options.timeout.max(LEAST),
        monitor,
        qmp: None,
        exact: Vec::new(),
        contains: Vec::new(),
        cut: None,
    };
    crate::qemu::run_test(&booter.qemu, &qemu_args, booter.target(), &run)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A model of 2.6 GB needs about 4 GiB by the disk's own figure; the case asks for
    // more, and never asks for less than the caller did.
    const _: () = assert!(MEMORY_MIB >= 8_192);
    const _: () = assert!(LEAST.as_secs() >= 3_600);

    #[test]
    fn the_usage_names_every_flag() {
        for flag in [
            "--local",
            "--store",
            "--accel",
            "--cpus",
            "--mem",
            "--timeout",
        ] {
            assert!(USAGE.contains(flag), "{flag}");
        }
    }
}
