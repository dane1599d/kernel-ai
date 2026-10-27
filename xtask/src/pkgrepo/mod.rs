//! `cargo xtask pkgrepo build [DIR]`: the built-in apps as signed packages (P4
//! task 1's host half; ADR 0011 §1-§4).
//!
//! Each app in [`APPS`] is packed from its version on the boot drive into a
//! `kai-pkg 1` file: the `kai-package 1` header, then the ADR 0003 artifact as
//! `app.wasm`. The packer refuses an app whose parts disagree (ADR 0011 §2):
//! the artifact's component and licence against the header's, its action names
//! against the `<name>_` prefix and the OS's rows, and the header's `world`
//! against the `kai:app` version the artifact's WIT world includes. The
//! packages then go into a TUF repository signed by a fresh test root
//! ([`tuf`]), written to DIR (default `target/pkgrepo`).

mod tuf;

#[cfg(test)]
mod tests;

pub use tuf::{Keys, Repository};

use crate::components;
use crate::esp::Module;
use kaipkg::bundle;
use kaipkg::generation::Time;
use kaipkg::header::Header;
use livecore::artifact;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io};
use wit_parser::WorldItem;

/// The artifact's path inside an `app` package, its second file.
pub const ARTIFACT: &str = "app.wasm";

/// A built-in app: the version on the boot drive it is packed from, and the
/// header fields the artifact does not carry.
pub struct App {
    /// The package name, which must be the artifact's component.
    pub name: &'static str,
    /// The boot drive's version, `comp/<name>/<label>.wasm`.
    pub label: &'static str,
    /// At most 80 bytes; data, never instructions.
    pub summary: &'static str,
    /// Must be the artifact's own licence.
    pub licence: &'static str,
    /// The app's WIT package directory.
    pub wit: &'static str,
}

/// Every built-in app. P4's apps join as they land (P4 plan, tasks 4-11).
pub const APPS: &[App] = &[App {
    name: "notes",
    label: livecore::boot::BUILT_IN,
    summary: "Plain-text notes the AI can read and write",
    licence: "MPL-2.0",
    wit: components::NOTES_RECIPE.wit,
}];

/// One packed package and the index fields its artifact gives.
pub struct Package {
    pub header: Header,
    /// The header as written, the package's first file.
    pub header_text: String,
    /// The artifact's action names, in its rows' order.
    pub actions: Vec<String>,
    /// The `kai-grants 1` names its `kai-app 1` text asks for. No artifact has
    /// a `kai.app` section yet (ADR 0001), so this is empty.
    pub grants: Vec<String>,
    pub bytes: Vec<u8>,
}

impl Package {
    /// Its TUF target path, `<name>-<version>.kpkg`.
    pub fn target(&self) -> String {
        format!("{}-{}.kpkg", self.header.name, self.header.version)
    }

    /// Its `custom.kai` index fields (ADR 0011 §3): the header's, then the
    /// grants and actions, then `yanked`. `depends` keeps the header's text.
    pub fn index(&self) -> Value {
        let depends: Vec<&str> = self
            .header_text
            .lines()
            .filter_map(|line| line.strip_prefix("depends "))
            .collect();
        let mut index = json!({
            "name": self.header.name,
            "version": self.header.version.to_string(),
            "kind": self.header.kind.name(),
            "summary": self.header.summary,
            "licence": self.header.licence,
            "depends": depends,
            "grants": self.grants,
            "actions": self.actions,
            "yanked": false,
        });
        if let (Some(world), Value::Object(map)) = (&self.header.world, &mut index) {
            map.insert("world".into(), world.as_str().into());
        }
        index
    }
}

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let dir = match args {
        [build] if build == "build" => root.join("target").join("pkgrepo"),
        [build, dir] if build == "build" => PathBuf::from(dir),
        _ => return Err("usage: cargo xtask pkgrepo build [DIR]".into()),
    };
    let versions = components::build(&root)?;
    let packages = pack_all(&root, &versions)?;
    let keys = Keys::generate()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    let repository = tuf::build(&packages, &keys, Time(now.as_secs()))?;
    write(&repository, &dir)?;
    for package in &packages {
        println!(
            "xtask: {} {} KiB, {} actions",
            package.target(),
            package.bytes.len().div_ceil(1024),
            package.actions.len()
        );
    }
    println!(
        "xtask: a repository signed by test root {} in {}",
        &keys.root.id()[..12],
        dir.display()
    );
    Ok(())
}

/// Every app of [`APPS`], packed from `versions`, the boot drive's.
pub fn pack_all(root: &Path, versions: &[Module]) -> Result<Vec<Package>, String> {
    let mut actions = BTreeSet::new();
    let mut packages = Vec::new();
    for app in APPS {
        let path = format!("comp/{}/{}.wasm", app.name, app.label);
        let module = versions
            .iter()
            .find(|m| m.path == path)
            .ok_or_else(|| format!("{path} was not built"))?;
        let package = pack(root, app, &module.bytes)?;
        if let Some(clash) = package
            .actions
            .iter()
            .find(|a| !actions.insert(a.as_str().to_owned()))
        {
            return Err(format!("{}: action {clash} is another package's", app.name));
        }
        packages.push(package);
    }
    Ok(packages)
}

