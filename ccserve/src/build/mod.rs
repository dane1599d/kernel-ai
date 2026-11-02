//! `cc build` (ADR 0012 D7): a component's C sources, compiled with its world's
//! glue and [`BUILDRT`], linked standalone by wcc (its own memory, a
//! [`STACK`]-byte stack, no libraries), checked and renamed by [`module`], and
//! written as ADR 0003's artifact: `kai.manifest`, `kai.source`, `kai.actions`,
//! `kai.app` for an app, and `kai.notices` with xcc's MIT notice.
//!
//! Every wcc run is one of [`Compiler`]'s, in its own Store under one budget,
//! so a build stops at the deadline or Ctrl-C like an entry. The build is all
//! of its own input: what it writes depends on the files, name, version and
//! kind alone, and its source hash is [`Built::provenance`]'s.

pub mod module;
pub mod shm;

use crate::bind::{self, World};
use crate::compiler::{CompileError, Compiler};
use crate::{Budget, XCC_HEADERS};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION, MAX_ARTIFACT, SOURCE_SECTION, valid_label};
use livecore::artifact::{self, Manifest, append_custom_section};
use livecore::manifest::World as WitWorld;
use sha2::{Digest, Sha256};

/// The runtime every build links: the allocator, `abort` and `<string.h>`.
pub const BUILDRT: &str = include_str!("../../kai/buildrt.c");
/// An app's ADR 0001 D9 section, which a plain component lacks.
pub const APP_SECTION: &str = "kai.app";
/// Third-party notices (ADR 0009).
pub const NOTICES_SECTION: &str = "kai.notices";
/// The commit `cc.wasm` was built from (third_party/xcc/UPSTREAM.toml).
pub const XCC_COMMIT: &str = "631e318432aeaaf72d6d2dfc93085766934314a2";
/// `kai.notices`: xcc made the module.
const NOTICES: &str = concat!(
    "xcc (https://github.com/tyfkda/xcc) at 631e318432aeaaf72d6d2dfc93085766934314a2\n",
    "compiled and linked this module.\n\n",
    include_str!("../../../third_party/xcc/LICENSE"),
);
/// The stack each module gets, below its data: an overflow traps.
pub const STACK: u32 = 64 << 10;
/// Most files in a component's directory.
pub const MAX_FILES: usize = 64;
/// Longest file name.
pub const MAX_FILE_NAME: usize = 64;
/// Most bytes of objects a build holds for its link: what wcc's link could
/// load at all, so a source that expands into huge objects stops early.
pub const MAX_OBJECTS: usize = 4 << 20;
/// What an artifact says when its sources name no one licence.
pub const NO_LICENCE: &str = "NOASSERTION";

/// Which world a build is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `kai:app/component`: an artifact only.
    Component,
    /// `kai:app/app` with an empty `actions`, which `pkg` installs from `local`.
    App,
}

impl Kind {
    fn world(self) -> &'static World {
        match self {
            Kind::Component => &bind::COMPONENT,
            Kind::App => &bind::APP,
        }
    }

    /// What `files` build: an app if one of them includes the app world's
    /// header (`#include <app.h>` or `"app.h"`), else a component.
    pub fn of(files: &[(&str, &[u8])]) -> Kind {
        let header = bind::APP.header_name;
        let includes_app = |text: &[u8]| {
            let text = String::from_utf8_lossy(text);
            text.lines().any(|line| {
                let Some(rest) = line.trim_start().strip_prefix('#') else {
                    return false;
                };
                let Some(name) = rest.trim_start().strip_prefix("include") else {
                    return false;
                };
                // Bytes, not a str slice: a split character must not panic.
                let name = name.trim().as_bytes();
                name.len() == header.len() + 2
                    && matches!(name[0], b'<' | b'"')
                    && &name[1..name.len() - 1] == header.as_bytes()
            })
        };
        match files.iter().any(|(_, text)| includes_app(text)) {
            true => Kind::App,
            false => Kind::Component,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Component => "component",
            Kind::App => "app",
        }
    }
}

/// What to build.
pub struct Build<'a> {
    /// The component's name, as a package's: 2-12 of a-z, 0-9 and `-`,
    /// starting with a letter, and no OS row group.
    pub name: &'a str,
    /// `<major>.<minor>.<patch>`, as `pkg`'s `local-next` proposes.
    pub version: &'a str,
    pub kind: Kind,
    /// The component's directory: `.c` sources and `.h` headers, by plain name.
    pub files: &'a [(&'a str, &'a [u8])],
}

/// A finished build.
pub struct Built {
    pub artifact: Vec<u8>,
    /// SHA-256 of the `kai.source` bundle, in hex.
    pub source: String,
}

