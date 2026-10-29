//! S, storage (ADR 0004, P2): block writes and flushes on the `kai-data` disk,
//! and S04-3's raw case, replayed from QEMU's blklogwrites log.

use super::{TestCase, case};
use crate::blklog::{Data, Expect};
use crate::disk::Fixture;

const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};
const NVME: Data = Data {
    nvme: true,
    ..VIRTIO
};
/// S04-3's commits fill a raw disk.
const RAW: Data = Data {
    gpt: false,
    ..VIRTIO
};

/// Boot 2 of `blk-write`: what boot 1 wrote and flushed reads back.
const REREAD: TestCase = TestCase {
    expect: &["blk-write: boot 1's writes read back"],
    server: false,
    data: Some(VIRTIO),
    ..case("blk-write (boot 2)", "test=blk-write reread")
};
const REREAD_NVME: TestCase = TestCase {
    data: Some(NVME),
    ..REREAD
};

/// Boot 2 of `fs-reboot`: what boot 1 wrote through `fs` reads back.
const FS_REBOOT_2: TestCase = TestCase {
    expect: &[
        "filecheck: boot 2 read back what boot 1 wrote, byte for byte",
        "files: boot 2: filecheck's step through its Dir handle passed",
    ],
    server: false,
    data: Some(VIRTIO),
    ..case("fs-reboot (boot 2)", "test=files reread")
};

/// Boot 2 of `app-saved`: the app starts from its saved state.
const APP_SAVED_2: TestCase = TestCase {
    expect: &[
        "live: notes-window 1 restored from its saved state",
        "app-saved: boot 2 found boot 1's note, restored from the app's saved state",
    ],
    server: false,
    data: Some(VIRTIO),
    ..case("app-saved (boot 2)", "test=app-saved reread")
};

/// Boot 2 of `fs-undo-reboot`: the log on disk still undoes boot 1's change.
const FS_UNDO_REBOOT_2: TestCase = TestCase {
    expect: &["fs-undo-reboot: boot 2's undo restored notes.txt from the log on disk"],
    data: Some(VIRTIO),
    ..case("fs-undo-reboot (boot 2)", "test=fs-undo-reboot reread")
};

