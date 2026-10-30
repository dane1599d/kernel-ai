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

use crate::bind::{self, World};
use crate::compiler::{CompileError, Compiler};
use crate::{Budget, XCC_HEADERS};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION, MAX_ARTIFACT, SOURCE_SECTION};
use livecore::artifact::{self, Manifest, append_custom_section};
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
    valid_name(build.name)?;
    let version = valid_version(build.version)?;
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
    let world = build.kind.world();

    let one = |files: &[(&str, &[u8])], name: &str, budget: &mut Budget| {
        compiler
            .compile_in(files, name, budget)
            .map_err(|e| BuildError::Compile(name.to_string(), e))
    };
    let glue = one(&[("glue.c", world.glue.as_bytes())], "glue.c", budget)?;
    let imports = module::world_imports(&glue).map_err(BuildError::Module)?;
    let mut objects = Vec::from([
        glue,
        one(&[("buildrt.c", BUILDRT.as_bytes())], "buildrt.c", budget)?,
    ]);
    for (name, _) in files.iter().filter(|(n, _)| n.ends_with(".c")) {
        objects.push(one(&files, name, budget)?);
    }
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
    let module = module::finish(&linked, &imports, world.exports).map_err(BuildError::Module)?;
    // As wasmhost will load it. wcc's linker keeps one type per import, so a
    // call through a world import declared with another type fails here.
    let engine = wasmi::Engine::new(&crate::engine::config(true));
    wasmi::Module::validate(&engine, &module)
        .map_err(|e| BuildError::Module(format!("the module does not validate: {e}")))?;
    write(module, build, &version, &files, &bundle)
}

/// The module with the artifact's sections; `bundle` is `files`'.
fn write(
    mut module: Vec<u8>,
    build: &Build,
    version: &(u32, u32, u32),
    files: &[(&str, &[u8])],
    bundle: &[u8],
) -> Result<Built, BuildError> {
    let source = hex(&Sha256::digest(bundle));
    let name = build.name;
    let (major, minor, patch) = version;
    let world = format!("app:{name}/{name}@{major}.{minor}.{patch}");
    let manifest = Manifest {
        lifecycle: 2,
        component: name.into(),
        version: format!("{major}-{minor}-{patch}"),
        world: world.clone(),
        engine: "wasmi".into(),
        features: "-".into(),
        licence: licence_of(files),
        build: hex(&Sha256::digest(&module)),
        source: source.clone(),
    };
    append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
    append_custom_section(&mut module, SOURCE_SECTION, bundle);
    let actions = format!("kai-actions 1\nworld {world}\n");
    append_custom_section(&mut module, ACTIONS_SECTION, actions.as_bytes());
    if build.kind == Kind::App {
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
