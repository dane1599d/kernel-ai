//! The service an app package runs as (P4, the window apps): its artifact's
//! component, which is the package's own name or its window, `<name>-window`
//! (package `notes` carries the Notes window, `notes-window`). No package may be
//! named `<x>-window` ([`crate::header::valid_name`]), so a window's service
//! comes from its one package: none can carry another app's component and run
//! in its place.

use crate::bundle;
use crate::generation::Installed;
use crate::header::{Header, Kind};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What a window's service name ends with.
pub const WINDOW: &str = "-window";

/// Whether package `name`'s app may run as `component`: itself, or its window.
pub fn may_run_as(name: &str, component: &str) -> bool {
    component == name || component.strip_suffix(WINDOW) == Some(name)
}

/// The service `object`'s app runs as, its artifact's component; none for a
/// package that is not an app, or an app without an artifact (nothing of it
/// runs). An artifact of another component is refused.
pub fn service(header: &Header, object: &[u8]) -> Result<Option<String>, String> {
    if header.kind != Kind::App {
        return Ok(None);
    }
    let files = bundle::files(object)?;
    let Some((_, artifact)) = files.iter().find(|(path, _)| *path == bundle::ARTIFACT) else {
        return Ok(None);
    };
    let component = livecore::artifact::parse(artifact)?.manifest.component;
    match may_run_as(&header.name, &component) {
        true => Ok(Some(component)),
        false => Err(format!(
            "its artifact is component {component}, not {name} or {name}{WINDOW}",
            name = header.name
        )),
    }
}

/// What starting a generation's apps came to ([`start_each`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Starts {
    /// What `start` said of each app it started.
    pub started: Vec<String>,
    /// `<name> <version> is installed but not started: <why>`, one a
    /// package whose start failed.
    pub not_started: Vec<String>,
}

/// Starts each of `packages` with `start`, one at a time, so one that fails
/// (its component not taken, an answer the supervisor should not give)
/// never keeps the rest from starting at every boot. `start` answers
/// `Ok(Some(line))` for an app it started, `Ok(None)` for a package it passed
/// over (not an app, or one it said why it does not start), and `Err(why)`
/// for a failure, which is said and gone past.
pub fn start_each<F>(packages: &[Installed], mut start: F) -> Starts
where
    F: FnMut(&Installed) -> Result<Option<String>, String>,
{
    let mut starts = Starts::default();
    for package in packages {
        match start(package) {
            Ok(Some(line)) => starts.started.push(line),
            Ok(None) => {}
            Err(why) => starts.not_started.push(format!(
                "{} {} is installed but not started: {why}",
                package.name, package.version
            )),
        }
    }
    starts
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use alloc::vec::Vec;
    use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION, SOURCE_SECTION};
    use livecore::artifact::{Manifest, append_custom_section};

    /// App package `name` 1.0.0 around an artifact of `component`.
    pub(crate) fn package(name: &str, component: &str) -> (Header, Vec<u8>) {
        let text = format!(
            "kai-package 1\nname {name}\nversion 1.0.0\nkind app\nsummary An app\n\
             licence MPL-2.0\nworld kai:app@0.1\n"
        );
        let world = format!("app:{component}/{component}@1.0.0");
        let manifest = Manifest {
            lifecycle: 2,
            component: component.into(),
            version: "1".into(),
            world: world.clone(),
            engine: "wasmi".into(),
            features: "-".into(),
            licence: "MPL-2.0".into(),
            build: "0123456789abcdef".into(),
            source: "fedcba9876543210".into(),
        };
        let mut module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
        append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
        let source = livecore::artifact::bundle(&[("src/lib.rs", b"//\n")]);
        append_custom_section(&mut module, SOURCE_SECTION, &source);
        let actions = format!("kai-actions 1\nworld {world}\n");
        append_custom_section(&mut module, ACTIONS_SECTION, actions.as_bytes());
        let object = bundle::build(&[
            (bundle::HEADER, text.as_bytes()),
            (bundle::ARTIFACT, &module),
        ]);
        (Header::parse(&text).unwrap(), object)
    }

    #[test]
    fn an_app_runs_as_its_own_component_or_its_window() {
        let (header, object) = package("notes", "notes");
        assert_eq!(service(&header, &object), Ok(Some("notes".into())));
        let (header, object) = package("notes", "notes-window");
        assert_eq!(service(&header, &object), Ok(Some("notes-window".into())));
    }

    #[test]
    fn an_app_carrying_another_component_is_refused() {
        for other in ["files-window", "notes-windows", "window", "settings"] {
            let (header, object) = package("notes", other);
            let why = service(&header, &object).unwrap_err();
            assert!(why.contains("not notes or notes-window"), "{other}: {why}");
        }
        assert!(!may_run_as("notes", "-window"));
        assert!(!may_run_as("notes", "notes-window-window"));
    }

    #[test]
    fn the_store_refuses_a_download_that_would_run_as_another_app() {
        let (_, object) = package("files", "notes-window");
        let why = crate::store::Refused::AnotherApp("files".into());
        let installed = crate::effects::tests::install(&object, &[]);
        assert_eq!(installed, Err(crate::store::Error::Refused(why)));
        let (_, window) = package("files", "files-window");
        assert_eq!(crate::effects::tests::install(&window, &[]), Ok(2));
    }

    #[test]
    fn a_package_that_is_not_an_app_runs_as_nothing() {
        let text =
            "kai-package 1\nname words\nversion 1.0.0\nkind data\nsummary Words\nlicence MIT\n";
        let header = Header::parse(text).unwrap();
        let object = bundle::build(&[(bundle::HEADER, text.as_bytes()), ("words.txt", b"a\n")]);
        assert_eq!(service(&header, &object), Ok(None));
    }

    /// One package whose start fails is said and gone past: the ones after
    /// it still start (the audit: one COMP_ADD refused kept every later app
    /// from starting at every boot).
    #[test]
    fn one_failed_start_never_stops_the_rest() {
        use crate::generation::Installed;
        use crate::version::Version;
        let package = |name: &str| Installed {
            name: String::from(name),
            version: Version::parse("1.0.0").unwrap(),
            sha: crate::sha256(name.as_bytes()),
            explicit: true,
            grants: alloc::vec::Vec::new(),
        };
        let packages = [
            package("notes"),
            package("timer"),
            package("lib"),
            package("clock"),
        ];
        let mut tried = alloc::vec::Vec::new();
        let starts = start_each(&packages, |p| {
            tried.push(p.name.clone());
            match p.name.as_str() {
                "timer" => Err(String::from("COMP_ADD timer: NoMemory")),
                "lib" => Ok(None),
                name => Ok(Some(format!("{name} started"))),
            }
        });
        assert_eq!(tried, ["notes", "timer", "lib", "clock"]);
        assert_eq!(starts.started, ["notes started", "clock started"]);
        assert_eq!(
            starts.not_started,
            ["timer 1.0.0 is installed but not started: COMP_ADD timer: NoMemory"]
        );
    }
}
