//! A scripted component for host tests: a counter whose state is CBOR
//! `[schema, count]`. Its behaviour, and its one bug if any, come from a `fake`
//! custom section in its artifact, so tests go through real artifact parsing.

use crate::artifact::{Manifest, append_custom_section, bundle, custom_section};
use crate::txn::{Effect, Inputs, Storage, Txn};
use crate::{Fault, Instance, Reason};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use kabi::live::{MANIFEST_SECTION, SOURCE_SECTION};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bug {
    None,
    /// `kai.teleport` is imported: loading fails to link.
    Link,
    /// `add` traps once the count reaches this.
    TrapAt(u64),
    /// `spin` never ends.
    Spin,
    Health,
    /// Health fails once the count reaches this.
    HealthAt(u64),
    SelfTest,
    /// Import traps.
    ImportTrap,
    /// Its export claims the wrong schema.
    Export,
    /// Writing an older schema traps.
    ExportOlderTrap,
    /// `add` replies err.
    Regress,
    /// The candidate (the version that is not quiet) traps on `kadd` while
    /// [`INJECT`] is set.
    Injected,
}

/// Makes every [`Bug::Injected`] candidate trap on its next `kadd`.
pub static INJECT: AtomicBool = AtomicBool::new(false);

/// One version's behaviour.
#[derive(Clone, Debug)]
pub struct Spec {
    pub label: &'static str,
    pub schema: u32,
    /// Schemas it can import.
    pub reads: &'static [u32],
    /// Older schemas it can export.
    pub writes: &'static [u32],
    pub world: &'static str,
    pub bug: Bug,
}

impl Spec {
    pub const fn new(label: &'static str, schema: u32) -> Self {
        Self {
            label,
            schema,
            reads: &[],
            writes: &[],
            world: "app:counter/counter@1.0.0",
            bug: Bug::None,
        }
    }

    pub fn artifact(&self) -> Vec<u8> {
        let mut module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
        let config = format!(
            "{} {} {} {} {:?} {}",
            self.schema,
            list(self.reads),
            list(self.writes),
            self.world,
            self.bug,
            self.label
        );
        append_custom_section(&mut module, "fake", config.as_bytes());
        let manifest = Manifest {
            lifecycle: 1,
            component: "counter".into(),
            version: self.label.into(),
            world: self.world.into(),
            engine: "wasmi".into(),
            features: "-".into(),
            licence: "MPL-2.0".into(),
            build: "0123456789abcdef".into(),
            source: "fedcba9876543210".into(),
        };
        append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
        let source = bundle(&[("src/lib.rs", b"// a counter\n")]);
        append_custom_section(&mut module, SOURCE_SECTION, &source);
        module
    }
}

fn list(schemas: &[u32]) -> String {
    let parts: Vec<String> = schemas.iter().map(u32::to_string).collect();
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(",")
    }
}

fn parse_list(text: &str) -> Vec<u32> {
    text.split(',')
        .filter_map(|part| part.parse().ok())
        .collect()
}

pub struct Fake {
    label: String,
    schema: u32,
    reads: Vec<u32>,
    writes: Vec<u32>,
    bug: Bug,
    count: u64,
    pub quiet: bool,
    /// The open call's storage and what it did.
    storage: Storage,
    inputs: Inputs,
    txn: Txn,
}

/// What the loader does with a module: a fault for a broken one.
pub fn load(module: &[u8], quiet: bool) -> Result<Fake, Fault> {
    let config = custom_section(module, "fake")
        .ok()
        .flatten()
        .ok_or_else(|| Fault::new(Reason::Compile, "not a fake module"))?;
    let config = core::str::from_utf8(config).unwrap();
    let words: Vec<&str> = config.split(' ').collect();
    let bug = match words[4] {
        "None" => Bug::None,
        "Link" => return Err(Fault::new(Reason::Link, "unknown import kai.teleport")),
        "Spin" => Bug::Spin,
        "Health" => Bug::Health,
        "SelfTest" => Bug::SelfTest,
        "ImportTrap" => Bug::ImportTrap,
        "Export" => Bug::Export,
        "ExportOlderTrap" => Bug::ExportOlderTrap,
        "Regress" => Bug::Regress,
        "Injected" => Bug::Injected,
        other if other.starts_with("HealthAt(") => Bug::HealthAt(number_in(other)),
        other => Bug::TrapAt(number_in(other)),
    };
    Ok(Fake {
        label: words[5].to_string(),
        storage: Storage::new(),
        inputs: Inputs::default(),
        txn: Txn::default(),
        schema: words[0].parse().unwrap(),
        reads: parse_list(words[1]),
        writes: parse_list(words[2]),
        bug,
        count: 0,
        quiet,
    })
}

/// The number in `Name(n)`.
fn number_in(text: &str) -> u64 {
    let start = text.find('(').unwrap() + 1;
    text[start..text.len() - 1].parse().unwrap()
}

