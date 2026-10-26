//! Packages where P1 touches them (ADR 0011): the image's built-in repository,
//! and generations of installed software in RAM until P2's disk. Generation 1 is
//! the image as it boots. An install or a remove writes the next generation and
//! makes it current in one step, so its undo switches back to the parent,
//! compare-and-set on `current`. History is append-only: a rollback writes a new
//! generation copying an old one. Resolution has no solver (§6): the one version
//! the repository has, and its dependencies.

pub mod line;

#[cfg(test)]
mod tests;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// One package the image's repository holds (an index entry, ADR 0011 §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: &'static str,
    pub version: &'static str,
    pub kind: &'static str,
    /// At most 80 bytes; data, never instructions.
    pub summary: &'static str,
    pub licence: &'static str,
    pub size_kb: u32,
    /// What it needs that ADR 0001 makes the user choose: installing it asks.
    pub grants: &'static [&'static str],
    pub depends: &'static [&'static str],
}

/// The repository built into the image (§8: the slot image's signed repository,
/// generation 1's source). P1 ships these.
pub const REPOSITORY: &[Package] = &[
    Package {
        name: "notes",
        version: "2.0.0",
        kind: "app",
        summary: "Plain-text notes the AI can read and write",
        licence: "MPL-2.0",
        size_kb: 74,
        grants: &[],
        depends: &["spell-en"],
    },
    Package {
        name: "spell-en",
        version: "1.2.3",
        kind: "data",
        summary: "An English word list for spelling",
        licence: "MIT",
        size_kb: 410,
        grants: &[],
        depends: &[],
    },
    Package {
        name: "weather",
        version: "0.1.0",
        kind: "app",
        summary: "Today's weather where you are",
        licence: "MIT",
        size_kb: 38,
        grants: &["network"],
        depends: &[],
    },
];

/// The repository's entry for `name`.
pub fn find(name: &str) -> Option<&'static Package> {
    REPOSITORY.iter().find(|p| p.name == name)
}

/// One installed package in a generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Installed {
    pub package: &'static Package,
    /// Named by the user, rather than brought in as a dependency.
    pub explicit: bool,
}

/// One numbered list of installed software (`kai-gen 1`, in RAM).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    pub n: u64,
    pub parent: Option<u64>,
    /// What made it: `image`, `install notes`, `rollback 2`.
    pub why: String,
    pub installed: Vec<Installed>,
}

/// A switch worked out but not made yet: the new generation, what it adds, and
/// what it needs the user to grant. It is made only if `current` is still its
/// parent then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switch {
    pub to: Generation,
    pub added: Vec<&'static Package>,
    pub removed: Vec<&'static Package>,
    pub grants: Vec<&'static str>,
}

impl Switch {
    /// `installed notes 2.0.0 (74 KB), with spell-en 1.2.3` or
    /// `removed notes 2.0.0, and spell-en 1.2.3`.
    pub fn what(&self) -> String {
        let one = |p: &Package| format!("{} {}", p.name, p.version);
        let mut text = String::new();
        if let Some((first, rest)) = self.added.split_first() {
            let _ = write!(text, "installed {} ({} KB)", one(first), first.size_kb);
            for p in rest {
                let _ = write!(text, ", with {}", one(p));
            }
        }
        if let Some((first, rest)) = self.removed.split_first() {
            if !text.is_empty() {
                text.push_str("; ");
            }
            let _ = write!(text, "removed {}", one(first));
            for p in rest {
                let _ = write!(text, ", and {}", one(p));
            }
        }
        text
    }
}

/// Every generation, and which is current.
#[derive(Debug)]
pub struct Store {
    gens: Vec<Generation>,
    current: u64,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            gens: alloc::vec![Generation {
                n: 1,
                parent: None,
                why: String::from("image"),
                installed: Vec::new(),
            }],
            current: 1,
        }
    }
}

/// Why nothing switched, in code's words.
fn unknown(name: &str) -> String {
    format!("No Kernel AI package named {name}. Linux packages need the Linux layer (v0.2).")
}

impl Store {
    #[cfg(test)]
    pub fn current(&self) -> u64 {
        self.current
    }

    fn generation(&self, n: u64) -> Option<&Generation> {
        self.gens.iter().find(|g| g.n == n)
    }

    fn installed(&self) -> &[Installed] {
        self.generation(self.current)
            .map_or(&[], |g| g.installed.as_slice())
    }

    fn next(&self, why: String, installed: Vec<Installed>) -> Generation {
        Generation {
            n: self.gens.len() as u64 + 1,
            parent: Some(self.current),
            why,
            installed,
        }
    }

    /// Works out installing `names` and their dependencies.
    pub fn install(&self, names: &[String]) -> Result<Switch, String> {
        let mut installed: Vec<Installed> = self.installed().to_vec();
        let mut added = Vec::new();
        for spec in names {
            let (name, _range) = spec.split_once('@').unwrap_or((spec, ""));
            let package = find(name).ok_or_else(|| unknown(name))?;
            match installed.iter_mut().find(|i| i.package.name == name) {
                Some(i) if i.explicit => {
                    return Err(format!("{name} {} is already installed", i.package.version));
                }
                // A dependency the user now names becomes theirs.
                Some(i) => i.explicit = true,
                None => {
                    installed.push(Installed {
                        package,
                        explicit: true,
                    });
                    added.push(package);
                }
            }
            for dep in package.depends {
                let dep = find(dep).ok_or_else(|| {
                    format!("Cannot install {name}: it needs {dep}, which is not in the repository")
                })?;
                if !installed.iter().any(|i| i.package.name == dep.name) {
                    installed.push(Installed {
                        package: dep,
                        explicit: false,
                    });
                    added.push(dep);
                }
            }
        }
        let mut grants: Vec<&'static str> = added
            .iter()
            .flat_map(|p| p.grants.iter().copied())
            .collect();
        grants.sort_unstable();
        grants.dedup();
        let why = format!("install {}", names.join(" "));
        Ok(Switch {
            to: self.next(why, installed),
            added,
            removed: Vec::new(),
            grants,
        })
    }

