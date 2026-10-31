//! C, components: live editing, the edit loop and the boot manifest.

use super::{TestCase, case};
use crate::disk::Fixture;
use std::time::Duration;

pub(super) const CASES: &[TestCase] = &[
    case("live", "test=live"),
    TestCase {
        expect: &[
            "edit: the edit service restarted; restored notes e1 with 1 edit",
            "edit: the edit service restarted; restored notes 2 with 1 edit",
            "[edit] built e3 from 2 (commit ",
        ],
        build: true,
        after: Some(crate::buildsvc::verify::after_edit_case),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("edit", "test=edit")
    },
    // P4 task 11: the editor app. The agent's door opens Notes' source at a line in
    // the editor's window; once the case has clicked into it the host types a
    // comment (`// src/lib.rs:6`) a few keys a stage, each stage waited for in the
    // editor's own status (so TCG is as good as KVM), then Ctrl-B, and the edit
    // builds in the background; then the case clicks on that link with Ctrl held,
    // and the editor shows line 6.
    TestCase {
        keys: &[
            &["slash", "slash", "spc", "s", "r"],
            &["c", "slash", "l", "i", "b"],
            &["dot", "r", "s", "shift-semicolon", "6"],
            &["ctrl-b"],
            // Ctrl held for the case's click on the link, which it makes inside the hold.
            &["!sendkey ctrl 8000"],
        ],
        expect: &[
            "app-editor: the agent opened Notes' src/lib.rs at line 1 in the editor",
            "[edit] built e",
            "app-editor: the host's keys were typed in the editor",
            "app-editor: an edit typed in the editor was built in the background",
            "app-editor: a Ctrl-click on a source link in the editor opened its line",
        ],
        build: true,
        min_timeout: Some(Duration::from_secs(300)),
        // The page budget is half the free frames: with `desktop`, Notes, the edit
        // service and a second comphost the 512 MiB base leaves the editor out.
        memory_mib: Some(1024),
        ..case("app-editor", "test=app-editor")
    },
    // P4 task 11, on the default boot: the editor makes a file of the person's own (Ctrl-O
    // and a path that is not there, typed letters, Ctrl-S), replaces it, and after an
    // `undo` typed at the shell reads it again as the first save left it.
    TestCase {
        // A writable `kai-data` disk: the shell's `/home/user` is on it.
        data: Some(crate::blklog::Data {
            nvme: false,
            log: None,
            gpt: true,
        }),
        keys: &[
            // Ctrl-O asks for a path; the first half of /home/user/todo.txt, then the rest
            // and Enter: the path is not there, so a new file opens.
            &[
                "ctrl-o", "slash", "h", "o", "m", "e", "slash", "u", "s", "e", "r",
            ],
            &["slash", "t", "o", "d", "o", "dot", "t", "x", "t", "ret"],
            // Four letters, then Ctrl-S makes the file; one more and Ctrl-S replaces it.
            &["a", "b", "c", "d"],
            &["ctrl-s"],
            &["z"],
            &["ctrl-s"],
            // After `undo`, the same path again: the file is read back.
            &[
                "ctrl-o", "slash", "h", "o", "m", "e", "slash", "u", "s", "e", "r",
            ],
            &["slash", "t", "o", "d", "o", "dot", "t", "x", "t", "ret"],
        ],
        expect: &[
            "comphost: files: read /home/user/todo.txt (person false) failed: missing: /home/user/todo.txt is not there",
            "app-editor-files: Ctrl-O on a path that is not there opened a new file",
            "comphost: files: write /home/user/todo.txt (person true) answered 0 bytes",
            "app-editor-files: Ctrl-S made the file: ",
            "app-editor-files: Ctrl-S replaced the file: ",
            "app-editor-files: undo took the second save back",
            "comphost: files: read /home/user/todo.txt (person false) answered 4 bytes",
            "app-editor-files: the file read again is the first save's",
        ],
        min_timeout: Some(Duration::from_secs(300)),
        // The file system service, the shell, the desktop and the editor (32 MiB of
        // pages for the last) outgrow the default 512 MiB, as for app-settings.
        memory_mib: Some(1024),
        ..case("app-editor-files", "test=app-editor-files")
    },
    TestCase {
        build: true,
        prepare: Some(crate::boot_smoke::prepare_manifest),
        after: Some(crate::boot_smoke::after_manifest),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("manifest", "test=manifest")
    },
    TestCase {
        build: true,
        prepare: Some(crate::boot_smoke::prepare_manifest_good),
        after: Some(crate::boot_smoke::after_manifest_good),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("manifest-good", "test=manifest-good")
    },
    // S03-1's deferred figures: the view rebuild and layout pass in hard-float
    // ring 3, reported in the log.
    TestCase {
        expect: &["appbench: done"],
        server: false,
        ..case("appbench", "test=appbench")
    },
    // The live scenario with the model on the disk and no server: `:why` is
    // answered by the local model.
    TestCase {
        disk: Some(Fixture::LiveLocal),
        server: false,
        expect: &["live: :why notes was answered by the local model"],
        ..case("live-local", "test=live")
    },
];