/// `app`'s package around `artifact`, or why its parts disagree.
pub fn pack(root: &Path, app: &App, artifact: &[u8]) -> Result<Package, String> {
    let fail = |why: String| format!("{}: {why}", app.name);
    let parsed = artifact::parse(artifact).map_err(fail)?;
    let manifest = &parsed.manifest;
    if manifest.component != app.name {
        return Err(fail(format!(
            "the artifact is component {}",
            manifest.component
        )));
    }
    if manifest.licence != app.licence {
        return Err(fail(format!(
            "the artifact's licence is {}, not {}",
            manifest.licence, app.licence
        )));
    }
    let version = manifest
        .world
        .strip_prefix(&format!("app:{}/", app.name))
        .and_then(|rest| rest.split_once('@'))
        .map(|(_, version)| version)
        .ok_or_else(|| {
            fail(format!(
                "the artifact's world {} is not app:{}/",
                manifest.world, app.name
            ))
        })?;
    let world = kai_app_world(root, app, &manifest.world).map_err(fail)?;
    let rows = parsed
        .actions
        .ok_or_else(|| fail("the artifact has no kai.actions".into()))?;
    let rows = livecore::actions::parse(rows).map_err(fail)?;
    if rows.world != manifest.world {
        return Err(fail(format!("its actions are for {}", rows.world)));
    }
    let prefix = format!("{}_", app.name.replace('-', "_"));
    let mut actions = Vec::new();
    for row in &rows.rows {
        let function = row
            .name
            .strip_prefix(&prefix)
            .ok_or_else(|| fail(format!("action {} does not start {prefix}", row.name)))?;
        if agentabi::takes_os_name(app.name, function) {
            return Err(fail(format!("action {} is an OS row's name", row.name)));
        }
        actions.push(row.name.clone());
    }
    // The world is a dependency on the OS's virtual package (ADR 0011 §6).
    let (package, minor) = world.split_once('@').ok_or("a world has no version")?;
    let header_text = format!(
        "kai-package 1\nname {}\nversion {version}\nkind app\nsummary {}\nlicence {}\n\
         world {world}\ndepends {package} ^{minor}\n",
        app.name, app.summary, app.licence
    );
    let header = Header::parse(&header_text).map_err(fail)?;
    let bytes = bundle::build(&[
        (bundle::HEADER, header_text.as_bytes()),
        (ARTIFACT, artifact),
    ]);
    bundle::files(&bytes).map_err(|why| fail(why.into()))?;
    Ok(Package {
        header,
        header_text,
        actions,
        grants: Vec::new(),
        bytes,
    })
}

/// `kai:app@<major>.<minor>`, the one `kai:app` version `world` (such as
/// `app:notes/notes@1.0.0`) imports or exports interfaces of.
fn kai_app_world(root: &Path, app: &App, world: &str) -> Result<String, String> {
    let mut dirs = kai_wit(root)?;
    dirs.push(root.join(app.wit));
    let (resolve, packages) = crate::wit::load_dirs(&dirs)?;
    let (package, name) = world
        .split_once('/')
        .and_then(|(ns_name, rest)| {
            let (name, version) = rest.split_once('@')?;
            Some((format!("{ns_name}@{version}"), name))
        })
        .ok_or_else(|| format!("{world} is not <namespace>:<package>/<world>@<version>"))?;
    let world_id = packages
        .get(&package)
        .and_then(|id| resolve.packages[*id].worlds.get(name))
        .ok_or_else(|| format!("no WIT world {world}"))?;
    let items = &resolve.worlds[*world_id];
    let versions: BTreeSet<String> = items
        .imports
        .values()
        .chain(items.exports.values())
        .filter_map(|item| match item {
            WorldItem::Interface { id, .. } => resolve.interfaces[*id].package,
            _ => None,
        })
        .map(|id| &resolve.packages[id].name)
        .filter(|n| n.namespace == "kai" && n.name == "app")
        .filter_map(|n| n.version.as_ref())
        .map(|v| format!("kai:app@{}.{}", v.major, v.minor))
        .collect();
    let mut versions = versions.into_iter();
    match (versions.next(), versions.next()) {
        (Some(one), None) => Ok(one),
        (None, _) => Err(format!("{world} includes no kai:app world")),
        (Some(_), Some(_)) => Err(format!("{world} includes two kai:app versions")),
    }
}

/// The in-tree `kai:*` WIT packages, `wit/kai-*`.
fn kai_wit(root: &Path) -> Result<Vec<PathBuf>, String> {
    let wit = root.join("wit");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&wit)
        .map_err(|e| format!("cannot list {}: {e}", wit.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("kai-"))
        })
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// `repository` under `dir`, replacing what an earlier build wrote there.
pub fn write(repository: &Repository, dir: &Path) -> Result<(), String> {
    for sub in ["metadata", "targets"] {
        match fs::remove_dir_all(dir.join(sub)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                return Err(format!("cannot clear {}: {e}", dir.join(sub).display()));
            }
            _ => {}
        }
    }
    for (path, bytes) in &repository.files {
        let path = dir.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot make {}: {e}", parent.display()))?;
        }
        fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}