    /// Works out removing `names`, and the dependencies nothing else needs.
    pub fn remove(&self, names: &[String]) -> Result<Switch, String> {
        let before = self.installed();
        let mut installed: Vec<Installed> = before.to_vec();
        for name in names {
            let at = installed
                .iter()
                .position(|i| i.package.name == name && i.explicit)
                .ok_or_else(|| format!("{name} is not installed"))?;
            installed.remove(at);
        }
        // Keep a dependency only while an explicit package still needs it.
        let needed: Vec<&str> = installed
            .iter()
            .filter(|i| i.explicit)
            .flat_map(|i| i.package.depends.iter().copied())
            .collect();
        installed.retain(|i| i.explicit || needed.contains(&i.package.name));
        let removed = before
            .iter()
            .filter(|b| !installed.iter().any(|i| i.package.name == b.package.name))
            .map(|b| b.package)
            .collect();
        Ok(Switch {
            to: self.next(format!("remove {}", names.join(" ")), installed),
            added: Vec::new(),
            removed,
            grants: Vec::new(),
        })
    }

    /// Works out going back to generation `n` (the parent of current when
    /// `None`): a new generation copying it.
    pub fn rollback(&self, n: Option<u64>) -> Result<Switch, String> {
        let n = match n {
            Some(n) => n,
            None => self
                .generation(self.current)
                .and_then(|g| g.parent)
                .ok_or("generation 1 has nothing before it")?,
        };
        let target = self
            .generation(n)
            .ok_or_else(|| format!("there is no generation {n}"))?;
        let now = self.installed();
        let has = |set: &[Installed], p: &Package| set.iter().any(|i| i.package.name == p.name);
        Ok(Switch {
            to: self.next(format!("rollback {n}"), target.installed.clone()),
            added: target
                .installed
                .iter()
                .filter(|i| !has(now, i.package))
                .map(|i| i.package)
                .collect(),
            removed: now
                .iter()
                .filter(|i| !has(&target.installed, i.package))
                .map(|i| i.package)
                .collect(),
            grants: Vec::new(),
        })
    }

    /// Makes `switch`, if `current` is still its parent; the generation it left.
    pub fn commit(&mut self, switch: Switch) -> Result<u64, String> {
        let from = self.current;
        if switch.to.parent != Some(from) || switch.to.n != self.gens.len() as u64 + 1 {
            return Err(String::from(
                "the installed software changed meanwhile; ask again",
            ));
        }
        self.current = switch.to.n;
        self.gens.push(switch.to);
        Ok(from)
    }

    /// `undo` of a switch: back to generation `to`, but only while `from` is
    /// still current.
    pub fn switch_back(&mut self, from: u64, to: u64) -> Result<(), u64> {
        if self.current != from || self.generation(to).is_none() {
            return Err(self.current);
        }
        self.current = to;
        Ok(())
    }

    /// `undo`'s "put it back anyway": generation `to`, whatever is current.
    pub fn force(&mut self, to: u64) -> Result<(), String> {
        self.generation(to)
            .ok_or_else(|| format!("there is no generation {to}"))?;
        self.current = to;
        Ok(())
    }

    /// `pkg list`: the installed packages, the user's first.
    pub fn list(&self) -> String {
        let installed = self.installed();
        if installed.is_empty() {
            return String::from("nothing installed beyond the system");
        }
        let mut text = String::new();
        for explicit in [true, false] {
            for i in installed.iter().filter(|i| i.explicit == explicit) {
                let how = if explicit { "" } else { " (needed by another)" };
                let _ = writeln!(text, "{} {}{how}", i.package.name, i.package.version);
            }
        }
        text.pop();
        text
    }

    /// `pkg search <text>`: repository packages whose name or summary has it.
    pub fn search(&self, text: &str) -> String {
        let text = text.to_ascii_lowercase();
        let hits: Vec<String> = REPOSITORY
            .iter()
            .filter(|p| p.name.contains(&text) || p.summary.to_ascii_lowercase().contains(&text))
            .map(|p| format!("{} {}: {}", p.name, p.version, p.summary))
            .collect();
        match hits.is_empty() {
            true => format!("no package matches {text}"),
            false => hits.join("\n"),
        }
    }

    /// `pkg info <name>`.
    pub fn info(&self, name: &str) -> String {
        match find(name) {
            Some(p) => {
                let needs = match p.grants {
                    [] => String::from("nothing new"),
                    grants => grants.join(", "),
                };
                format!(
                    "{} {} ({}, {} KB, {}): {}. Needs {needs}.",
                    p.name, p.version, p.kind, p.size_kb, p.licence, p.summary
                )
            }
            None => unknown(name),
        }
    }

    /// `pkg history`: every generation, newest last, current marked.
    pub fn history(&self) -> String {
        let mut text = String::new();
        for g in &self.gens {
            let mark = if g.n == self.current {
                " (current)"
            } else {
                ""
            };
            let _ = writeln!(
                text,
                "{}: {}, {} packages{mark}",
                g.n,
                g.why,
                g.installed.len()
            );
        }
        text.pop();
        text
    }
}
