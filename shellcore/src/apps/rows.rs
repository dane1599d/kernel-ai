//! An app's rows as the shell holds them (`AppRows`): read from its
//! `kai-actions 1` answer, checked, and made registry rows.

#[allow(unused_imports)]
use super::*;

/// Every app's rows, the newest version of each.
#[derive(Default)]
pub struct AppRows {
    pub(crate) loaded: Vec<Loaded>,
    pub(crate) loads: usize,
    /// Apps whose rows did not load: not asked again until reboot.
    pub(crate) failed: Vec<String>,
    /// (app, pid) of each running instance [`wire`] asked for its rows.
    pub(crate) asked: Vec<(String, u64)>,
    /// One entry per time `pkg` gave no answer for an installed app's rows.
    pub(crate) unanswered: Vec<String>,
    /// (service, pid) of each running instance that answered "no such app"
    /// to [`is_app`].
    pub(crate) not_apps: Vec<(String, u64)>,
    /// (service, pid, when) of each running instance whose answer to
    /// [`is_app`] was another error, for [`SLOW_FOR_NS`].
    pub(crate) slow: Vec<(String, u64, u64)>,
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

    /// The pid of `app`'s instance [`wire`] last asked for its rows.
    pub(crate) fn asked(&self, app: &str) -> Option<u64> {
        self.asked
            .iter()
            .rev()
            .find(|(name, _)| name == app)
            .map(|(_, pid)| *pid)
    }

    /// The `kai-actions 1` answer `app`'s loaded rows came from.
    pub fn answer(&self, app: &str) -> Option<&str> {
        self.loaded
            .iter()
            .find(|l| l.app == app)
            .map(|l| l.answer.as_str())
    }

    /// Whether the instance `pid` of `service` said it is no app.
    pub fn not_app(&self, service: &str, pid: u64) -> bool {
        self.not_apps.iter().any(|(s, p)| s == service && *p == pid)
    }

    /// Whether any instance of `service` said it is no app since boot.
    pub fn said_not_app(&self, service: &str) -> bool {
        self.not_apps.iter().any(|(s, _)| s == service)
    }

    /// Whether the instance `pid` of `service` answered [`is_app`] with
    /// another error less than [`SLOW_FOR_NS`] before `now`.
    pub fn slow(&self, service: &str, pid: u64, now: u64) -> bool {
        self.slow
            .iter()
            .any(|(s, p, at)| s == service && *p == pid && now.saturating_sub(*at) < SLOW_FOR_NS)
    }

    /// Notes that the instance `pid` of `service` answered slowly at `now`.
    pub(crate) fn slow_since(&mut self, service: &str, pid: u64, now: u64) {
        self.slow.retain(|(s, p, at)| {
            !(s == service && *p == pid) && now.saturating_sub(*at) < SLOW_FOR_NS
        });
        if self.slow.len() >= MAX_SLOW {
            self.slow.remove(0);
        }
        self.slow.push((service.to_string(), pid, now));
    }

    /// Whether `app`'s rows failed to load since boot.
    pub fn failed(&self, app: &str) -> bool {
        self.failed.iter().any(|f| f == app)
    }

    pub(crate) fn try_load(
        &mut self,
        app: &str,
        answer: &str,
        others: &[String],
    ) -> Result<usize, String> {
        let (head, section) = answer
            .split_once('\n')
            .ok_or_else(|| format!("{app}'s rows are not a kai-actions answer"))?;
        let version = head
            .strip_prefix("version ")
            .ok_or_else(|| format!("{app}'s rows: {head}"))?;
        // The same version, or the same rows under another label: `pkg` labels
        // an installed app by its package version and comphost by its store
        // label, so the app's first run must not load its rows twice.
        let same = |l: &Loaded| {
            l.version == version || l.answer.split_once('\n').is_some_and(|(_, s)| s == section)
        };
        if let Some(loaded) = self.loaded.iter().find(|l| l.app == app && same(l)) {
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
        // holds none, and the reverse: one owner for a prefix. A window that
        // declares only its own `<app>_*` rows fronts nothing.
        let own = format!("{}_", app.replace('-', "_"));
        let fronts = fronts(app, &parsed.world)
            .filter(|_| parsed.rows.iter().any(|row| !row.name.starts_with(&own)));
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
            fronted_export(app, fronts, row)?;
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
            answer: answer.to_string(),
            effects: parsed.effects.clone(),
            fronts: fronts.map(str::to_string),
        });
        Ok(count)
    }

    /// Why `name` may not be one of `app`'s rows: no `<app>_` start (or, for a
    /// window, its package's), an OS row's name, a longer service name's
    /// start, or another loaded app's row.
    pub(crate) fn name_free(
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

    /// App `app`'s loaded rows, in the order it declares them.
    pub fn rows_of(&self, app: &str) -> Vec<&'static Action> {
        self.loaded
            .iter()
            .filter(|l| l.app == app)
            .flat_map(|l| l.rows.iter().map(|r| r.row))
            .collect()
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

pub(super) fn leak<T>(value: T) -> &'static T {
    Box::leak(Box::new(value))
}

pub(super) fn leak_str(text: &str) -> &'static str {
    Box::leak(text.to_string().into_boxed_str())
}

pub(super) fn leak_strs(texts: &[String]) -> &'static [&'static str] {
    Box::leak(
        texts
            .iter()
            .map(|t| leak_str(t))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    )
}

/// One declared row as an ADR 0002 row, checked as the OS's rows are.
pub(super) fn convert(app: &str, fronts: Option<&str>, row: &ActionRow) -> Result<AppRow, String> {
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

/// Words that ask for the same thing as a row's function word: a person says
/// "delete note 2" to a `remove`, "show my notes" to a `list`. The OS owns the
/// list, so an app cannot widen what its row is a request for (ADR 0002 D6).
/// Each group is one meaning; a word in it brings the others.
pub(super) const SAME_ASK: [&[&str]; 4] = [
    &["remove", "delete", "erase"],
    &["list", "show"],
    &["edit", "change", "replace", "rewrite"],
    &["append", "add"],
];

/// The words of a row's function name, as its cues: `add` for `notes_add`,
/// and, for `remove`, `delete` and `erase` too ([`SAME_ASK`]).
pub(super) fn cues_of(function: &str) -> Vec<String> {
    let mut cues: Vec<String> = Vec::new();
    for word in function.split('_').filter(|word| word.len() > 2) {
        let group = SAME_ASK.iter().find(|group| group.contains(&word));
        for cue in group.map_or(&[word][..], |group| *group) {
            if !cues.iter().any(|have| have == cue) {
                cues.push(String::from(*cue));
            }
        }
    }
    cues
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
pub(super) fn arg_type(
    kind: &ArgKind,
    ty: &Ty,
    min: Option<i64>,
    max: Option<i64>,
) -> Option<ArgType> {
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