pub(super) const CASES: &[TestCase] = &[
    TestCase {
        expect: &[
            "block: kai-data is ",
            "block: kai-cache is ",
            "blk-write: kai-data over blk takes writes and a flush",
        ],
        server: false,
        data: Some(VIRTIO),
        then: Some(&REREAD),
        ..case("blk-write", "test=blk-write")
    },
    TestCase {
        expect: &[
            "blk-write: kai-data over nvme takes writes and a flush",
            ", volatile write cache, AWUPF",
        ],
        server: false,
        data: Some(NVME),
        then: Some(&REREAD_NVME),
        ..case("blk-write-nvme", "test=blk-write")
    },
    // P2 task 3: `fs` alone holds a Partition, for kai-data only; S04-2 re-run on
    // the real calls.
    TestCase {
        expect: &[
            "fs: up; kai-data has ",
            "part: fs alone holds a Partition, for kai-data only, and its checks pass",
            "part: S04-2 on the real calls: ",
        ],
        server: false,
        data: Some(VIRTIO),
        ..case("part", "test=part")
    },
    TestCase {
        expect: &[
            "part: fs alone holds a Partition, for kai-data only, and its checks pass",
            "part: S04-2 on the real calls: ",
        ],
        server: false,
        data: Some(NVME),
        ..case("part-nvme", "test=part")
    },
    // P2 task 8: files through a `Dir` handle survive a reboot, and `fs` killed
    // mid-action restarts with nothing half done and its handles working.
    TestCase {
        expect: &[
            "fs: up; kai-data has ",
            "filecheck: boot 1 wrote notes/plan.txt and a 20 KiB file through its Dir handle",
            "files: boot 1: filecheck's step through its Dir handle passed",
            "files: ENDPOINT_MINT, the FILES ceiling and SYSTEM_STATE refuse what they must",
        ],
        server: false,
        data: Some(VIRTIO),
        then: Some(&FS_REBOOT_2),
        ..case("fs-reboot", "test=files")
    },
    // P4: an app's state survives a reboot through its own folder (dir=apps).
    TestCase {
        expect: &["app-saved: boot 1 added a note"],
        server: false,
        data: Some(VIRTIO),
        then: Some(&APP_SAVED_2),
        ..case("app-saved", "test=app-saved")
    },
    TestCase {
        expect: &[
            "filecheck: the File handle opened before fs restarted reads what was committed",
            "files: fs restarted mid-action: nothing half-done is on disk, and an open File handle works again",
        ],
        server: false,
        data: Some(VIRTIO),
        ..case("fs-restart", "test=files-restart")
    },
    // A client killed between Begin and Commit: fs aborts its action rather
    // than holding every later change at Busy (PART_PEER_ALIVE).
    TestCase {
        expect: &[
            "filecheck: holding an action with half.txt in it",
            "files: a client killed mid-action leaves nothing, and the next Begin goes through",
        ],
        server: false,
        data: Some(VIRTIO),
        ..case("fs-dead-client", "test=files-dead-client")
    },
    // P2 task 9: 200 file actions through `fs` behind blklogwrites; every crash
    // state the log allows mounts, checks and keeps the last durable commit.
    TestCase {
        expect: &[
            "filecheck: 200 file actions committed; commit p50 ",
            " files whole after the actions",
            "files: 200 file actions are on the disk; the host replays the log",
        ],
        server: false,
        data: Some(Data {
            log: Some(Expect::Clean),
            ..VIRTIO
        }),
        ..case("fs-actions", "test=files-actions")
    },
    // P2 task 9: 20 power cuts while `filecheck` makes file actions, each boot
    // first finding every file whole; the 21st boot ends the case.
    TestCase {
        expect: &["files: boot 21: every file is whole after the cut"],
        server: false,
        data: Some(VIRTIO),
        cuts: 20,
        min_timeout: Some(std::time::Duration::from_secs(900)),
        ..case("fs-powercut", "test=files-churn")
    },
    // P2 task 11: file rows through the one door, with the mock model.
    TestCase {
        expect: &[
            "fs-act: a plain create and move ran at once on kai-data, and a read showed only to the user",
        ],
        data: Some(VIRTIO),
        ..case("fs-act", "test=fs-act")
    },
    TestCase {
        expect: &["fs-undo: undo restored the file's transaction, and undo again restored it back"],
        data: Some(VIRTIO),
        ..case("fs-undo", "test=fs-undo")
    },
    TestCase {
        expect: &["fs-trash: trash and restore ran with undo, and emptying the trash asked first"],
        data: Some(VIRTIO),
        ..case("fs-trash", "test=fs-trash")
    },
    // P2 task 12: the change and its log line in one commit, undone after a reboot.
    TestCase {
        expect: &["fs-undo-reboot: boot 1 created notes.txt, its log line in the same commit"],
        data: Some(VIRTIO),
        then: Some(&FS_UNDO_REBOOT_2),
        ..case("fs-undo-reboot", "test=fs-undo-reboot")
    },
    // S04-3 raw: 200 commits shaped like kaifs's, every flush point replayed.
    TestCase {
        expect: &["blk-flush: 200 commits durable"],
        server: false,
        data: Some(Data {
            log: Some(Expect::Clean),
            ..RAW
        }),
        ..case("blk-flush", "test=blk-flush")
    },
    // The harness must see a missing flush: the guest skips the first flush of
    // every commit, and the replay must find violations.
    TestCase {
        expect: &["blk-flush: 200 commits durable"],
        server: false,
        data: Some(Data {
            log: Some(Expect::Caught),
            ..RAW
        }),
        ..case("blk-flush-bug", "test=blk-flush blkbug=noflush")
    },
    // P2 task 10: no kai-model disk; the model is in kai-data's store, and fs
    // fills the runs the boot reserved from kai-modelhint 1.
    TestCase {
        expect: &[
            "model: 2101245 bytes reserved for the model store's model; fs fills them",
            "model-store: fs filled 2101245 bytes from /system/models",
            "[llm] model: waiting for the model store to fill the model",
            "[llm] model: the model store filled the model",
            "model-store: llm waited for the fill and maps the store's model",
        ],
        server: false,
        data: Some(VIRTIO),
        store: Some(Fixture::Gguf),
        ..case("model-store", "test=model-store")
    },
    // The same store with the hint's FNV-1a64 changed: what fs fills is refused.
    TestCase {
        expect: &[
            "model: the model store's file is refused: its FNV-1a64 is not the hint's",
            "model-store: the store's blob is refused",
        ],
        server: false,
        data: Some(VIRTIO),
        store: Some(Fixture::Gguf),
        store_bad_fnv: true,
        ..case("model-store-bad", "test=model-store bad")
    },
];
