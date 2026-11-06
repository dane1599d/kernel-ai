//! C, components: live editing, the edit loop and the boot manifest.

use super::{TestCase, case};
use crate::disk::Fixture;
use std::time::Duration;

/// `app-notes`' disk: Notes' saved state is in `/system/apps` on `kai-data`.
const NOTES_DISK: crate::blklog::Data = crate::blklog::Data {
    nvme: false,
    log: None,
    gpt: true,
};

/// Boot 2 of `app-notes`: Notes starts from its saved state, the one note left.
const APP_NOTES_2: TestCase = TestCase {
    expect: &[
        "live: notes-window 1 restored from its saved state",
        "app-notes: the agent read the window: ok Notes shows: buy milk.",
        "app-notes: boot 2's window shows the note Kai made, from the saved state",
    ],
    data: Some(NOTES_DISK),
    memory_mib: Some(1024),
    min_timeout: Some(Duration::from_secs(300)),
    ..case("app-notes (boot 2)", "test=app-notes reread")
};

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
            // Ctrl-O asks for a path, which is typed with Enter in the same stage so that
            // they arrive in order: /home/user/todo.txt is not there, so a new file opens.
            &[
                "ctrl-o", "slash", "h", "o", "m", "e", "slash", "u", "s", "e", "r", "slash", "t",
                "o", "d", "o", "dot", "t", "x", "t", "ret",
            ],
            // Four letters, then Ctrl-S makes the file; one more and Ctrl-S replaces it.
            &["a", "b", "c", "d"],
            &["ctrl-s"],
            &["z"],
            &["ctrl-s"],
            // After `undo`, the same path again: the file is read back.
            &[
                "ctrl-o", "slash", "h", "o", "m", "e", "slash", "u", "s", "e", "r", "slash", "t",
                "o", "d", "o", "dot", "t", "x", "t", "ret",
            ],
            // One key moves the cursor off the top; the agent's action then reads the file
            // again and the cursor is back.
            &["right"],
        ],
        expect: &[
            "comphost: files: read /home/user/todo.txt (person true) failed: missing: /home/user/todo.txt is not there",
            "app-editor-files: Ctrl-O on a path that is not there opened a new file",
            "comphost: files: write /home/user/todo.txt (person true) answered 0 bytes",
            "app-editor-files: Ctrl-S made the file: ",
            "app-editor-files: Ctrl-S replaced the file: ",
            "app-editor-files: undo took the second save back",
            "comphost: files: read /home/user/todo.txt (person true) answered 4 bytes",
            "app-editor-files: the file read again is the first save's",
            // The agent's `editor_open_file`: a read with no person's key behind it.
            "comphost: files: read /home/user/todo.txt (person false) answered 4 bytes",
            "app-editor-files: the agent's action opened the file by its path",
        ],
        min_timeout: Some(Duration::from_secs(300)),
        // The file system service, the shell, the desktop and the editor (32 MiB of
        // pages for the last) outgrow the default 512 MiB, as for app-settings.
        memory_mib: Some(1024),
        ..case("app-editor-files", "test=app-editor-files")
    },
    // P4's close: Kai makes a note in Notes' window, the window shows it, a second
    // note is taken back by `undo`, and boot 2 finds the one left on disk.
    TestCase {
        expect: &[
            "app-notes: Kai's notes_add ran at once, the text being the person's: ok ",
            "app-notes: the agent read the window: ok Notes shows: buy milk.",
            "app-notes: the Notes window shows the note Kai made",
            "app-notes: undo took the second note back, in Notes and its window",
        ],
        data: Some(NOTES_DISK),
        memory_mib: Some(1024),
        min_timeout: Some(Duration::from_secs(300)),
        then: Some(&APP_NOTES_2),
        ..case("app-notes", "test=app-notes")
    },
    // P4 task 11, C components from the editor: the editor opens a C source the case
    // wrote into `/home/user/c/hello/` (Ctrl-O and its path), and Ctrl-B builds the
    // component through the file door and the shell as a typed `cc build hello` does:
    // `pkg` installs it from `local` as generation 2 and starts it. After `undo`, typed
    // at the shell, packages are back at generation 1. Minutes when QEMU emulates the
    // CPU, as `c-build`'s two builds are.
    TestCase {
        data: Some(crate::blklog::Data {
            nvme: false,
            log: None,
            gpt: true,
        }),
        keys: &[
            // Ctrl-O asks for a path, typed with Enter in the same stage so that they
            // arrive in order: /home/user/c/hello/hello.c, the file the case wrote.
            &[
                "ctrl-o", "slash", "h", "o", "m", "e", "slash", "u", "s", "e", "r", "slash", "c",
                "slash", "h", "e", "l", "l", "o", "slash", "h", "e", "l", "l", "o", "dot", "c",
                "ret",
            ],
            // Ctrl-B builds the component the open file is in.
            &["ctrl-b"],
        ],
        expect: &[
            "pkg: generation 1 starts no app from a package",
            "app-editor-build: Ctrl-O opened /home/user/c/hello/hello.c",
            "comphost: files: build hello (person true) answered ",
            "app-editor-build: Ctrl-B built hello 0.0.1 from the editor, and pkg installed it as generation 2",
            "pkg: started from local: hello 0.0.1 as p-",
            "hello from C, built in the OS",
            "app-editor-build: hello 0.0.1 runs as p-",
            "pkg: switched back from generation 2 to 1's packages as generation 3",
            "pkg: ended, undone: hello as p-",
            "app-editor-build: undo took the editor's build back to generation 1",
        ],
        min_timeout: Some(Duration::from_secs(600)),
        // fs, pkg, the model, the shell, cc, desktop, the editor and the built app
        // together pass the default 512 MiB guest's page budget, as `c-build`'s do.
        memory_mib: Some(1024),
        ..case("app-editor-build", "test=app-editor-build")
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
    // Spike S13-3 (not for merge): the embedding model named by KAI_EMBED_MODEL
    // through infer in hard-float ring 3: indexing tokens/s and a query over
    // 10,000 chunks, reported in the log. Run by s13-3-embed.yml only.
    TestCase {
        expect: &["embed-bench: done"],
        server: false,
        real_model: Some("KAI_EMBED_MODEL"),
        min_timeout: Some(Duration::from_secs(900)),
        ..case("embed-bench", "test=embed-bench")
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
