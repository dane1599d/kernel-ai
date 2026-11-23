//! K's desktop cases: the desk, its panel, windows, naps, cards, profiles and the agent.

use super::{TestCase, case};

pub(in crate::cases) const CASES: &[TestCase] = &[
    // P3 task 6's exit case: the OS boots to the desktop, and its screen is the
    // golden one deskcore::paint's host test paints.
    TestCase {
        keys: &[crate::screen::KEYS],
        expect: &[
            "desktop: drew the desk at 1280x800",
            "desk-boot: desktop booted to the desk and the screendump was taken",
        ],
        screen: Some(crate::screen::Expect {
            size: Some((1280, 800)),
            cursor: false,
            golden: Some(crate::screen::Golden {
                sha256: "e2d10823399090a959e01d43604e45e6603c6b55eb5054e67a29095241a917a8",
                mask: desk_stats,
            }),
        }),
        ..case("desk-boot", "test=desk-boot")
    },
    // P3 task 7: Ctrl+Space focuses the AI panel, and Enter queues its line.
    TestCase {
        expect: &[
            "desktop: focus Panel",
            "desktop: the panel queued a request (2 characters)",
            "desk-panel: typed Ctrl+Space, \"hi\" and Enter",
        ],
        ..case("desk-panel", "test=desk-panel")
    },
    // Kai Desk PR 9: "apps" opens Start's All apps (the image's own apps),
    // and one wheel detent over it moves its list a row.
    TestCase {
        expect: &[
            "desktop: focus Panel",
            "desktop: Start shows all apps",
            "desktop: the wheel moved Start's list",
            "desk-wheel: opened All apps and turned the wheel over Start",
        ],
        ..case("desk-wheel", "test=desk-wheel")
    },
    // P3 task 7: the shell takes the panel's line from `desktop` and reports
    // its result back.
    TestCase {
        expect: &[
            "desktop: the panel queued a request (2 characters)",
            "desktop: the shell took the panel's request",
            "ms after Enter (within 100 ms)",
            "desktop: the panel shows ",
            "desk-panel-shell: typed \"hi\" in the panel",
        ],
        ..case("desk-panel-shell", "test=desk-panel-shell")
    },
    // Kai builds apps, follow-up 2: the prompt wakes for the shell's waiting
    // card, holds it while another ask is up, then asks the shell for it.
    TestCase {
        expect: &[
            "card: the terminal asked the shell for its waiting card",
            "desk-card-wake: the prompt woke for the shell's card, waited while another ask was up, then asked the shell",
        ],
        ..case("desk-card-wake", "test=desk-card-wake")
    },
    // P3 task 9: an app's kai:ui window on the desktop, closed from its control.
    TestCase {
        expect: &[
            "comphost: window 1 \"Notes\" shown at 640x420",
            "desktop: window 1 \"Notes\" opened at 48,52 size 642x454",
            "comphost: window 1 closed",
            "desktop: window 1 closed",
            "ui-window: Notes opened its window and pressed its close control",
        ],
        ..case("ui-window", "test=ui-window")
    },
    // P3 task 12b: an unused app naps, and a click wakes it with its state.
    TestCase {
        expect: &[
            "comphost: notes-window napped, unused",
            "comphost: woke from its nap: ok resumed notes-window 1",
            "desk-nap: Notes napped unused, gave its memory back, and a click woke it with its note",
        ],
        ..case("desk-nap", "test=desk-nap")
    },
    // P3 task 7: asks answered on desktop's card, and on the console when it dies.
    TestCase {
        expect: &[
            "ask: 1 went to desktop's card",
            "desktop: ask 1 shows on the card",
            "desktop: ask 1 answered yes",
            "desk-card: an ask went to desktop's card, and y on it allowed",
            "desktop: ask 2 answered no",
            "desk-card: a letter and Enter did not answer the card, and n denied",
            "ask: 3 came back from the card; the console asks it",
            "desk-card: with desktop killed, the ask came back and the console answered it",
        ],
        ..case("desk-card", "test=desk-card")
    },
    // P3 task 11: a standing instruction fired by Notes' window events.
    TestCase {
        expect: &[
            "desk-standing: the rule was allowed on the card, and the shell holds it",
            "desktop: event minimize notes-window",
            "desk-standing: minimizing Notes fired the rule, and Notes is suspended",
            "desktop: event restore notes-window",
            "desk-standing: restoring Notes resumed it, with its note intact",
        ],
        ..case("desk-standing", "test=desk-standing")
    },
    // P3's exit case: no standing instruction from text the AI read (a note the
    // mock model was steered by), and no firing outside the rule allowed.
    TestCase {
        expect: &[
            "desk-standing-refused: the note's instruction made no rule, asked nothing, and its refusal is logged",
            r#"supervisor: refused "service_stop(\"notes-window\")" for pid "#,
            "desk-standing-refused: a firing outside the allowed rule was refused and logged, and nothing asked",
        ],
        ..case("desk-standing-refused", "test=desk-standing-refused")
    },
    // P3's exit case: S05-1's budgets on the real `desktop` at 1920x1080, a gate
    // under KVM, best of two (a scene with a miss is measured once more, and a
    // figure over budget in both runs fails it), unless the reference work says
    // the runner is over 3.5 times its baseline; under TCG nothing is judged.
    TestCase {
        gpu: true,
        memory_mib: Some(2048),
        min_timeout: Some(std::time::Duration::from_secs(1_200)),
        expect: &[
            "desk-perf: reference p50 ",
            "desk-perf: (a) full composite at 1920x1080: ",
            "desk-perf: (b) drag render: ",
            "desk-perf: (b) drag flush: ",
            "desk-perf: (c) text page ",
            "desk-perf: (e) key to flush: ",
            "desk-perf: (f) drag under load, render+flush: ",
            "desk-perf: (g) full composite with Start and the pane open at 1920x1080: ",
            "desk-perf: the figures are in",
        ],
        ..case("desk-perf", "test=desk-perf display=1920x1080")
    },
    // P3's exit case: a profile applied exactly, switched by a real focus change
    // through the user's rule, and undone.
    TestCase {
        expect: &[
            "desk-profile: the focus rule was allowed on the card",
            "desktop: event focus notes-window",
            "desk-profile: focusing Notes switched to gaming: both apps went to the background, their quotas and the system untouched",
            "desk-profile: undo put default back: both apps normal again",
        ],
        ..case("desk-profile", "test=desk-profile")
    },
    // P3's exit case: each resource-profile floor, asked for as the shell asks,
    // refused by the kernel.
    TestCase {
        expect: &[
            "desk-profile-floor: the kernel's reserve: a quota of all ",
            "desk-profile-floor: the model holder: llm was not made background, nor its quota lowered",
            "desk-profile-floor: the supervisor, the shell and desktop were not made background, nor their quotas lowered",
            "desk-profile-floor: free / 2: a raise one page past the admission budget was refused, and one to it was taken",
            "desk-profile-floor: system services' CPU share: a service holding TERM was not made background, and an app was",
            "desk-profile-floor: every floor held, whatever the shell asked",
        ],
        ..case("desk-profile-floor", "test=desk-profile-floor")
    },
    // Key input for uikit apps: the keyboard reaches Notes' widgets.
    TestCase {
        // The window takes the keyboard with its search field focused (#872);
        // Tab to New, Enter, and the new note's editor has the keys.
        keys: &[&["tab", "ret", "h", "i"]],
        expect: &[
            "ui-keys: Tab, Enter and typed text reached Notes' widgets through desktop and comphost",
        ],
        ..case("ui-keys", "test=ui-keys")
    },
    // The same keys while Notes is called every 5 ms, more often than
    // comphost's 20 ms poll: comphost polls its windows on a deadline too.
    TestCase {
        keys: &[&["tab", "ret", "h", "i"]],
        expect: &["ui-keys: busy: the keys reached Notes while it was called every 5 ms"],
        ..case("ui-keys (busy)", "test=ui-keys busy")
    },
    // P3 task 8: the kernel's terminal in a window, and an ask typed there.
    // "switch the scheduler to round robin", which the mock widens to
    // `interactive`, so it asks; a question's added change is refused (#721).
    TestCase {
        keys: &[
            &[
                "s", "w", "i", "t", "c", "h", "spc", "t", "h", "e", "spc", "s", "c", "h", "e", "d",
                "u", "l", "e", "r", "spc", "t", "o", "spc", "r", "o", "u", "n", "d", "spc", "r",
                "o", "b", "i", "n", "ret", "y",
            ],
            &["n"],
            &[
                "w", "h", "a", "t", "spc", "i", "s", "spc", "t", "h", "e", "spc", "s", "c", "h",
                "e", "d", "u", "l", "i", "n", "g", "spc", "p", "o", "l", "i", "c", "y", "ret",
            ],
        ],
        expect: &[
            "term: opened its window",
            "desktop: the terminal window has the keys",
            "term: shows ? ",
            "desk-term: an ask typed in the terminal window was answered only on the console",
            "desktop: the panel queued a request",
            "desk-term: with the focus in the pane the keys went to desktop, and nothing asked",
        ],
        ..case("desk-term", "test=desk-term")
    },
    // P3 task 10: the agent opens Notes through `ui_open`, reads its view and
    // acts in it after a yes on desktop's card, then runs Notes' own
    // `notes_add`, which Notes' window fronts.
    TestCase {
        // `fetch`, in every image since the shell routes to it, left the
        // default 512 MiB too little to open Notes.
        memory_mib: Some(1024),
        keys: &[
            &["o", "p", "e", "n", "spc", "n", "o", "t", "e", "s", "ret"],
            &[
                "s", "e", "a", "r", "c", "h", "spc", "n", "o", "t", "e", "s", "spc", "f", "o", "r",
                "spc", "m", "i", "l", "k", "ret",
            ],
            &["~600", "y", "y"],
            &[
                "a", "d", "d", "spc", "a", "spc", "n", "o", "t", "e", "spc", "s", "a", "y", "i",
                "n", "g", "spc", "b", "u", "y", "spc", "m", "i", "l", "k", "ret",
            ],
        ],
        expect: &[
            "desktop: the agent's ui windows: ok",
            "desk-agent: ui_open started Notes and its window opened",
            "desktop: the agent's ui view: ok",
            "desktop: the agent's ui check: ok",
            "desktop: the agent's ui act: ok",
            "comphost: the agent acted in window ",
            "desk-agent: the agent opened Notes, read its view and typed in it after a yes",
            "desk-agent: Notes' notes_add, loaded from its window, added the note",
        ],
        ..case("desk-agent", "test=desk-agent")
    },
    // P4 task 7: the Settings app runs as the system image plans it, its reads
    // go through comphost and desktop to the shell and back, and its window is
    // kept from the agent: the view shows only that it is private, and an act in
    // it is refused after a yes.
    TestCase {
        // A writable `kai-data` disk, so the shell can save the theme the first
        // line sets and answer Settings' `file appearance` with it.
        data: Some(crate::blklog::Data {
            nvme: false,
            log: None,
            gpt: true,
        }),
        // The file system service, the shell, the desktop, the terminal and
        // Settings (32 MiB of pages each for the last) outgrew the default 512 MiB:
        // the first boot with the data disk could not start settings-window.
        memory_mib: Some(1024),
        keys: &[
            &[
                "s", "w", "i", "t", "c", "h", "spc", "t", "o", "spc", "t", "h", "e", "spc", "l",
                "i", "g", "h", "t", "spc", "t", "h", "e", "m", "e", "ret",
            ],
            // A standing rule, typed as the user's own line: `rules_add on
            // time(22:00) -> app_suspend(game)`, asked on the card, and a yes.
            &[
                "r",
                "u",
                "l",
                "e",
                "s",
                "shift-minus",
                "a",
                "d",
                "d",
                "spc",
                "o",
                "n",
                "spc",
                "t",
                "i",
                "m",
                "e",
                "shift-9",
                "2",
                "2",
                "shift-semicolon",
                "0",
                "0",
                "shift-0",
                "spc",
                "minus",
                "shift-dot",
                "spc",
                "a",
                "p",
                "p",
                "shift-minus",
                "s",
                "u",
                "s",
                "p",
                "e",
                "n",
                "d",
                "shift-9",
                "g",
                "a",
                "m",
                "e",
                "shift-0",
                "ret",
            ],
            &["~600", "y", "y"],
            &[
                "o", "p", "e", "n", "spc", "f", "i", "x", "t", "u", "r", "e", "ret",
            ],
            &[
                "o", "p", "e", "n", "spc", "s", "e", "t", "t", "i", "n", "g", "s", "ret",
            ],
            // A person's keys in the fixture's window and Settings' (the
            // sequences are pinned in `apps/settings-ui/tests/keys.rs`): Alt+Tab
            // raises the fixture, under the Terminal until then, and the
            // fixture's button is pressed; remove the rule on the Rules page;
            // revoke the fixture's row; press (refused); the test types `undo`
            // at the shell, which gives the row back; press (runs); the test
            // types `undo` again, which withholds it; restore the row from
            // Settings; press (runs); then the Theme button, and the Updates
            // page's switch for the restart window.
            &["alt-tab", "tab", "ret"],
            &["tab", "right", "right", "tab", "ret"],
            &["tab", "left", "tab", "tab", "ret"],
            &["tab", "ret"],
            &["tab", "ret"],
            &["tab", "tab", "tab", "ret"],
            &["tab", "ret"],
            &["tab", "left", "tab", "ret"],
            &[
                "tab", "tab", "tab", "tab", "right", "right", "right", "right", "tab", "spc",
            ],
            &[
                "l", "o", "o", "k", "spc", "a", "t", "spc", "s", "e", "t", "t", "i", "n", "g", "s",
                "ret",
            ],
            &["~600", "y", "y"],
        ],
        expect: &[
            "comphost: window 1 \"Settings\" shown at 640x460",
            "\"Settings\" opened at ",
            "comphost: the first settings answer reached the app",
            "app-settings: the theme was set before Settings opened",
            // The agent's theme taken back with `undo` (the test checks the
            // shell's words and that its log lists the undo), so Settings, which
            // reads it when it starts, shows the theme before.
            "app-settings: undo put the theme back to dark and the log lists it",
            "settings: appearance theme dark",
            "app-settings: a standing rule was added before Settings opened",
            // The rule Settings lists, and gone once it is removed by key and
            // the rules are read again.
            "settings: rules 1",
            "settings: rules 0",
            "app-settings: ui_open started the fixture and its window opened",
            "app-settings: keys were pressed in Settings' window",
            // The revoke, held end to end: the fixture's `fs_list` runs, is
            // refused after Settings revokes the row (and Settings reads back
            // that the shell now holds it withheld), runs again when `undo`
            // gives the row back (typed by the test at the shell, which checks
            // its words, and again to withhold it), and runs once more when the
            // restore click gives it back.
            "fixture: fs_list ok #1",
            "settings: grants fixture-window 1 rows, 1 withheld",
            "fixture: fs_list refused #2",
            "fixture: fs_list ok #3",
            "app-settings: undo gave the revoked row back and undo again withheld it",
            "fixture: fs_list ok #4",
            // The theme the key press sets, from the theme before (`dark`), read
            // back from the shell, and the restart window set from the page and
            // read back through `file updates`.
            "settings: appearance theme light",
            "settings: restart window 02:00-05:00",
            // What each other page was given, from the app's log (its window is
            // private to the agent): the rules, the apps with rows, About, the
            // profiles, the apps that can nap, the keep-running list and the
            // restart window.
            "settings: rules ",
            "settings: grants ",
            "settings: about ",
            "settings: profiles ",
            "settings: apps ",
            "settings: keep running ",
            "settings: restart window off",
            "app-settings: ui_open started Settings and its window opened",
            "desktop: the agent's ui view: ok (private window)",
            "desktop: the agent's ui act: refused (private window)",
            "app-settings: the agent looked at Settings, saw only that it is private, and could not act in it",
        ],
        ..case("app-settings", "test=app-settings")
    },
    // The founder's test 2, item 7: the shell answers an app's reads while the
    // model thinks, and holds its changes until the line ends, given back to
    // `desktop` so a shell killed meanwhile loses none (#890)
    // (`kernel/src/wait_reads_tests.rs`). The test app `wait-window`'s Read
    // and Make buttons queue a read and a change as a person's effects; the
    // mock model takes 8 s over one line's call and fails another's after 6 s.
    TestCase {
        // The shell answers the app's `fs_list` and `fs_create` from
        // `/home/user`, on a writable `kai-data` disk.
        data: Some(crate::blklog::Data {
            nvme: false,
            log: None,
            gpt: true,
        }),
        // The file system service, the shell, the desktop and a comphost app,
        // as in `app-settings`.
        memory_mib: Some(1024),
        // Tab goes round the app's two buttons, Read first. With no line:
        // Read. During the slow line: Read; then Make and Read behind it.
        // During the failing line: Read; then Make. During the slow line
        // again, the shell killed after: Make.
        keys: &[
            &["tab", "ret"],
            &["ret"],
            &["tab", "ret", "tab", "ret"],
            &["ret"],
            &["tab", "ret"],
            &["ret"],
        ],
        expect: &[
            "shell: answered wait-window's read while the model thought",
            "shell: gave wait-window's request back to desktop until the line ends",
            "shell: runs wait-window's held request after the line",
            "wait-reads: with no line running, a read was answered in ",
            "wait-reads: while the model thought, the app's read was answered in ",
            "wait-reads: the app's change and the read behind it waited for the line, then ran once each, in order",
            "wait-reads: a model call that failed partway ended its line; the read during it took ",
            "wait-reads: the shell was killed with a change given back to desktop; the restarted shell ran it once",
            "wait-reads: the shell answered the app's reads while the model thought, and held its changes until the line ended",
        ],
        ..case("wait-reads", "test=wait-reads")
    },
    // P3 task 10: a key in the window the agent drives stops it, and the panel
    // says so.
    TestCase {
        // `fetch`, in every image since the shell routes to it, left the
        // default 512 MiB too little to open Notes.
        memory_mib: Some(1024),
        keys: &[
            &["o", "p", "e", "n", "spc", "n", "o", "t", "e", "s", "ret"],
            &[
                "s", "w", "i", "t", "c", "h", "spc", "t", "o", "spc", "n", "o", "t", "e", "s",
                "ret",
            ],
            &["a"],
        ],
        expect: &[
            "desk-takeover: ui_open started Notes and its window opened",
            "desktop: the agent's ui focus: ok",
            "desktop: the user took over w",
            "the panel says: Stopped: you took over",
            "desk-takeover: a key in the window the agent drove stopped it",
        ],
        ..case("desk-takeover", "test=desk-takeover")
    },
];

/// What changes while the desk is still (the tray's chip, memory and
/// clock): deskgeom's tray mask, which deskcore paints them in.
fn desk_stats(width: u32, height: u32) -> (u32, u32, u32, u32) {
    let px = |n: u32| i32::try_from(n).unwrap_or(i32::MAX);
    let r = deskgeom::points(px(width), px(height)).tray_mask();
    let u = |n: i32| u32::try_from(n).unwrap_or(0);
    (u(r.x), u(r.y), u(r.w), u(r.h))
}
