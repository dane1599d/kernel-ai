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

/// The shell's apps: their rows, and the effects their calls queued, which
/// the agent core runs once the call's run succeeded.
#[derive(Default)]
pub struct Apps {
    pub rows: AppRows,
    pub effects: Vec<(String, String)>,
}

impl Apps {
    /// What the runs queued since the last take, as (app, effect lines).
    pub fn take_effects(&mut self) -> Vec<(String, String)> {
        core::mem::take(&mut self.effects)
    }
}

/// Loads the rows of the app whose `<app>_` names `name`, if a service of
/// that name answers `kai-actions 1`: the first call to an app's row loads its
/// rows, so no app starts only to be listed. The longest name wins, so
/// `notes_window_add` is `notes-window`'s before `notes`'s.
pub(crate) fn load_for(host: &dyn crate::Host, name: &str) {
    let Some(apps) = host.apps() else {
        return;
    };
    let Ok(services) = host.services() else {
        return;
    };
    let starts = |pkg: &str| name.starts_with(&format!("{}_", pkg.replace('-', "_")));
    // A service named for the row's start, the longest; else a window that
    // fronts that package (`notes_add` is `notes-window`'s with no `notes`).
    let named = services
        .iter()
        .map(|s| s.name.as_str())
        .filter(|app| starts(app))
        .max_by_key(|app| app.len());
    let window = || {
        services
            .iter()
            .map(|s| s.name.as_str())
            .filter(|app| app.strip_suffix("-window").is_some_and(starts))
            .max_by_key(|app| app.len())
    };
    let Some(app) = named.or_else(window) else {
        return;
    };
    if apps.borrow().rows.has(app) || apps.borrow().rows.failed(app) {
        return;
    }
    let others: Vec<String> = services
        .iter()
        .map(|s| s.name.clone())
        .filter(|name| name != app)
        .collect();
    let answer = match host.app_call(app, "kai-actions 1") {
        Ok(answer) => answer,
        Err(why) => return host.audit(&format!("{app}'s rows were not asked for: {why}")),
    };
    let loaded = apps.borrow_mut().rows.load_among(app, &answer, &others);
    match loaded {
        Ok(count) => host.audit(&format!("{app}'s {count} actions loaded")),
        Err(why) => host.audit(&why),
    }
}

/// Loads app `app`'s rows, by its name, if they are not loaded yet: for its
/// effects, which may come before any call to its rows.
pub(crate) fn load_app(host: &dyn crate::Host, app: &str) {
    let Some(apps) = host.apps() else {
        return;
    };
    if apps.borrow().rows.has(app) || apps.borrow().rows.failed(app) {
        return;
    }
    let others: Vec<String> = host
        .services()
        .map(|all| {
            all.into_iter()
                .map(|s| s.name)
                .filter(|n| n != app)
                .collect()
        })
        .unwrap_or_default();
    let answer = match host.app_call(app, "kai-actions 1") {
        Ok(answer) => answer,
        Err(why) => return host.audit(&format!("{app}'s rows were not asked for: {why}")),
    };
    let loaded = apps.borrow_mut().rows.load_among(app, &answer, &others);
    match loaded {
        Ok(count) => host.audit(&format!("{app}'s {count} actions loaded")),
        Err(why) => host.audit(&why),
    }
}

/// `kai-undo 1` of app `app`'s call kept as `id`, with `force` if `anyway`:
/// the undo's own id, which undoes it again, or why not.
pub(crate) fn undo(
    host: &dyn crate::Host,
    app: &str,
    id: u64,
    anyway: bool,
) -> Result<u64, crate::undo::Refused> {
    use crate::undo::Refused;
    let force = if anyway { "force\n" } else { "" };
    let request = format!("kai-undo 1\nid {id}\n{force}");
    let text = host.app_call(app, &request).map_err(Refused::Failed)?;
    let mut lines = text.lines();
    if lines.next() != Some(livecore::call::REPLY_HEADER) {
        return Err(Refused::Failed(format!("{app} did not answer the undo")));
    }
    let mut redo = None;
    for line in lines {
        match line.split_once(' ') {
            Some(("undone", _)) => {}
            Some(("undo", next)) => redo = next.parse().ok(),
            Some(("changed", _)) => {
                return Err(Refused::Moved(format!("{app} changed since")));
            }
            _ => return Err(Refused::Failed(format!("{app}: {line}"))),
        }
    }
    redo.ok_or_else(|| Refused::Failed(format!("{app} kept no undo of its undo")))
}

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
    /// The OS rows its effects may call, as its package declares them.
    effects: Vec<String>,
    /// The package a `<pkg>-window` service fronts, whose `<pkg>_` rows it
    /// owns ([`fronts`]).
    fronts: Option<String>,
}