impl core::fmt::Debug for Built {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Built({} bytes, source {})",
            self.artifact.len(),
            self.source
        )
    }
}

impl Built {
    /// The ledger's record of it (ADR 0012 D7).
    pub fn provenance(&self) -> String {
        format!("built-by cc {XCC_COMMIT} source {}", self.source)
    }
}

/// Why a build made no artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    /// A name, version or file the build refuses.
    Request(String),
    /// A file did not compile: its name, and why.
    Compile(String, CompileError),
    /// wcc did not link the objects, such as for a symbol nothing defines.
    Link(CompileError),
    /// The linked module asks for or offers what its world does not, or the
    /// artifact would not load.
    Module(String),
}

impl BuildError {
    /// The error as the shell shows it, or why the build was stopped, which
    /// the reply says with a `trap`.
    pub fn text(&self) -> Result<String, crate::Stopped> {
        let compile = |e: &CompileError| match e {
            CompileError::Source(messages) => Ok(messages.clone()),
            CompileError::Stopped(stop) => Err(*stop),
            CompileError::Compiler(why) => Ok(format!("cc: {why}")),
        };
        match self {
            BuildError::Request(why) | BuildError::Module(why) => Ok(why.clone()),
            BuildError::Compile(name, e) => compile(e).map(|why| format!("{name}: {why}")),
            BuildError::Link(e) => compile(e).map(|why| format!("the link: {why}")),
        }
    }
}

/// `cc`'s compiler with the headers builds include: xcc's, `<stdlib.h>` and
/// each world's.
pub fn compiler(cc: &Compiler) -> Result<Compiler, String> {
    let mut headers: Vec<(&str, &[u8])> = XCC_HEADERS.to_vec();
    headers.push(("stdlib.h", bind::STDLIB_H));
    for world in [&bind::COMPONENT, &bind::APP] {
        headers.push((world.header_name, world.header));
    }
    cc.with_headers(&headers)
}

/// Builds `build` with `compiler` (from [`compiler`]).
pub fn build(compiler: &Compiler, build: &Build, budget: &mut Budget) -> Result<Built, BuildError> {
    let request = Request::check(build)?;
    let world = build.kind.world();
    let (objects, imports) = compile(compiler, world, &request.files, budget)?;
    let linked = link(compiler, world, &objects, budget)?;
    let module = module::finish(&linked, &imports, world.exports).map_err(BuildError::Module)?;
    // As wasmhost will load it. wcc's linker keeps one type per import, so a
    // call through a world import declared with another type fails here.
    let engine = wasmi::Engine::new(&crate::engine::config(true));
    wasmi::Module::validate(&engine, &module)
        .map_err(|e| BuildError::Module(format!("the module does not validate: {e}")))?;
    write(module, build.kind, &request)
}

/// A request that passed its checks, which all come before anything compiles.
struct Request<'a> {
    name: &'a str,
    /// Sorted by name.
    files: Vec<(&'a str, &'a [u8])>,
    /// `kai.source`: the files' bundle.
    bundle: Vec<u8>,
    /// `app:<name>/<name>@<version>`.
    world: String,
    /// The manifest's version label, `<major>-<minor>-<patch>`.
    label: String,
}

impl<'a> Request<'a> {
    fn check(build: &Build<'a>) -> Result<Self, BuildError> {
        let name = build.name;
        valid_name(name)?;
        let (major, minor, patch) = valid_version(build.version)?;
        let world = format!("app:{name}/{name}@{major}.{minor}.{patch}");
        if WitWorld::parse(&world).is_none() {
            return Err(BuildError::Request(format!(
                "{name} is not a WIT name: each part between - starts with a letter"
            )));
        }
        let label = format!("{major}-{minor}-{patch}");
        if !valid_label(&label) {
            return Err(BuildError::Request(format!(
                "{} is too long for a version label",
                build.version
            )));
        }
        let mut files = build.files.to_vec();
        files.sort_by(|a, b| a.0.cmp(b.0));
        valid_files(&files)?;
        let bundle = artifact::bundle(&files);
        if bundle.len() > artifact::MAX_SOURCE {
            return Err(BuildError::Request(format!(
                "the sources are over {} KiB",
                artifact::MAX_SOURCE >> 10
            )));
        }
        Ok(Self {
            name,
            files,
            bundle,
            world,
            label,
        })
    }
}

/// The objects a build holds for its link: at most [`MAX_OBJECTS`] bytes.
#[derive(Default)]
struct Objects {
    list: Vec<Vec<u8>>,
    bytes: usize,
}

