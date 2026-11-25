//! Spike S10-2's eight stub apps: their rows written as an app's build writes
//! `kai-actions 1`, and loaded through the shell's own loader, as
//! `actbench::apps::shipped` loads the image's apps. Pairs share words on
//! purpose (to-do and shopping lists, timers and alarms, calendar and
//! contacts), so the confusable tasks can tell whether the apps turn's
//! index lines are enough to pick the right one.

use llmproto::pack::apps::AppGroup;
use shellcore::apps::AppRows;

/// An argument's kind, as the decoder steers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// An integer from 0 to this.
    Int(u32),
}

/// One stub row: its name, doc, arguments, and whether it only reads.
pub struct Row {
    pub name: &'static str,
    pub doc: &'static str,
    pub args: &'static [(&'static str, Kind)],
    pub read: bool,
}

/// One stub app and its rows.
pub struct Stub {
    pub app: &'static str,
    pub rows: &'static [Row],
}

const fn row(
    name: &'static str,
    doc: &'static str,
    args: &'static [(&'static str, Kind)],
    read: bool,
) -> Row {
    Row {
        name,
        doc,
        args,
        read,
    }
}

const TEXT: Kind = Kind::Text;

/// The eight apps, in the order the apps turn lists them.
pub static STUBS: [Stub; 8] = [
    Stub {
        app: "todo",
        rows: &[
            row("todo_add", "Add a to-do item.", &[("text", TEXT)], false),
            row(
                "todo_done",
                "Mark the to-do item with this number done.",
                &[("item", Kind::Int(999))],
                false,
            ),
            row("todo_list", "The open to-do items, numbered.", &[], true),
        ],
    },
    Stub {
        app: "shopping",
        rows: &[
            row(
                "shopping_add",
                "Add an item to the shopping list.",
                &[("item", TEXT)],
                false,
            ),
            row(
                "shopping_remove",
                "Take an item off the shopping list.",
                &[("item", TEXT)],
                false,
            ),
            row("shopping_list", "The shopping list.", &[], true),
        ],
    },
    Stub {
        app: "timer",
        rows: &[
            row(
                "timer_start",
                "Start a countdown of this many seconds.",
                &[("seconds", Kind::Int(86_400))],
                false,
            ),
            row("timer_cancel", "Cancel the running countdown.", &[], false),
            row("timer_left", "Seconds left on the countdown.", &[], true),
        ],
    },
    Stub {
        app: "alarm",
        rows: &[
            row(
                "alarm_set",
                "Set an alarm for a clock time, like 07:30.",
                &[("time", TEXT)],
                false,
            ),
            row(
                "alarm_clear",
                "Remove the alarm set for a clock time.",
                &[("time", TEXT)],
                false,
            ),
            row("alarm_list", "The alarms that are set.", &[], true),
        ],
    },
    Stub {
        app: "weather",
        rows: &[
            row(
                "weather_now",
                "The weather in a city now.",
                &[("city", TEXT)],
                true,
            ),
            row(
                "weather_week",
                "The coming week's forecast for a city.",
                &[("city", TEXT)],
                true,
            ),
        ],
    },
    Stub {
        app: "music",
        rows: &[
            row(
                "music_play",
                "Play a song, an artist or a style.",
                &[("song", TEXT)],
                false,
            ),
            row("music_pause", "Pause the music.", &[], false),
            row("music_skip", "Skip to the next song.", &[], false),
            row(
                "music_volume",
                "Set the music's volume, from 0 to 100.",
                &[("level", Kind::Int(100))],
                false,
            ),
        ],
    },
    Stub {
        app: "contacts",
        rows: &[
            row(
                "contacts_find",
                "A contact's phone number, by name.",
                &[("name", TEXT)],
                true,
            ),
            row(
                "contacts_add",
                "Add a contact with a phone number.",
                &[("name", TEXT), ("phone", TEXT)],
                false,
            ),
        ],
    },
    Stub {
        app: "calendar",
        rows: &[
            row(
                "calendar_add",
                "Add an event on a day.",
                &[("title", TEXT), ("day", TEXT)],
                false,
            ),
            row(
                "calendar_day",
                "The events on a day, like today or 2026-12-01.",
                &[("day", TEXT)],
                true,
            ),
        ],
    },
];

/// `stub`'s `kai-actions 1` answer, as the shell asks an app for it.
pub fn answer(stub: &Stub) -> String {
    let app = stub.app;
    let mut text = format!("version 1.0.0\nkai-actions 1\nworld app:{app}/{app}@1.0.0\n");
    for row in stub.rows {
        text.push_str(&action(app, row));
    }
    text
}

/// One `action` ... `end` block.
fn action(app: &str, row: &Row) -> String {
    let export = row
        .name
        .strip_prefix(&format!("{app}_"))
        .unwrap_or(row.name)
        .replace('_', "-");
    let params: Vec<String> = row
        .args
        .iter()
        .map(|(name, kind)| match kind {
            Kind::Text => format!("{name}: string"),
            Kind::Int(_) => format!("{name}: u32"),
        })
        .collect();
    let mut text = format!(
        "action {}\nexport app:{app}/actions@1.0.0#{export}\ndoc {}\nsig func({}) -> result<string, string>\n",
        row.name,
        row.doc,
        params.join(", "),
    );
    for (name, kind) in row.args {
        text.push_str(&match kind {
            Kind::Text => format!("arg {name} text max=80 role=payload\n"),
            Kind::Int(max) => format!("arg {name} int min=0 max={max} role=option\n"),
        });
    }
    let facts = match row.read {
        true => "read-only=yes destructive=no idempotent=yes open-world=no",
        false => "read-only=no destructive=no idempotent=no open-world=no",
    };
    text.push_str(&format!("facts {facts}\ninverse none\nend\n"));
    text
}

/// The eight apps' groups, loaded as the shell loads an app's rows.
pub fn groups() -> Result<Vec<AppGroup>, String> {
    let names: Vec<String> = STUBS.iter().map(|s| s.app.to_string()).collect();
    let mut rows = AppRows::default();
    let mut groups = Vec::new();
    for stub in &STUBS {
        let others: Vec<String> = names.iter().filter(|n| *n != stub.app).cloned().collect();
        rows.load_among(stub.app, &answer(stub), &others)
            .map_err(|why| format!("stub app {}: {why}", stub.app))?;
        groups.push(AppGroup {
            app: stub.app.into(),
            rows: rows.rows_of(stub.app),
        });
    }
    Ok(groups)
}

/// The stub row `name`, if one app has it.
#[cfg(test)]
pub fn find(name: &str) -> Option<(&'static Stub, &'static Row)> {
    STUBS
        .iter()
        .find_map(|stub| stub.rows.iter().find(|r| r.name == name).map(|r| (stub, r)))
}
