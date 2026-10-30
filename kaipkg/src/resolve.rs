//! Resolution without a solver (ADR 0011 §6): for each package wanted, keep an
//! installed version that already satisfies the range, else take the newest
//! compatible version that is not yanked. The `kai:*` WIT packages the OS
//! provides are virtual packages, so `world` is an ordinary dependency. A
//! failure is one sentence in code's words. A real solver goes here when the
//! first real conflict appears (founder question 2).

use crate::Sha;
use crate::generation::Installed;
use crate::header::Depends;
use crate::version::{Range, Version};
use alloc::string::String;
use alloc::vec::Vec;

/// One version of a package in the repository's index (the TUF target's
/// `custom.kai` fields that resolution needs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub version: Version,
    pub sha: Sha,
    pub size: u64,
    pub depends: Vec<Depends>,
    pub grants: Vec<String>,
    /// The OS rows its effects may call, as its index entry lists them.
    pub effects: Vec<String>,
    pub yanked: bool,
}

/// A package the resolution picked: from the index, or kept as installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub name: String,
    pub version: Version,
    pub sha: Sha,
    pub explicit: bool,
    /// Present only when the pick comes from the index and is not yet stored.
    pub entry: Option<Entry>,
    pub grants: Vec<String>,
}

/// What the user wants: each name, with a range or any version.
pub struct Want<'a> {
    pub name: &'a str,
    pub range: Option<Range>,
}

fn any() -> Range {
    Range {
        low: Version::new(0, 0, 0),
        high: None,
    }
}

/// A name to resolve: its range, whether it is wanted itself, and which picked
/// package needs it.
type Queued = (String, Range, bool, Option<(String, Version)>);

