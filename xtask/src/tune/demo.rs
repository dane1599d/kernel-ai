//! `cargo xtask tune demo --local ID [--store DIR] [--accel kvm] [--cpus N]
//! [--timeout SECS]`: the founder's first view of the model tuning Kernel AI
//! itself. Boots `test=tune-demo` with mix A and the catalog model on the
//! `kai-model` disk, types `tune editor` at the terminal, `yes` once the
//! proposal card waits for it (only then), and has QEMU save a screendump
//! (`target/tune-demo.ppm`) after the verdict. Pipe the output to a file, then
//! `cargo xtask tune check --log FILE --local ID` requires every answer the guest
//! gave to be the host engine's. `tune-model.yml` runs both on a KVM runner.

use super::boot::{Booter, split};
use crate::cases::{TestCase, case};
use crate::disk::ModelDisk;
use std::time::Duration;

#[rustfmt::skip]
const TYPE_TUNE: &[&str] = &[
    "t", "u", "n", "e", "spc", "e", "d", "i", "t", "o", "r", "ret",
];
const YES: &[&str] = &["y", "e", "s", "ret"];
const SHOT: &[&str] = &["!screendump tune-demo.ppm"];

const DEMO: TestCase = TestCase {
    keys: &[TYPE_TUNE, YES, SHOT],
    expect: &["tune-demo: done"],
    server: false,
    ..case("tune-demo", "test=tune-demo workload=A seed=1")
};

/// The model must prefill and answer under the shell's limits; the boot has
/// room for that, the observe window and the trial.
const LEAST: Duration = Duration::from_secs(1_200);

const USAGE: &str = "usage: cargo xtask tune demo --local ID [--store DIR] [--accel kvm] \
                     [--cpus N] [--timeout SECS]";

pub fn main(args: &[String]) -> Result<(), String> {
    let (own, rest) = split(args, &["--local", "--store"]);
    let value = |name: &str| own.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let id = value("--local").ok_or(USAGE)?;
    let path = crate::models::stored(&id, value("--store").as_deref())?;
    let disk = ModelDisk::open(&path)?;
    println!("xtask: {}", disk.describe());
    let booter = Booter::build(&rest)?;
    let cmdline = format!("{} {}", DEMO.cmdline, disk.cmdline());
    let firmware = booter.prepare(&cmdline)?;
    let monitor = crate::qemu::free_port()?;
    let sizing = crate::qemu::run_sizing(booter.accel, booter.options.sizing, true);
    let mut qemu_args = crate::qemu::args(&firmware, booter.accel, true, Some(&disk), sizing);
    qemu_args.push("-monitor".into());
    qemu_args.push(format!("tcp:127.0.0.1:{monitor},server=on,wait=off"));
    println!(
        "xtask: tune demo: {} {}",
        booter.qemu.display(),
        qemu_args.join(" ")
    );
    let run = crate::qemu::TestRun {
        case: &DEMO,
        accel: booter.accel,
        timeout: booter.options.timeout.max(LEAST),
        monitor,
        exact: Vec::new(),
        contains: Vec::new(),
    };
    crate::qemu::run_test(&booter.qemu, &qemu_args, booter.target(), &run)?;
    println!(
        "xtask: the demo ran; screendump at {}; check the answers with: \
         cargo xtask tune check --log <this output> --local {id}",
        booter.target().join("tune-demo.ppm").display()
    );
    Ok(())
}
