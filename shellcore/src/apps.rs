//! Installed apps' action rows (P4, the agent-to-app route): what an app's
//! `kai-actions 1` answer declares, checked and turned into ADR 0002 rows with
//! owner `App(<app>)`, so the agent core decides, runs, logs and undoes them as
//! it does the OS's.
//!
//! The rows come from the artifact comphost loaded (`livecore::rows`), never
//! from the app's code. Each name must start with `<app>_`, where `<app>` is
//! the kernel's name for the process, not anything the app says. A row's tier
//! is the OS's, from its facts: a fact the app left out takes ADR 0002's
//! pessimistic default, so the app can only make a tier stricter.
//!
//! The rows are leaked into `'static` to fit ADR 0002's static row type, at
//! most [`MAX_ROWS`] a version and [`MAX_LOADS`] versions in all until reboot;
//! past that a load is refused (owned rows replace this: ADR 0002 and 0003).

use agentabi::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use appabi::Ty;
use livecore::actions::{self as declared, ActionRow, ArgKind};

/// Most rows one app version may declare.
pub const MAX_ROWS: usize = 64;
/// Most app versions whose rows load until reboot.
pub const MAX_LOADS: usize = 32;
/// The longest text a row's text argument takes when the app sets no `max=`.
const TEXT_MAX: u16 = 2000;
/// The most items a list argument takes.
const LIST_MAX: u8 = 32;

/// One loaded row and what calling it needs.
#[derive(Clone, Debug)]
pub struct AppRow {
    pub row: &'static Action,
    pub app: &'static str,
    /// The export `kai-call 1` names, such as `app:notes/actions@1.0.0#add`.
    pub export: String,
    /// The `args` record's type.
    pub args: Ty,
}

struct Loaded {
    app: String,
    version: String,
    rows: Vec<AppRow>,
}

/// Every app's rows, the newest version of each.
#[derive(Default)]
pub struct AppRows {
    loaded: Vec<Loaded>,
    loads: usize,
}

impl AppRows {
    /// Loads app `app`'s rows from its `kai-actions 1` answer (`version <label>`
    /// then the section): how many rows it has, or why none loaded. A version
    /// already loaded is not loaded again.
    pub fn load(&mut self, app: &str, answer: &str) -> Result<usize, String> {
        let (head, section) = answer
            .split_once('\n')
            .ok_or_else(|| format!("{app}'s rows are not a kai-actions answer"))?;
        let version = head
            .strip_prefix("version ")
            .ok_or_else(|| format!("{app}'s rows: {head}"))?;
        if let Some(loaded) = self
            .loaded
            .iter()
            .find(|l| l.app == app && l.version == version)
        {
            return Ok(loaded.rows.len());
        }
        if self.loads == MAX_LOADS {
            return Err(format!(
                "{app}'s rows were not loaded: {MAX_LOADS} app versions loaded since boot; a restart loads more"
            ));
        }
        let parsed = declared::parse(section).map_err(|why| format!("{app}'s rows: {why}"))?;
        if parsed.rows.len() > MAX_ROWS {
            return Err(format!("{app} declares over {MAX_ROWS} actions"));
        }
        let rows = parsed
            .rows
            .iter()
            .map(|row| convert(app, row))
            .collect::<Result<Vec<_>, _>>()?;
        // A model calls a row by its name alone, so no two apps share one.
        if let Some(taken) = rows.iter().find_map(|row| {
            self.loaded
                .iter()
                .filter(|l| l.app != app)
                .find(|l| l.rows.iter().any(|r| r.row.name == row.row.name))
                .map(|l| (row.row.name, l.app.clone()))
        }) {
            return Err(format!("{app}'s action {} is {}'s", taken.0, taken.1));
        }
        self.loads += 1;
        self.loaded.retain(|l| l.app != app);
        let count = rows.len();
        self.loaded.push(Loaded {
            app: app.to_string(),
            version: version.to_string(),
            rows,
        });
        Ok(count)
    }

    /// The loaded row named `name`.
    pub fn find(&self, name: &str) -> Option<&AppRow> {
        self.loaded
            .iter()
            .flat_map(|l| &l.rows)
            .find(|row| row.row.name == name)
    }