impl Objects {
    /// Holds `name`'s object, unless it takes them past the cap.
    fn hold(&mut self, name: &str, object: Vec<u8>) -> Result<(), BuildError> {
        self.bytes = self.bytes.saturating_add(object.len());
        if self.bytes > MAX_OBJECTS {
            return Err(BuildError::Module(format!(
                "{name} takes the objects over {} MiB",
                MAX_OBJECTS >> 20
            )));
        }
        self.list.push(object);
        Ok(())
    }
}

/// The objects to link, the world's glue first, then [`BUILDRT`] and each
/// `.c`; and the world's imports, read from the glue's object.
fn compile(
    compiler: &Compiler,
    world: &World,
    files: &[(&str, &[u8])],
    budget: &mut Budget,
) -> Result<(Vec<Vec<u8>>, Vec<module::Import>), BuildError> {
    let mut objects = Objects::default();
    let mut one = |sources: &[(&str, &[u8])], name: &str, budget: &mut Budget| {
        let object = compiler
            .compile_in(sources, name, budget)
            .map_err(|e| BuildError::Compile(name.to_string(), e))?;
        objects.hold(name, object)
    };
    one(&[("glue.c", world.glue.as_bytes())], "glue.c", budget)?;
    one(&[("buildrt.c", BUILDRT.as_bytes())], "buildrt.c", budget)?;
    for (name, _) in files.iter().filter(|(n, _)| n.ends_with(".c")) {
        one(files, name, budget)?;
    }
    let imports = module::world_imports(&objects.list[0]).map_err(BuildError::Module)?;
    Ok((objects.list, imports))
}

/// wcc's link of `objects`, exporting the world's symbols and [`module::TRAP`].
fn link(
    compiler: &Compiler,
    world: &World,
    objects: &[Vec<u8>],
    budget: &mut Budget,
) -> Result<Vec<u8>, BuildError> {
    let names: Vec<String> = (0..objects.len()).map(|i| format!("{i}.o")).collect();
    let named: Vec<(&str, &[u8])> = names
        .iter()
        .map(String::as_str)
        .zip(objects.iter().map(Vec::as_slice))
        .collect();
    let mut exports: Vec<&str> = world
        .exports
        .lines()
        .filter_map(|l| l.split(' ').next())
        .collect();
    exports.push(module::TRAP);
    let linked = compiler
        .link(&named, &exports, STACK, budget)
        .map_err(BuildError::Link)?;
    match linked.len() > MAX_ARTIFACT {
        true => Err(BuildError::Module(format!(
            "the module is over {} KiB",
            MAX_ARTIFACT >> 10
        ))),
        false => Ok(linked),
    }
}

