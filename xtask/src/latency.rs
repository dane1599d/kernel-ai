//! `cargo xtask latency --local ID [--store DIR] [--accel kvm] [--cpus N]
//! [--timeout SECS]`: spike S02-2's baseline (ADR 0002, P1 plan section 5). Boots
//! `test=latency` with the catalog model on the `kai-model` disk and types
//! [`REQUESTS`] at the terminal, one at each `latency: ready for keys`. The kernel
//! times each by its own clock, Enter to the busy sign and Enter to the reply on
//! screen, and logs `latency:` lines, which this prints again at the end.
//! `tune-model.yml` runs it on a KVM runner (mode `latency`); never on a PC
//! without the founder's OK.

use crate::cases::{TestCase, case};
use crate::disk::ModelDisk;
use crate::tune::boot::{Booter, split};
use std::time::Duration;

/// Plain requests on today's script path: three reads and two policy switches,
/// each of which names the policy, so all run at once.
pub const REQUESTS: [&str; 5] = [
    "how many cpus are there",
    "switch to round robin",
    "how much memory is free",
    "go back to interactive",
    "how long has the system been up",
];

/// Room for the model to load and cache its prompt, and for five requests of up
/// to two rounds each.
const LEAST: Duration = Duration::from_secs(1_800);

const USAGE: &str = "usage: cargo xtask latency --local ID [--store DIR] [--accel kvm] \
                     [--cpus N] [--timeout SECS]";

/// QEMU `sendkey` names that type `text` and press Enter.
fn keys(text: &str) -> Result<&'static [&'static str], String> {
    let mut names = Vec::with_capacity(text.len() + 1);
    for c in text.chars() {
        let name: &'static str = match c {
            ' ' => "spc",
            'a'..='z' | '0'..='9' => String::from(c).leak(),
            other => return Err(format!("no key name for {other:?}")),
        };
        names.push(name);
    }
    names.push("ret");
    Ok(names.leak())
}

pub fn main(args: &[String]) -> Result<(), String> {
    let (own, rest) = split(args, &["--local", "--store"]);
    let value = |name: &str| own.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let id = value("--local").ok_or(USAGE)?;
    let path = crate::models::stored(&id, value("--store").as_deref())?;
    let disk = ModelDisk::open(&path)?;
    println!("xtask: {}", disk.describe());
    let stages: Vec<&'static [&'static str]> =
        REQUESTS.iter().map(|r| keys(r)).collect::<Result<_, _>>()?;
    let latency = TestCase {
        keys: stages.leak(),
        expect: &["latency: done"],
        server: false,
        ..case("latency", "test=latency")
    };
    let booter = Booter::build(&rest)?;
    let cmdline = format!("{} {}", latency.cmdline, disk.cmdline());
    let firmware = booter.prepare(&cmdline)?;
    let monitor = crate::qemu::free_port()?;
    let sizing = crate::qemu::run_sizing(booter.accel, booter.options.sizing, true);
    let mut qemu_args = crate::qemu::args(&firmware, booter.accel, true, Some(&disk), sizing);
    qemu_args.push("-monitor".into());
    qemu_args.push(format!("tcp:127.0.0.1:{monitor},server=on,wait=off"));
    println!(
        "xtask: latency: {} {}",
        booter.qemu.display(),
        qemu_args.join(" ")
    );
    let run = crate::qemu::TestRun {
        case: &latency,
        accel: booter.accel,
        timeout: booter.options.timeout.max(LEAST),
        monitor,
        qmp: None,
        exact: Vec::new(),
        contains: Vec::new(),
    };
    crate::qemu::run_test(&booter.qemu, &qemu_args, booter.target(), &run)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_types_as_keys_and_names_what_it_changes() {
        for request in REQUESTS {
            let typed = keys(request).unwrap();
            assert_eq!(typed.len(), request.len() + 1);
            assert_eq!(typed.last(), Some(&"ret"));
        }
        // A switch the request does not name would ask, and wait for an answer.
        assert!(
            REQUESTS
                .iter()
                .all(|r| !r.contains("switch") || r.contains("round robin"))
        );
        assert!(keys("Upper").is_err());
    }
}