    /// App `app`'s own row named `name`, and no other app's: a bound control
    /// in `app`'s window takes the tier of `app`'s row (ADR 0005 §6), so `notes`
    /// never reaches `notes_window_*` rows of an app `notes-window`.
    pub fn find_in(&self, app: &str, name: &str) -> Option<&AppRow> {
        self.loaded
            .iter()
            .filter(|l| l.app == app)
            .flat_map(|l| &l.rows)
            .find(|row| row.row.name == name)
    }

    /// Every loaded row.
    pub fn rows(&self) -> impl Iterator<Item = &'static Action> + '_ {
        self.loaded
            .iter()
            .flat_map(|l| l.rows.iter().map(|r| r.row))
    }

    /// Whether `app`'s rows are loaded.
    pub fn has(&self, app: &str) -> bool {
        self.loaded.iter().any(|l| l.app == app)
    }
}

fn leak<T>(value: T) -> &'static T {
    Box::leak(Box::new(value))
}

fn leak_str(text: &str) -> &'static str {
    Box::leak(text.to_string().into_boxed_str())
}

fn leak_strs(texts: &[String]) -> &'static [&'static str] {
    Box::leak(
        texts
            .iter()
            .map(|t| leak_str(t))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    )
}

/// One declared row as an ADR 0002 row, checked as the OS's rows are.
fn convert(app: &str, row: &ActionRow) -> Result<AppRow, String> {
    let prefix = format!("{}_", app.replace('-', "_"));
    let function = row
        .name
        .strip_prefix(&prefix)
        .ok_or_else(|| format!("{app}'s action {} does not start {prefix}", row.name))?;
    if agentabi::takes_os_name(app, function) {
        return Err(format!("{app}'s action {} is an OS row's name", row.name));
    }
    let args = row
        .args
        .iter()
        .zip(&row.func.params)
        .map(|(arg, ty)| {
            Ok(Arg {
                name: leak_str(&arg.name),
                ty: arg_type(&arg.kind, ty, arg.min, arg.max)
                    .ok_or_else(|| format!("{}: argument {} has no OS type", row.name, arg.name))?,
                role: match arg.role {
                    declared::Role::Target => Role::Target,
                    declared::Role::Payload => Role::Payload,
                    declared::Role::Option => Role::Option,
                },
                from_request: arg.from_request,
                never: &[],
                aliases: &[],
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let read_only = row.facts.read_only.unwrap_or(false);
    let inverse = match (&row.inverse, read_only) {
        (_, true) => Inverse::Read,
        (declared::Inverse::Snapshot, false) => Inverse::Snapshot,
        (declared::Inverse::Action(name), false) => Inverse::Compensate(leak_str(name)),
        (declared::Inverse::None, false) => Inverse::None("the app's own action has no undo"),
    };
    // A change with no undo is destructive whatever the app says.
    let destructive = row.facts.destructive.unwrap_or(true)
        || (!read_only && matches!(inverse, Inverse::None(_)));
    let action = Action {
        name: leak_str(&row.name),
        doc: leak_str(&row.doc),
        args: Box::leak(args.into_boxed_slice()),
        facts: Facts {
            read_only,
            destructive: destructive && !read_only,
            idempotent: row.facts.idempotent.unwrap_or(false),
            open_world: row.facts.open_world.unwrap_or(true),
        },
        inverse,
        cues: leak_strs(&row.cues),
        // A result is the app's content (ADR 0003 §5).
        returns: Returns::Content(leak_str(app)),
        ret: RetType::Data,
        owner: Owner::App(leak_str(app)),
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    };
    agentabi::check(core::slice::from_ref(&action))
        .map_err(|why| format!("{app}'s action {}: {why}", row.name))?;
    Ok(AppRow {
        row: leak(action),
        app: leak_str(app),
        export: row.export.clone(),
        args: row.args_record(),
    })
}

/// The OS's type for a declared argument of WIT type `ty`.
fn arg_type(kind: &ArgKind, ty: &Ty, min: Option<i64>, max: Option<i64>) -> Option<ArgType> {
    Some(match (kind, ty) {
        (ArgKind::Text, _) => ArgType::Text {
            max: max.map_or(TEXT_MAX, |m| u16::try_from(m).unwrap_or(u16::MAX)),
        },
        (ArgKind::Int, _) => ArgType::Int {
            min: min.unwrap_or(0),
            max: max.unwrap_or(i64::from(u32::MAX)),
        },
        (ArgKind::Bool, _) => ArgType::Enum(&["false", "true"]),
        (ArgKind::Enum, Ty::Enum(cases)) => ArgType::Enum(leak_strs(cases)),
        (ArgKind::Path, _) => ArgType::Path,
        (ArgKind::Host, _) => ArgType::Host,
        (ArgKind::List(inner), Ty::List(item)) => {
            ArgType::List(leak(arg_type(inner, item, min, max)?), LIST_MAX)
        }
        (ArgKind::Option(inner), Ty::Option(item)) => {
            ArgType::Opt(leak(arg_type(inner, item, min, max)?))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTES: &str = "version 2
kai-actions 1
world app:notes/notes@1.0.0
action notes_add
export app:notes/actions@1.0.0#add
doc Add a note and return its id.
sig func(text: string) -> result<u32, string>
arg text text max=2000 role=payload from-request
facts read-only=no destructive=no idempotent=no open-world=no
inverse snapshot
end
action notes_count
export app:notes/actions@1.0.0#count
doc How many notes there are.
sig func() -> u32
facts read-only=yes destructive=no idempotent=yes open-world=no
inverse none
end
";

    #[test]
    fn an_apps_rows_load_as_its_own_with_the_oss_tiers() {
        let mut apps = AppRows::default();
        assert_eq!(apps.load("notes", NOTES), Ok(2));
        let add = apps.find("notes_add").unwrap();
        assert_eq!(add.row.owner, Owner::App("notes"));
        assert_eq!(add.export, "app:notes/actions@1.0.0#add");
        assert_eq!(agentabi::derive(add.row), Tier::Run);
        assert!(matches!(add.row.inverse, Inverse::Snapshot));
        let count = apps.find("notes_count").unwrap();
        assert!(count.row.facts.read_only);
        // The same version again loads nothing new.
        assert_eq!(apps.load("notes", NOTES), Ok(2));
        assert_eq!(apps.rows().count(), 2);
    }

    #[test]
    fn a_fact_left_out_is_pessimistic_and_no_undo_asks() {
        let rows = NOTES.replace(
            "facts read-only=no destructive=no idempotent=no open-world=no\ninverse snapshot",
            "inverse none",
        );
        let mut apps = AppRows::default();
        apps.load("notes", &rows).unwrap();
        let add = apps.find("notes_add").unwrap();
        assert!(add.row.facts.destructive && add.row.facts.open_world);
        assert_eq!(agentabi::derive(add.row), Tier::Ask);
    }

    #[test]
    fn a_row_must_carry_its_apps_kernel_name() {
        let mut apps = AppRows::default();
        let err = apps.load("files", NOTES).unwrap_err();
        assert!(err.contains("does not start files_"), "{err}");
        assert!(apps.load("notes", "kai-actions 1\n").is_err());
    }

    #[test]
    fn a_bound_control_finds_only_its_own_apps_row() {
        let mut apps = AppRows::default();
        apps.load("notes", NOTES).unwrap();
        let window = NOTES
            .replace("notes_add", "notes_window_add")
            .replace("notes_count", "notes_window_count")
            .replace("app:notes/", "app:notes-window/");
        apps.load("notes-window", &window).unwrap();
        assert!(apps.find_in("notes-window", "notes_window_add").is_some());
        assert!(apps.find_in("notes", "notes_window_add").is_none());
        assert!(apps.find_in("notes-window", "notes_add").is_none());
        // `notes` may not take `notes-window`'s name for a row of its own.
        let taken = NOTES.replace("notes_count", "notes_window_count");
        let err = apps
            .load("notes", &taken.replacen("version 2", "version 3", 1))
            .unwrap_err();
        assert!(err.contains("notes-window's"), "{err}");
    }

    #[test]
    fn loads_are_bounded_until_reboot() {
        let mut apps = AppRows::default();
        for version in 0..MAX_LOADS {
            let rows = NOTES.replacen("version 2", &format!("version {version}"), 1);
            apps.load("notes", &rows).unwrap();
        }
        let err = apps
            .load("notes", &NOTES.replacen("version 2", "version x", 1))
            .unwrap_err();
        assert!(err.contains("restart"), "{err}");
        assert_eq!(apps.rows().count(), 2);
    }
}