/// The package `app` fronts: a `<pkg>-window` service whose world is
/// `app:<pkg>/…` is its package's window, so its rows may be `<pkg>_*` (Notes'
/// window declares `notes_add`). The service's name is the plan line's, which
/// the kernel holds to, so a package cannot claim another's rows by its world.
pub fn fronts<'a>(app: &'a str, world: &str) -> Option<&'a str> {
    let pkg = app.strip_suffix("-window")?;
    let (named, _) = world.strip_prefix("app:")?.split_once('/')?;
    // Fails closed on its own: an empty or odd name fronts nothing.
    let package = pkg.starts_with(|c: char| c.is_ascii_lowercase())
        && pkg
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    (package && named == pkg).then_some(pkg)
}

/// The OS rows an app may declare for its effects (P4, deny by default): the
/// file rows, in `/home/user` as the shell holds it. Every other row, the
/// package, rule, grant, profile, update and service rows above all, stays the
/// user's and Kai's to run, and a package declaring one does not load.
pub fn declarable(row: &str) -> bool {
    agentabi::FS_ROWS.iter().any(|r| r.name == row)
}

/// Every app's rows, the newest version of each.
#[derive(Default)]
pub struct AppRows {
    loaded: Vec<Loaded>,
    loads: usize,
    /// Apps whose rows did not load: not asked again until reboot.
    failed: Vec<String>,
}

impl AppRows {
    /// Loads app `app`'s rows from its `kai-actions 1` answer (`version <label>`
    /// then the section): how many rows it has, or why none loaded. A version
    /// already loaded is not loaded again.
    pub fn load(&mut self, app: &str, answer: &str) -> Result<usize, String> {
        self.load_among(app, answer, &[])
    }

    /// [`AppRows::load`], with the names of the other services: a row may not
    /// start with another's longer `<name>_` either, so `notes` cannot declare
    /// `notes_window_add` for `notes-window`. A load that fails is remembered,
    /// and counts against [`MAX_LOADS`] once anything of it was leaked.
    pub fn load_among(
        &mut self,
        app: &str,
        answer: &str,
        others: &[String],
    ) -> Result<usize, String> {
        let loaded = self.try_load(app, answer, others);
        if loaded.is_err() && !self.failed.iter().any(|f| f == app) {
            self.failed.push(app.to_string());
        }
        loaded
    }

    /// Whether `app`'s rows failed to load since boot.
    pub fn failed(&self, app: &str) -> bool {
        self.failed.iter().any(|f| f == app)
    }

    fn try_load(&mut self, app: &str, answer: &str, others: &[String]) -> Result<usize, String> {
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
        // A window fronts its package's rows while the package's own service
        // holds none, and the reverse: one owner for a prefix.
        let fronts = fronts(app, &parsed.world);
        if let Some(pkg) = fronts
            && self.has(pkg)
        {
            return Err(format!(
                "{app}'s rows are {pkg}'s, and {pkg} has them loaded"
            ));
        }
        if let Some(window) = self
            .loaded
            .iter()
            .find(|l| l.fronts.as_deref() == Some(app))
        {
            return Err(format!(
                "{app}'s rows are loaded for its window {}",
                window.app
            ));
        }
        // Every check on names comes before anything is leaked.
        for row in &parsed.rows {
            self.name_free(app, fronts, &row.name, others)?;
        }
        if let Some(row) = parsed.effects.iter().find(|row| !declarable(row)) {
            return Err(format!(
                "{app} declares effects that call {row}, which no app may"
            ));
        }
        // From here on rows are leaked, so the load counts even if one fails.
        self.loads += 1;
        let rows = parsed
            .rows
            .iter()
            .map(|row| convert(app, fronts, row))
            .collect::<Result<Vec<_>, _>>()?;
        self.loaded.retain(|l| l.app != app);
        let count = rows.len();
        self.loaded.push(Loaded {
            app: app.to_string(),
            version: version.to_string(),
            rows,
            effects: parsed.effects.clone(),
            fronts: fronts.map(str::to_string),
        });
        Ok(count)
    }

