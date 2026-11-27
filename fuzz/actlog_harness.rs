//! The `actlog` fuzz target's body (ADR 0002 D7, ADR 0013 §1; ADR 0000 P8):
//! the action log as the shell reads it back from `/system/log` at a start,
//! through `shellcore::actlog::parse_setting_aside` and `parse`, then the
//! log's own readers `undo` and `log` use. `fuzz_targets/actlog.rs` calls
//! [`actlog`] with libFuzzer's input, and `shellcore/tests/actlog_hostile.rs`
//! includes this file and runs it over hostile mutations of the samples here.
//! It uses `shellcore::actlog` only. It must return for any input; only a bug
//! in the code under it may panic.

#![allow(dead_code)]

use shellcore::actlog::{self, ActLog, KEEP, Record, parse, parse_setting_aside};

/// A person's own log: a call with its texts and change, its undo with its
/// own change, and a system event.
pub const V1: &str = "actlog 1\n\
E 17 t=1792618800123456789 reg=1 pack=- form=call tier=run consent=asked state=done\n\
R 21\nswitch to round robin\n\
O 3\nran\n\
C 26\nset_policy(\"round-robin\");\n\
D 17.1 set_policy owner=kernel res=policy before=interactive after=round-robin inv=spec:1:0:2:0\n\
U 18 of=17 t=1792618860000000000 state=done\n\
D 18.1 set_policy owner=kernel res=policy before=round-robin after=interactive inv=spec:2:0:1:0\n\
S 19 t=1792618860000000000 owner=kernel 33\nnvme0 reset after 5 s timeout, ok\n";

/// A log with an agent's entries: a script with a pack id and two changes,
/// one with an inverse this shell cannot read, a text over several lines,
/// and an entry of the person's own.
pub const V2: &str = "actlog 2\n\
E 3 t=1792618800000000000 reg=1 pack=00000000deadbeef form=script tier=ask consent=allowed state=done agent=goals\n\
R 11\ntidy\nnotes/\n\
D 3.1 fs_move owner=fs res=/home/user/notes/a before=/home/user/notes/a after=/home/user/old/a inv=fs:12\n\
D 3.2 pkg_remove owner=pkg res=app_calc before=1.0 after=- inv=future:7\n\
E 4 t=1792618900000000000 reg=1 pack=- form=words tier=- consent=- state=nothing agent=kai\n\
O 7\nnothing\n";

/// The target: the input as a log read back from disk.
pub fn actlog(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let (records, aside) = match parse_setting_aside(text) {
        Ok(read) => read,
        Err(e) => {
            // A refusal names a line of the text, and `parse` refuses it too.
            assert!(e.line <= text.matches('\n').count() + 1, "{e}");
            assert!(!e.why.is_empty());
            assert_eq!(parse(text), Err(e));
            return;
        }
    };
    assert_eq!(parse(text).as_ref(), Ok(&records));
    // `actlog::text`'s MAX_RECORDS.
    assert!(records.len() <= 4 * KEEP);
    // A version 1 log sets nothing aside, and nothing set aside is kept.
    if text.starts_with("actlog 1\n") {
        assert!(aside.is_empty());
    }
    for record in &records {
        if let Record::Entry(e) = record {
            assert!(e.of.is_some() || !aside.contains(&e.id), "#{} kept", e.id);
            for text in [&e.request, &e.outcome, &e.script] {
                assert!(text.len() <= actlog::MAX_TEXT);
            }
        }
    }
    rewrite(&records);
    let ids: Vec<u64> = records
        .iter()
        .map(|r| match r {
            Record::Entry(e) => e.id,
            Record::System(s) => s.id,
        })
        .collect();
    read_as_undo_does(&ActLog::from_records(records), &ids);
}

/// The log written again, as the shell writes it after a start, reads back as
/// the newest [`KEEP`] records it was made of, and is a fixed point: written
/// once more it gives the same bytes. (The text the input was need not be:
/// the writer folds `=` in a value to `_` and writes texts in one order.)
fn rewrite(records: &[Record]) {
    let kept = &records[records.len().saturating_sub(KEEP)..];
    let first = ActLog::from_records(records.to_vec()).text();
    let (again, aside) = parse_setting_aside(&first).expect("a written log reads");
    assert!(aside.is_empty(), "a written log set {aside:?} aside");
    assert_eq!(again.len(), kept.len());
    if !kept.iter().any(folded) {
        assert_eq!(again, kept, "a log read is not as written");
    }
    let second = ActLog::from_records(again).text();
    assert_eq!(second, first, "a written log is not a fixed point");
}

/// Whether a value in `record` holds a character the writer folds to `_`.
fn folded(record: &Record) -> bool {
    let bent = |s: &str| s.contains('=');
    match record {
        Record::Entry(e) => {
            bent(&e.consent)
                || e.changes.iter().any(|c| {
                    bent(&c.action)
                        || bent(&c.owner)
                        || bent(&c.res)
                        || bent(&c.before)
                        || bent(&c.after)
                })
        }
        Record::System(s) => bent(&s.owner),
    }
}

/// What `undo`, `undo N`, `log` and `log N` read of a log from disk: each
/// answers for any log, and what it names is in the log.
fn read_as_undo_does(log: &ActLog, ids: &[u64]) {
    // The log holds the newest KEEP records, so its last id is the highest of
    // theirs, whatever ids the records it dropped had.
    let last = log.last_id();
    let kept = &ids[ids.len().saturating_sub(KEEP)..];
    assert_eq!(kept.iter().copied().max().unwrap_or(0), last);
    for id in [log.newest_live(), log.undo_target()].into_iter().flatten() {
        assert!(id <= last);
        // What `undo` names is what it reverts: the entry `get` finds.
        assert!(log.get(id).is_some_and(|e| e.live()), "#{id} is not live");
    }
    for &id in ids {
        for later in log.later_touching(id) {
            assert!(later > id && log.get(later).is_some_and(|e| e.live()));
        }
        if let Some(script) = log.script(id) {
            assert!(script.starts_with(&format!("script {id}:\n")));
        }
    }
    let _ = log.list();
}