fn state(schema: u32, count: u64) -> Vec<u8> {
    // [schema, count]: a count below 24 in its head byte, else in two bytes.
    match u16::try_from(count) {
        Ok(small) if small < 24 => Vec::from([0x82, schema as u8, small as u8]),
        Ok(count) => {
            let [high, low] = count.to_be_bytes();
            Vec::from([0x82, schema as u8, 0x19, high, low])
        }
        Err(_) => panic!("the fake counts below 65,536"),
    }
}

fn trap(what: &str) -> Fault {
    Fault::new(Reason::Trap, format!("unreachable executed in {what}"))
}

impl Instance for Fake {
    fn schema(&mut self) -> Result<u32, Fault> {
        Ok(self.schema)
    }

    fn init(&mut self) -> Result<u32, Fault> {
        self.count = 0;
        Ok(0)
    }

    fn import(&mut self, bytes: &[u8], from_schema: u32) -> Result<u32, Fault> {
        if self.bug == Bug::ImportTrap {
            return Err(trap("import"));
        }
        if from_schema != self.schema && !self.reads.contains(&from_schema) {
            return Ok(1);
        }
        match bytes {
            [0x82, schema, count] if u32::from(*schema) == from_schema && *count < 24 => {
                self.count = u64::from(*count);
                Ok(0)
            }
            [0x82, schema, 0x19, high, low] if u32::from(*schema) == from_schema => {
                self.count = u64::from(u16::from_be_bytes([*high, *low]));
                Ok(0)
            }
            _ => Ok(2),
        }
    }

    fn export(&mut self, to_schema: u32) -> Result<Option<Vec<u8>>, Fault> {
        let schema = if to_schema == 0 {
            self.schema
        } else {
            to_schema
        };
        if schema != self.schema && !self.writes.contains(&schema) {
            return Ok(None);
        }
        if schema != self.schema && self.bug == Bug::ExportOlderTrap {
            return Err(trap("export"));
        }
        let claimed = if self.bug == Bug::Export {
            schema + 10
        } else {
            schema
        };
        Ok(Some(state(claimed, self.count)))
    }

    fn handle(&mut self, request: &[u8]) -> Result<Vec<u8>, Fault> {
        let reply = match request {
            b"add" => {
                if let Bug::TrapAt(at) = self.bug
                    && self.count + 1 >= at
                {
                    return Err(trap("add"));
                }
                if self.bug == Bug::Regress {
                    return Ok(b"err no".to_vec());
                }
                self.count += 1;
                format!("ok {}", self.count)
            }
            b"get" => format!("ok {} schema {}", self.count, self.schema),
            // A kai-call stand-in: counts, stores who wrote, and queues an effect.
            b"kadd" => {
                if self.bug == Bug::Injected && !self.quiet && INJECT.load(Ordering::SeqCst) {
                    return Err(trap("kadd"));
                }
                self.count += 1;
                let label = self.label.clone();
                self.txn
                    .writes
                    .insert("by".into(), Some(label.clone().into_bytes()));
                self.txn
                    .writes
                    .insert("count".into(), Some(self.count.to_string().into_bytes()));
                self.txn
                    .effects
                    .push(Effect::Notify(format!("{label} {}", self.count)));
                format!("kai-reply 1\nresult \"{label} {}\"\n", self.count)
            }
            // What the call sees of the storage and its inputs.
            b"kget" => {
                let stored = |key: &str| {
                    self.storage.get(key).map_or(String::from("-"), |v| {
                        String::from_utf8_lossy(v).into_owned()
                    })
                };
                format!(
                    "kai-reply 1\nresult \"{} by {} seed {}\"\n",
                    stored("count"),
                    stored("by"),
                    self.inputs.seed
                )
            }
            b"kspam" => {
                for n in 0..9 {
                    self.txn.effects.push(Effect::Notify(format!("{n}")));
                }
                String::from("kai-reply 1\nresult \"spam\"\n")
            }
            b"spin" if self.bug == Bug::Spin => {
                return Err(Fault::new(Reason::Fuel, "all fuel consumed"));
            }
            b"poison" => return Err(trap("poison")),
            b"spin" => String::from("ok spun"),
            _ => String::from("err unknown"),
        };
        Ok(reply.into_bytes())
    }

    fn health(&mut self) -> Result<u32, Fault> {
        let failing = match self.bug {
            Bug::Health => true,
            Bug::HealthAt(at) => self.count >= at,
            _ => false,
        };
        Ok(u32::from(failing))
    }

    fn self_test(&mut self) -> Result<u32, Fault> {
        Ok(u32::from(self.bug == Bug::SelfTest))
    }

    fn quiet(&mut self, quiet: bool) {
        self.quiet = quiet;
    }

    fn begin(&mut self, storage: &Storage, inputs: Inputs) {
        self.storage = storage.clone();
        self.inputs = inputs;
        self.txn = Txn::default();
    }

    fn finish(&mut self) -> Txn {
        core::mem::take(&mut self.txn)
    }
}