/// The closure of `wants` over `index`, `installed` kept where it satisfies.
/// `provided` are the OS's `kai:*` packages and their versions. Every want is
/// explicit; the rest are dependencies. The order is the order they were reached.
pub fn resolve(
    index: &[Entry],
    provided: &[(&str, Version)],
    installed: &[Installed],
    wants: &[Want<'_>],
) -> Result<Vec<Pick>, String> {
    let mut picks: Vec<Pick> = Vec::new();
    let mut queue: Vec<Queued> = wants
        .iter()
        .map(|w| (w.name.into(), w.range.unwrap_or_else(any), true, None))
        .collect();
    let mut i = 0;
    while i < queue.len() {
        let (name, range, explicit, by) = queue[i].clone();
        i += 1;
        let needs = |what: &str| match &by {
            Some((who, v)) => alloc::format!("Cannot install {who} {v}: it needs {what}"),
            None => alloc::format!("Cannot install {what}"),
        };
        if let Some(world) = name.strip_prefix("kai:") {
            let ours = provided.iter().find(|(p, _)| *p == name);
            match ours {
                Some((_, v)) if range.matches(v) => continue,
                Some((_, v)) => {
                    let wanted = range.low;
                    return Err(alloc::format!(
                        "{}; this OS has {}.{}.",
                        needs(&alloc::format!(
                            "kai:{world}@{}.{}",
                            wanted.major,
                            wanted.minor
                        )),
                        v.major,
                        v.minor
                    ));
                }
                None => return Err(alloc::format!("{}; this OS has none.", needs(&name))),
            }
        }
        if let Some(pick) = picks.iter_mut().find(|p| p.name == name) {
            if !range.matches(&pick.version) {
                return Err(alloc::format!(
                    "{}, but {} {} is chosen.",
                    needs(&alloc::format!("{name} in another range")),
                    pick.name,
                    pick.version
                ));
            }
            pick.explicit |= explicit;
            continue;
        }
        let kept = installed
            .iter()
            .find(|p| p.name == name && range.matches(&p.version));
        let (pick, depends) = match kept {
            Some(p) => {
                // An installed package's dependencies are already installed.
                let depends = index
                    .iter()
                    .find(|e| e.name == name && e.version == p.version)
                    .map(|e| e.depends.clone())
                    .unwrap_or_default();
                let pick = Pick {
                    name: p.name.clone(),
                    version: p.version,
                    sha: p.sha,
                    explicit,
                    entry: None,
                    grants: p.grants.clone(),
                };
                (pick, depends)
            }
            None => {
                let newest = index
                    .iter()
                    .filter(|e| e.name == name && !e.yanked && range.matches(&e.version))
                    .max_by_key(|e| e.version);
                let Some(e) = newest else {
                    let known = index.iter().any(|e| e.name == name);
                    return Err(match known {
                        true => {
                            alloc::format!("{}: no version that is not yanked fits.", needs(&name))
                        }
                        false => alloc::format!("{}: no package by that name.", needs(&name)),
                    });
                };
                let pick = Pick {
                    name: e.name.clone(),
                    version: e.version,
                    sha: e.sha,
                    explicit,
                    entry: Some(e.clone()),
                    grants: e.grants.clone(),
                };
                (pick, e.depends.clone())
            }
        };
        for d in depends {
            queue.push((
                d.name,
                d.range,
                false,
                Some((pick.name.clone(), pick.version)),
            ));
        }
        picks.push(pick);
    }
    Ok(picks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn entry(name: &str, version: &str, depends: &[(&str, &str)], yanked: bool) -> Entry {
        Entry {
            name: name.into(),
            version: Version::parse(version).unwrap(),
            sha: crate::sha256(alloc::format!("{name} {version}").as_bytes()),
            size: 1,
            depends: depends
                .iter()
                .map(|(n, r)| Depends {
                    name: (*n).into(),
                    range: Range::parse(r).unwrap(),
                })
                .collect(),
            grants: vec![],
            effects: Vec::new(),
            yanked,
        }
    }

    fn index() -> Vec<Entry> {
        vec![
            entry(
                "notes",
                "2.1.0",
                &[("spell-en", "^1.2"), ("kai:app", "^0.1")],
                false,
            ),
            entry("notes", "3.0.0", &[("kai:app", "^0.2")], false),
            entry("spell-en", "1.2.3", &[], false),
            entry("spell-en", "1.4.0", &[], false),
            entry("spell-en", "1.5.0", &[], true),
        ]
    }

    const OS: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];

    fn want<'a>(name: &'a str, range: Option<&str>) -> Want<'a> {
        Want {
            name,
            range: range.map(|r| Range::parse(r).unwrap()),
        }
    }

    #[test]
    fn the_newest_compatible_unyanked_version_is_taken() {
        let picks = resolve(&index(), OS, &[], &[want("notes", Some("^2"))]).unwrap();
        let got: Vec<(&str, Version, bool)> = picks
            .iter()
            .map(|p| (p.name.as_str(), p.version, p.explicit))
            .collect();
        assert_eq!(
            got,
            vec![
                ("notes", Version::new(2, 1, 0), true),
                ("spell-en", Version::new(1, 4, 0), false)
            ]
        );
    }

    #[test]
    fn an_installed_version_that_satisfies_is_kept() {
        let installed = vec![Installed {
            name: "spell-en".into(),
            version: Version::new(1, 2, 3),
            sha: crate::sha256(b"spell-en 1.2.3"),
            explicit: false,
            grants: vec![],
        }];
        let picks = resolve(&index(), OS, &installed, &[want("notes", Some("^2"))]).unwrap();
        assert_eq!(picks[1].version, Version::new(1, 2, 3));
        assert_eq!(picks[1].entry, None);
    }

    #[test]
    fn failures_are_one_sentence() {
        assert_eq!(
            resolve(&index(), OS, &[], &[want("notes", None)]),
            Err("Cannot install notes 3.0.0: it needs kai:app@0.2; this OS has 0.1.".into())
        );
        assert_eq!(
            resolve(&index(), OS, &[], &[want("weather", None)]),
            Err("Cannot install weather: no package by that name.".into())
        );
        assert_eq!(
            resolve(&index(), OS, &[], &[want("spell-en", Some("^1.5"))]),
            Err("Cannot install spell-en: no version that is not yanked fits.".into())
        );
    }
}