/// The module with the artifact's sections.
fn write(mut module: Vec<u8>, kind: Kind, request: &Request) -> Result<Built, BuildError> {
    let source = hex(&Sha256::digest(&request.bundle));
    let name = request.name;
    let manifest = Manifest {
        lifecycle: 2,
        component: name.into(),
        version: request.label.clone(),
        world: request.world.clone(),
        engine: "wasmi".into(),
        features: "-".into(),
        licence: licence_of(&request.files),
        build: hex(&Sha256::digest(&module)),
        source: source.clone(),
    };
    append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
    append_custom_section(&mut module, SOURCE_SECTION, &request.bundle);
    let actions = format!("kai-actions 1\nworld {}\n", request.world);
    append_custom_section(&mut module, ACTIONS_SECTION, actions.as_bytes());
    if kind == Kind::App {
        let app = format!("kai-app 1\nid {name}\nabi 1.0\n");
        append_custom_section(&mut module, APP_SECTION, app.as_bytes());
    }
    append_custom_section(&mut module, NOTICES_SECTION, NOTICES.as_bytes());
    if module.len() > MAX_ARTIFACT {
        return Err(BuildError::Module(format!(
            "the artifact is over {} KiB",
            MAX_ARTIFACT >> 10
        )));
    }
    artifact::parse(&module).map_err(BuildError::Module)?;
    Ok(Built {
        artifact: module,
        source,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A package's name (kaipkg's rule): 2-12 of a-z, 0-9 and `-`, starting with
/// a letter, and no OS row group.
pub fn valid_name(name: &str) -> Result<(), BuildError> {
    let bytes = name.as_bytes();
    let shaped = (2..=12).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    if !shaped {
        return Err(BuildError::Request(
            "a name is 2-12 of a-z, 0-9 and -, starting with a letter".into(),
        ));
    }
    if agentabi::is_reserved(name) {
        return Err(BuildError::Request(format!("{name} is an OS row group")));
    }
    Ok(())
}

/// `<major>.<minor>.<patch>`: decimal, no leading zeros, each under 10^9.
pub fn valid_version(version: &str) -> Result<(u32, u32, u32), BuildError> {
    let bad = || BuildError::Request(format!("{version:?} is not a version such as 0.0.1"));
    let mut parts = version.split('.').map(|n| {
        let plain = !n.is_empty()
            && n.len() <= 9
            && n.bytes().all(|b| b.is_ascii_digit())
            && (n == "0" || !n.starts_with('0'));
        plain.then(|| n.parse::<u32>().ok()).flatten()
    });
    let version = (
        parts.next().flatten().ok_or_else(bad)?,
        parts.next().flatten().ok_or_else(bad)?,
        parts.next().flatten().ok_or_else(bad)?,
    );
    match parts.next() {
        None => Ok(version),
        Some(_) => Err(bad()),
    }
}

/// The directory's files, sorted by name: at most [`MAX_FILES`], each a
/// plain `.c` or `.h` name of letters, digits, `_` and `-`, at least one `.c`.
fn valid_files(files: &[(&str, &[u8])]) -> Result<(), BuildError> {
    let bad = |why: String| Err(BuildError::Request(why));
    if files.len() > MAX_FILES {
        return bad(format!("a component has at most {MAX_FILES} files"));
    }
    for (i, (name, _)) in files.iter().enumerate() {
        let stem = name.strip_suffix(".c").or_else(|| name.strip_suffix(".h"));
        let plain = stem.is_some_and(|stem| {
            !stem.is_empty()
                && name.len() <= MAX_FILE_NAME
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        });
        if !plain {
            return bad(format!(
                "{name:?} is not a .c or .h name of letters, digits, _ and -, \
                 at most {MAX_FILE_NAME} bytes"
            ));
        }
        if i > 0 && files[i - 1].0 == *name {
            return bad(format!("{name} is there twice"));
        }
    }
    match files.iter().any(|(n, _)| n.ends_with(".c")) {
        true => Ok(()),
        false => bad("a component has no .c file".into()),
    }
}

/// The licence the files' `SPDX-License-Identifier:` lines name, when each
/// file names one and all name the same; otherwise [`NO_LICENCE`].
pub fn licence_of(files: &[(&str, &[u8])]) -> String {
    const TAG: &str = "SPDX-License-Identifier:";
    let mut found: Option<&str> = None;
    for (_, data) in files {
        let Ok(text) = core::str::from_utf8(data) else {
            return NO_LICENCE.into();
        };
        let mut lines = text
            .lines()
            .filter_map(|l| Some(&l[l.find(TAG)? + TAG.len()..]));
        let (Some(line), None) = (lines.next(), lines.next()) else {
            return NO_LICENCE.into();
        };
        let licence = line.trim().trim_end_matches("*/").trim();
        if found.is_some_and(|f| f != licence) {
            return NO_LICENCE.into();
        }
        found = Some(licence);
    }
    match found {
        Some(licence) if valid_licence(licence) => licence.into(),
        _ => NO_LICENCE.into(),
    }
}

/// An SPDX expression's shape, as a manifest takes it.
fn valid_licence(licence: &str) -> bool {
    !licence.is_empty()
        && licence.len() <= livecore::manifest::MAX_LICENCE
        && licence
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" .+-()".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn objects_past_the_cap_are_refused_naming_the_file() {
        let mut objects = Objects::default();
        objects.hold("a.c", vec![0; MAX_OBJECTS - 1]).unwrap();
        objects.hold("b.c", vec![0; 1]).unwrap();
        match objects.hold("c.c", vec![0; 1]) {
            Err(BuildError::Module(why)) => assert!(why.starts_with("c.c "), "{why}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(objects.list.len(), 2);
    }

    /// Security review: an `#include` line that splits a character where the
    /// header's name would end is not an app's, and does not panic.
    #[test]
    fn the_kind_is_read_from_includes_whatever_their_bytes() {
        let of = |text: &[u8]| Kind::of(&[("a.c", text)]);
        assert_eq!(of(b"#include <app.h>\n"), Kind::App);
        assert_eq!(of(b"  #  include \"app.h\"\n"), Kind::App);
        assert_eq!(of(b"#include <component.h>\n"), Kind::Component);
        assert_eq!(of("#include <abcd\u{e9}\n".as_bytes()), Kind::Component);
        assert_eq!(of(b"#include <app.\xff\n"), Kind::Component);
        assert_eq!(of(b"#include <\xff\xfeapp.h>\n"), Kind::Component);
        assert_eq!(of(b"// #include <app.h>\n"), Kind::Component);
        assert_eq!(of(b"#include <app.hh>\n"), Kind::Component);
    }
}