    /// Why `name` may not be one of `app`'s rows: no `<app>_` start (or, for a
    /// window, its package's), an OS row's name, a longer service name's
    /// start, or another loaded app's row.
    fn name_free(
        &self,
        app: &str,
        fronts: Option<&str>,
        name: &str,
        others: &[String],
    ) -> Result<(), String> {
        let own = format!("{}_", app.replace('-', "_"));
        let (owner, prefix) = match fronts {
            Some(pkg) if !name.starts_with(&own) => (pkg, format!("{}_", pkg.replace('-', "_"))),
            _ => (app, own),
        };
        let function = name
            .strip_prefix(&prefix)
            .ok_or_else(|| format!("{app}'s action {name} does not start {prefix}"))?;
        if agentabi::takes_os_name(owner, function) || is_os_row(name) {
            return Err(format!("{app}'s action {name} is an OS row's name"));
        }
        if let Some(other) = others.iter().find(|other| {
            let theirs = format!("{}_", other.replace('-', "_"));
            theirs.len() > prefix.len() && name.starts_with(&theirs)
        }) {
            return Err(format!("{app}'s action {name} starts with {other}'s name"));
        }
        if let Some(l) = self
            .loaded
            .iter()
            .filter(|l| l.app != app)
            .find(|l| l.rows.iter().any(|r| r.row.name == name))
        {
            return Err(format!("{app}'s action {name} is {}'s", l.app));
        }
        Ok(())
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

    /// The OS rows `app`'s package declares for its effects; none while its
    /// rows are not loaded.
    pub fn effects_of(&self, app: &str) -> &[String] {
        self.loaded
            .iter()
            .find(|l| l.app == app)
            .map_or(&[], |l| l.effects.as_slice())
    }

    /// Whether `app`'s package declares that its effects may call `row`.
    pub fn may_effect(&self, app: &str, row: &str) -> bool {
        self.loaded
            .iter()
            .any(|l| l.app == app && l.effects.iter().any(|e| e == row))
    }

    /// The apps whose rows are loaded, in load order.
    pub fn apps(&self) -> impl Iterator<Item = &str> + '_ {
        self.loaded.iter().map(|l| l.app.as_str())
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
fn convert(app: &str, fronts: Option<&str>, row: &ActionRow) -> Result<AppRow, String> {
    let prefix = format!("{}_", app.replace('-', "_"));
    let function = row
        .name
        .strip_prefix(&prefix)
        .or_else(|| {
            row.name
                .strip_prefix(&format!("{}_", fronts?.replace('-', "_")))
        })
        .unwrap_or(&row.name);
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
                // What a call acts on, or carries, must be in the user's line,
                // whatever the app declares.
                from_request: arg.from_request || !matches!(arg.role, declared::Role::Option),
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
        // The cues are the function's own words, never the app's list: a row
        // cannot claim every line as a request for it.
        cues: leak_strs(&cues_of(function)),
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

/// The words of a row's function name, as its cues: `add` for `notes_add`.
fn cues_of(function: &str) -> Vec<String> {
    function
        .split('_')
        .filter(|word| word.len() > 2)
        .map(String::from)
        .collect()
}

/// Whether `name` is a row of the OS under any of its lists.
pub(crate) fn is_os_row(name: &str) -> bool {
    agentabi::ROWS
        .iter()
        .chain(&agentabi::BOUND)
        .chain(&agentabi::CALL_ONLY)
        .chain(&agentabi::COMMANDS)
        .chain(&agentabi::SCRIPT_API)
        .chain(&agentabi::TYPED_ONLY)
        .chain(&agentabi::FS_ROWS)
        .chain(&agentabi::HELPERS)
        .any(|row| row.name == name)
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
    fn a_window_owns_its_packages_rows_and_only_one_service_does() {
        // Notes' window declares Notes' own rows, `notes_*`.
        let mut apps = AppRows::default();
        assert_eq!(apps.load("notes-window", NOTES), Ok(2));
        let add = apps.find_in("notes-window", "notes_add").unwrap();
        assert_eq!(add.app, "notes-window");
        // Then `notes` cannot load them too, nor the window after `notes`.
        let err = apps.load("notes", NOTES).unwrap_err();
        assert!(err.contains("loaded for its window notes-window"), "{err}");
        let mut apps = AppRows::default();
        apps.load("notes", NOTES).unwrap();
        let err = apps.load("notes-window", NOTES).unwrap_err();
        assert!(err.contains("notes has them loaded"), "{err}");
    }

    #[test]
    fn a_window_fronts_only_the_package_its_world_names() {
        let mut apps = AppRows::default();
        // A window named for notes whose package is another's takes nothing.
        let other = NOTES.replace("app:notes/", "app:files/");
        let err = apps.load("notes-window", &other).unwrap_err();
        assert!(err.contains("does not start notes_window_"), "{err}");
        // A package's world cannot make a service another's window.
        let err = apps.load("files-window", NOTES).unwrap_err();
        assert!(err.contains("does not start files_window_"), "{err}");
        assert_eq!(
            fronts("notes-window", "app:notes/notes@1.0.0"),
            Some("notes")
        );
        assert_eq!(fronts("notes", "app:notes/notes@1.0.0"), None);
        assert_eq!(fronts("notes-window", "app:notes-x/n@1"), None);
        // No name, or one no package could have, fronts nothing.
        assert_eq!(fronts("-window", "app:/x"), None);
        assert_eq!(fronts("Notes-window", "app:Notes/x"), None);
        assert_eq!(fronts("no_tes-window", "app:no_tes/x"), None);
    }

    #[test]
    fn no_package_may_declare_effects_that_type_shell_lines() {
        assert!(!declarable("shell_line"));
        let greedy = NOTES.replace(
            "world app:notes/notes@1.0.0\n",
            "world app:notes/notes@1.0.0\neffects shell_line\n",
        );
        let err = AppRows::default().load("notes", &greedy).unwrap_err();
        assert!(err.contains("shell_line, which no app may"), "{err}");
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
