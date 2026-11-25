//! `kai-modeltrial 1` and the probation a switched model is on.

use modelchan::trial::{
    AtBoot, Checked, Decision, HEALTH_WAIT_MS, HUNG, Health, MAX_WHY, Probation, State, Switch,
    Trial, Write, at_boot, check, decide, health_of, look, next, start, started_again, switch,
    verdict,
};

const OLD: [u8; 32] = [0x11; 32];
const NEW: [u8; 32] = [0x22; 32];
const OTHER: [u8; 32] = [0x33; 32];

/// A model's FNV-1a64, made up for the test.
fn fnv_of(model: [u8; 32]) -> u64 {
    u64::from(model[0]) * 0x0101_0101_0101_0101
}

/// The record a cancel leaves: settled on `model`, which the hint names.
fn settled(model: [u8; 32]) -> Trial {
    Trial {
        candidate: model,
        fnv: fnv_of(model),
        previous: None,
        state: State::Kept,
    }
}

fn pending(candidate: [u8; 32], previous: Option<[u8; 32]>) -> Trial {
    Trial {
        candidate,
        fnv: fnv_of(candidate),
        previous,
        state: State::Pending,
    }
}

#[test]
fn a_record_reads_back_as_written() {
    let records = [
        pending(NEW, Some(OLD)),
        pending(NEW, None),
        Trial {
            state: State::Attempted,
            ..pending(NEW, Some(OLD))
        },
        Trial {
            state: State::Kept,
            ..pending(NEW, Some(OLD))
        },
        Trial {
            state: State::RolledBack("it did not load within 300 s".into()),
            ..pending(NEW, Some(OLD))
        },
    ];
    for record in records {
        assert_eq!(Trial::parse(&record.encode()), Ok(record.clone()));
    }
    assert_eq!(
        pending(NEW, None).encode(),
        format!(
            "kai-modeltrial 1\ncandidate {}\nfnv 2222222222222222\nprevious none\nstate pending\n",
            "22".repeat(32)
        )
    );
}

#[test]
fn a_why_is_kept_to_one_short_line() {
    let long = format!("panic\nin {}", "x".repeat(400));
    let record = Trial {
        state: State::RolledBack(long),
        ..pending(NEW, Some(OLD))
    };
    let back = Trial::parse(&record.encode()).unwrap();
    let State::RolledBack(why) = back.state else {
        panic!("{back:?}")
    };
    assert!(
        why.starts_with("panic in xxx") && why.len() <= MAX_WHY,
        "{why}"
    );
    let blank = Trial {
        state: State::RolledBack("\n".into()),
        ..pending(NEW, Some(OLD))
    };
    let back = Trial::parse(&blank.encode()).unwrap();
    assert_eq!(back.state, State::RolledBack("no reason was given".into()));
}

#[test]
fn a_malformed_record_is_refused() {
    let good = pending(NEW, Some(OLD)).encode();
    let rolled = Trial {
        state: State::RolledBack("x".into()),
        ..pending(NEW, Some(OLD))
    }
    .encode();
    let bad = [
        good.trim_end().to_string(),
        good.replace("kai-modeltrial 1", "kai-modeltrial 2"),
        good.replace(
            &"22".repeat(32),
            &"2".repeat(64).to_uppercase().replace('2', "A"),
        ),
        good.replace(&"11".repeat(32), "11"),
        good.replace("state pending", "state kept-ish"),
        good.replace("candidate ", "candidate  "),
        format!("{good}extra\n"),
        good.replace("\nprevious", "\nprevius"),
        good.replace("fnv 2222222222222222", "fnv 222222222222222G"),
        good.replace("fnv 2222222222222222", "fnv 2222222222222222A"),
        good.replace("fnv 2222222222222222", "fnv 22222222"),
        good.replace("\nfnv 2222222222222222", ""),
        rolled.replace("why x", "why "),
        rolled.replace("why x", &format!("why {}", "y".repeat(MAX_WHY + 1))),
        rolled.replace("\nwhy x", ""),
        String::new(),
    ];
    for text in bad {
        assert!(Trial::parse(&text).is_err(), "{text:?}");
    }
}

#[test]
fn a_switch_tries_another_model_and_undo_cancels_it() {
    // From OLD to NEW: a trial, with OLD to go back to.
    let tried = switch(Some(OLD), None, NEW, fnv_of(NEW));
    assert_eq!(tried, Switch::Try(pending(NEW, Some(OLD))));
    let Switch::Try(record) = tried else {
        unreachable!()
    };
    // Again: nothing more to do. Back to OLD before the restart: cancelled.
    assert_eq!(
        switch(Some(OLD), Some(&record), NEW, fnv_of(NEW)),
        Switch::Nothing
    );
    assert_eq!(
        switch(Some(OLD), Some(&record), OLD, fnv_of(OLD)),
        Switch::Cancel(settled(OLD))
    );
    // A third model replaces the pending one, still going back to OLD.
    assert_eq!(
        switch(Some(OLD), Some(&record), OTHER, fnv_of(OTHER)),
        Switch::Try(pending(OTHER, Some(OLD)))
    );
    // The running model, with nothing pending: nothing.
    assert_eq!(switch(Some(OLD), None, OLD, fnv_of(OLD)), Switch::Nothing);
    let kept = Trial {
        state: State::Kept,
        ..record.clone()
    };
    assert_eq!(
        switch(Some(NEW), Some(&kept), NEW, fnv_of(NEW)),
        Switch::Nothing
    );
    assert_eq!(
        switch(Some(NEW), Some(&kept), OLD, fnv_of(OLD)),
        Switch::Try(pending(OLD, Some(NEW)))
    );
    // No model ran: the trial goes back to none.
    assert_eq!(
        switch(None, None, NEW, fnv_of(NEW)),
        Switch::Try(pending(NEW, None))
    );
}

#[test]
fn the_candidate_is_kept_only_once_it_loads() {
    const FNV: u64 = 0xfeed;
    let limit = 300_000;
    assert_eq!(
        decide(FNV, Health::Ready { fnv: FNV }, 5, limit),
        Decision::Keep
    );
    assert_eq!(decide(FNV, Health::NotYet, 1_000, limit), Decision::Wait);
    let back = |health, waited| match decide(FNV, health, waited, limit) {
        Decision::RollBack(why) => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(back(Health::NotYet, limit), "it did not load within 300 s");
    assert_eq!(
        back(Health::Ready { fnv: 1 }, 5),
        "another model started in its place"
    );
    assert_eq!(back(Health::NoModel, 5), "no model started");
    assert_eq!(
        back(Health::Failed("refused header\nline two"), 5),
        "it did not load: refused header line two"
    );
}

#[test]
fn a_boot_marks_a_trial_attempted_and_one_never_decided_rolls_back() {
    let record = pending(NEW, Some(OLD));
    let attempted = Trial {
        state: State::Attempted,
        ..record.clone()
    };
    assert_eq!(
        at_boot(Some(&record), false),
        AtBoot::Try(attempted.clone())
    );
    // That boot hung before it decided: the next one does not try again.
    assert_eq!(
        at_boot(Some(&attempted), false),
        AtBoot::RollBack(Trial {
            state: State::RolledBack(HUNG.into()),
            ..record.clone()
        })
    );
    let decided = [
        Trial {
            state: State::Kept,
            ..record.clone()
        },
        Trial {
            state: State::RolledBack("no model started".into()),
            ..record.clone()
        },
    ];
    for t in &decided {
        for again in [false, true] {
            assert_eq!(at_boot(Some(t), again), AtBoot::Nothing);
        }
    }
    assert_eq!(at_boot(None, false), AtBoot::Nothing);
    assert_eq!(at_boot(None, true), AtBoot::Nothing);
}

/// Bug hunt 1, #25: a switch while a boot judges a trial wrote a new pending
/// record, which the probation's verdict on the old candidate then wrote over.
#[test]
fn no_switch_while_a_boot_judges_the_trial_and_its_verdict_writes_only_over_it() {
    let attempted = Trial {
        state: State::Attempted,
        ..pending(NEW, Some(OLD))
    };
    // Whatever runs (llm is loading the candidate and does not say), and
    // whichever model it names: refused, the record as it is.
    for running in [None, Some(OLD), Some(NEW), Some(OTHER)] {
        for to in [OLD, NEW, OTHER] {
            assert_eq!(
                switch(running, Some(&attempted), to, fnv_of(to)),
                Switch::OnTrial,
                "{running:?} to {to:?}"
            );
        }
    }
    // The verdict on the judged trial, over that trial.
    let kept = Trial {
        state: State::Kept,
        ..attempted.clone()
    };
    assert_eq!(
        verdict(Some(&attempted), &attempted, State::Kept),
        Some(kept.clone())
    );
    let why = State::RolledBack("no model started".into());
    assert_eq!(
        verdict(Some(&attempted), &attempted, why.clone()),
        Some(Trial {
            state: why.clone(),
            ..attempted.clone()
        })
    );
    // A record since replaced, cancelled or decided keeps what it says, and
    // so does the hint: nothing is written.
    let replaced = pending(OTHER, None);
    let other_attempt = Trial {
        state: State::Attempted,
        ..pending(OTHER, Some(OLD))
    };
    for record in [Some(&replaced), Some(&other_attempt), Some(&kept), None] {
        assert_eq!(verdict(record, &attempted, State::Kept), None, "{record:?}");
        assert_eq!(verdict(record, &attempted, why.clone()), None, "{record:?}");
    }
}

/// Bug hunt 1, #26 and #27's second check: the way back a new trial records
/// is the model the next restart maps without it, as the record says, not a
/// candidate `llm` still runs after its rollback, nor none because `llm` did
/// not say.
#[test]
fn a_new_trial_goes_back_to_the_model_the_hint_names() {
    // NEW was rolled back to OLD: the hint names OLD again, though this boot
    // may still run NEW (it hung the boot before, and loaded this time) or
    // run nothing.
    let rolled = Trial {
        state: State::RolledBack(HUNG.into()),
        ..pending(NEW, Some(OLD))
    };
    for running in [Some(NEW), None] {
        assert_eq!(
            switch(running, Some(&rolled), OTHER, fnv_of(OTHER)),
            Switch::Try(pending(OTHER, Some(OLD))),
            "{running:?}"
        );
        // The model the hint names: nothing to do.
        assert_eq!(
            switch(running, Some(&rolled), OLD, fnv_of(OLD)),
            Switch::Nothing,
            "{running:?}"
        );
        // The rolled-back candidate again, asked for: a trial, not "already".
        assert_eq!(
            switch(running, Some(&rolled), NEW, fnv_of(NEW)),
            Switch::Try(pending(NEW, Some(OLD))),
            "{running:?}"
        );
    }
    // A kept candidate is what the hint names.
    let kept = Trial {
        state: State::Kept,
        ..pending(NEW, Some(OLD))
    };
    assert_eq!(
        switch(None, Some(&kept), OTHER, fnv_of(OTHER)),
        Switch::Try(pending(OTHER, Some(NEW)))
    );
    // A pending trial replaced keeps its way back, and switching to that way
    // back cancels it.
    let record = pending(NEW, Some(OLD));
    assert_eq!(
        switch(None, Some(&record), OTHER, fnv_of(OTHER)),
        Switch::Try(pending(OTHER, Some(OLD)))
    );
    assert_eq!(
        switch(None, Some(&record), OLD, fnv_of(OLD)),
        Switch::Cancel(settled(OLD))
    );
}

/// Bug hunt 1, #27: `updated` started again within a boot (the supervisor's
/// restart, or one the user asked for) took this boot's own attempt for a
/// boot that hung, and rolled back a candidate that was loading.
#[test]
fn a_start_again_within_the_boot_judges_by_what_llm_runs() {
    let record = pending(NEW, Some(OLD));
    let attempted = Trial {
        state: State::Attempted,
        ..record.clone()
    };
    assert_eq!(
        at_boot(Some(&attempted), true),
        AtBoot::Resume(attempted.clone())
    );
    // A pending trial found by a start again: written this boot for the next
    // restart, or this boot's while its first start ended before it marked
    // it. `llm` says which (`check`), and nothing is written meanwhile.
    assert_eq!(at_boot(Some(&record), true), AtBoot::Check(record.clone()));
    // The first start of a boot keeps K's rule.
    assert_eq!(
        at_boot(Some(&attempted), false),
        AtBoot::RollBack(Trial {
            state: State::RolledBack(HUNG.into()),
            ..record
        })
    );
    // The supervisor's `restart=N`, N its restarts of `updated` this boot.
    for (args, again) in [
        ("restart=1 serve", true),
        ("restart=12 step=probation", true),
        ("restart=0 serve", false),
        ("serve", false),
        ("", false),
        ("restart=x serve", false),
        ("norestart=3", false),
    ] {
        assert_eq!(started_again(args), again, "{args}");
    }
}

/// `llm`'s reply to `health` as the service writes it: `ok ` or `err `, and
/// an error line cut to 160 characters, as `llm` cuts it.
fn served(reply: Result<String, String>) -> String {
    match reply {
        Ok(text) => format!("ok {text}"),
        Err(why) => format!("err {}", llmproto::script::error_line(&why, 160)),
    }
}

fn judged(reply: &str) -> Health<'_> {
    let state = llmproto::health::state_of(reply).map(llmproto::health::State::word);
    health_of(reply, state)
}

/// Bug hunt 1, #28: with an LLM server named, a server whose health fails
/// hid the local model's state, so every trial rolled back after 300 s.
#[test]
fn a_failing_server_does_not_hide_the_local_model_on_trial() {
    use llmproto::health::{SERVER_HEALTH_MS, State as Llm, Status, reply};
    const FNV: u64 = 0xff;
    let model = String::from("model 2497281120 bytes fnv1a64 00000000000000ff");
    let engine = "local model Qwen3-4B-Instruct-2507-Q4_K_M (fast tier, 4 workers, context 4096, no answer yet)";
    let status = |state| Status { state, tps10: 0 };
    let limit = 300_000;
    for error in [
        String::from("network: ConnectionRefused"),
        format!("network: {}", "the server said something long ".repeat(10)),
    ] {
        let up = served(reply(
            Some(Err(error.clone())),
            Some(format!("{model}; {engine}")),
            status(Llm::Ready),
            160,
        ));
        assert!(up.starts_with("err "), "{up}");
        assert_eq!(judged(&up), Health::Ready { fnv: FNV }, "{up}");
        assert_eq!(decide(FNV, judged(&up), limit, limit), Decision::Keep);
        // The engine that cannot run is a failure, not a model that loaded.
        let cannot = served(reply(
            Some(Err(error)),
            Some(format!("{model}; the model cannot run: no tensors")),
            status(Llm::Failed),
            160,
        ));
        assert!(
            matches!(judged(&cannot), Health::Failed(_)),
            "{cannot}: {:?}",
            judged(&cannot)
        );
    }
    // The server's health answered: as before.
    let both = served(reply(
        Some(Ok(String::from("HTTP 200 in 4 ms"))),
        Some(format!("{model}; {engine}")),
        status(Llm::Ready),
        160,
    ));
    assert_eq!(judged(&both), Health::Ready { fnv: FNV });
    // No model and no server is final: no model started, at once.
    let none = served(reply(None, None, status(Llm::NoModel), 160));
    assert_eq!(judged(&none), Health::NoModel, "{none}");
    // A server that does not answer at all: the probation's call outlasts
    // `llm`'s own wait on it, so it hears the local model's state.
    const { assert!(HEALTH_WAIT_MS > SERVER_HEALTH_MS) };
}

/// Every state `llm` reports is read: one that cannot change this boot
/// decides, the others wait.
#[test]
fn every_llm_state_is_read() {
    use llmproto::health::State as Llm;
    let text = "ok model 12 bytes fnv1a64 00000000000000ff; x; status state=ready tps10=0";
    for state in Llm::ALL {
        let health = health_of(text, Some(state.word()));
        let want = match state {
            Llm::Ready | Llm::Warming => Health::Ready { fnv: 0xff },
            Llm::Failed => Health::Failed(text),
            Llm::NoModel | Llm::Remote => Health::NoModel,
            Llm::Waiting | Llm::Busy => Health::NotYet,
        };
        assert_eq!(health, want, "{state:?}");
    }
    assert_eq!(health_of(text, None), Health::NotYet);
    assert_eq!(
        health_of("ok x; status state=ready tps10=0", Some("ready")),
        Health::Failed("its health names no model")
    );
}

const FOURTH: [u8; 32] = [0x44; 32];
const LIMIT: u64 = 300_000;

/// What the next boot finds: the hint, whose model the kernel maps, and the
/// trial record.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Disk {
    hint: Option<[u8; 32]>,
    record: Option<Trial>,
}

impl Disk {
    /// After the first `n` of `writes`, as when the one after fails or the
    /// boot hangs there.
    fn made(&self, writes: &[Write], n: usize) -> Disk {
        let mut disk = self.clone();
        for write in writes.iter().take(n) {
            match write {
                Write::Hint(model) => disk.hint = *model,
                Write::Record(trial) => disk.record = Some(trial.clone()),
            }
        }
        disk
    }

    fn all(&self, writes: &[Write]) -> Disk {
        self.made(writes, writes.len())
    }

    /// Whether the next boot would run a model with no trial (the record
    /// neither settles on the hint's model, nor tries it, nor goes back to
    /// it), or map a candidate a boot already tried: an attempted record
    /// with the hint still on its candidate, unless `llm` said the candidate
    /// `loaded` (the cut inside a keep).
    fn runs_untried(&self, loaded: Option<[u8; 32]>) -> bool {
        let Some(t) = &self.record else {
            return false;
        };
        match &t.state {
            State::Kept => self.hint != Some(t.candidate),
            State::RolledBack(_) => self.hint != t.previous,
            State::Pending => self.hint != Some(t.candidate) && self.hint != t.previous,
            State::Attempted => {
                let kept_loaded = self.hint == Some(t.candidate) && loaded == Some(t.candidate);
                self.hint != t.previous && !kept_loaded
            }
        }
    }
}

/// `llm` runs `model`, loaded.
fn runs(model: [u8; 32]) -> Health<'static> {
    Health::Ready { fnv: fnv_of(model) }
}

/// The review of #1349, HIGH 1, scenario A: a boot marks the candidate and
/// hangs; in the next boot the first start of `updated` ends before it rolls
/// the trial back (a crash or a kill), or its at_boot fails on an fs error,
/// and a start again (`restart=1`) finds it attempted. The candidate is not
/// tried again: the mark put the hint on the way back, so that boot maps the
/// way back, and the start again rolls back by what `llm` runs.
#[test]
fn a_candidate_that_hung_a_boot_is_not_tried_again_whatever_updated_got_to() {
    // OLD runs; a switch to NEW.
    let used = switch(Some(OLD), None, NEW, fnv_of(NEW));
    let disk = Disk {
        hint: Some(OLD),
        record: None,
    }
    .all(&used.writes());
    assert_eq!(disk.hint, Some(NEW));
    // Boot 1 maps NEW; updated's first start marks it, then the boot hangs.
    let first = start(&at_boot(disk.record.as_ref(), false), 0);
    let disk = disk.all(&first.writes);
    assert_eq!(
        disk.record.as_ref().map(|t| &t.state),
        Some(&State::Attempted)
    );
    // Boot 2 maps what the hint names: the way back, not the candidate.
    assert_eq!(disk.hint, Some(OLD));
    let rolls_back = start(&at_boot(disk.record.as_ref(), false), 0);
    assert_eq!(rolls_back.retry, None);
    // (a) its first start ended before at_boot; (b) at_boot's first write
    // failed. Either way a start again finds the trial attempted.
    for disk in [disk.clone(), disk.made(&rolls_back.writes, 0)] {
        let found = at_boot(disk.record.as_ref(), true);
        assert!(matches!(found, AtBoot::Resume(_)), "{found:?}");
        let again = start(&found, 1_000);
        let disk = disk.all(&again.writes);
        let probation = again.probation.expect("a resumed probation");
        assert!(probation.holds_switches());
        // llm runs OLD, which boot 2 mapped: never NEW, and no keep.
        let seen = look(&probation, disk.record.as_ref(), runs(OLD), 7_000, LIMIT);
        let disk = disk.all(&seen.writes);
        assert_eq!(seen.after, None);
        assert_eq!(
            disk,
            Disk {
                hint: Some(OLD),
                record: Some(Trial {
                    state: State::RolledBack("another model started in its place".into()),
                    ..pending(NEW, Some(OLD))
                }),
            }
        );
    }
}

/// HIGH 1, scenario B: a boot maps the pending candidate, and the first start
/// of `updated` ends before it marks it. A start again finds it pending,
/// points the hint at the way back before it waits on `llm` (the delta review
/// of #1349), and checks what `llm` runs: another model means the record was
/// written this boot for the next restart, and the hint goes back on its
/// candidate; the candidate, loaded or not, is this boot's trial, marked and
/// judged this boot.
#[test]
fn a_pending_trial_a_first_start_left_unmarked_is_judged_once_llm_says_it_runs() {
    let record = pending(NEW, Some(OLD));
    let disk = Disk {
        hint: Some(NEW),
        record: Some(record.clone()),
    };
    let found = at_boot(disk.record.as_ref(), true);
    assert_eq!(found, AtBoot::Check(record.clone()));
    let begun = start(&found, 0);
    assert_eq!(begun.writes, vec![Write::Hint(Some(OLD))]);
    let checking = begun.probation.expect("a check");
    assert!(checking.checking);
    let disk = disk.all(&begun.writes);
    // llm is still loading: nothing more is written yet.
    let waiting = look(
        &checking,
        disk.record.as_ref(),
        Health::NotYet,
        5_000,
        LIMIT,
    );
    assert!(waiting.writes.is_empty());
    assert_eq!(waiting.after, Some(checking.clone()));

    // llm runs NEW: marked, with the hint on the way back, then kept.
    let marked = look(&checking, disk.record.as_ref(), runs(NEW), 10_000, LIMIT);
    let disk1 = disk.all(&marked.writes);
    assert_eq!(disk1.hint, Some(OLD));
    let judging = marked.after.expect("judged at the next look");
    assert!(judging.holds_switches());
    let kept = look(&judging, disk1.record.as_ref(), runs(NEW), 15_000, LIMIT);
    assert_eq!(kept.after, None);
    assert_eq!(
        disk1.all(&kept.writes),
        Disk {
            hint: Some(NEW),
            record: Some(Trial {
                state: State::Kept,
                ..record.clone()
            }),
        }
    );

    // llm mapped NEW and cannot run it: marked, then rolled back this boot.
    let text = "ok model 9 bytes fnv1a64 2222222222222222; the model cannot run: no tensors; status state=failed tps10=0";
    let cannot = health_of(text, Some("failed"));
    assert_eq!(check(&record, cannot, 0, LIMIT), Checked::Judge);
    let marked = look(&checking, disk.record.as_ref(), cannot, 5_000, LIMIT);
    let disk1 = disk.all(&marked.writes);
    let judging = marked.after.expect("judged at the next look");
    let back = look(&judging, disk1.record.as_ref(), cannot, 10_000, LIMIT);
    let disk2 = disk1.all(&back.writes);
    assert_eq!(disk2.hint, Some(OLD));
    let state = disk2.record.map(|t| t.state);
    assert!(
        matches!(&state, Some(State::RolledBack(why)) if why.starts_with("it did not load")),
        "{state:?}"
    );

    // This boot runs another model, loaded or not: the record was written
    // this boot for the next restart. The hint goes back on its candidate,
    // and the record stays.
    let other = "ok model 9 bytes fnv1a64 1111111111111111; the model cannot run: x; status state=failed tps10=0";
    for health in [runs(OLD), health_of(other, Some("failed"))] {
        assert_eq!(check(&record, health, 0, LIMIT), Checked::Leave);
        let left = look(&checking, disk.record.as_ref(), health, 5_000, LIMIT);
        assert_eq!(left.after, None, "{health:?}");
        assert_eq!(
            disk.all(&left.writes),
            Disk {
                hint: Some(NEW),
                record: Some(record.clone()),
            },
            "{health:?}"
        );
    }
    // A switch while it checks replaces the record: the check stops there,
    // writing nothing, and the switch's hint stands.
    let used = switch(None, disk.record.as_ref(), OTHER, fnv_of(OTHER));
    let replaced = disk.all(&used.writes());
    let stopped = look(&checking, replaced.record.as_ref(), runs(NEW), 5_000, LIMIT);
    assert!(stopped.writes.is_empty() && stopped.after.is_none());
    assert_eq!(replaced.hint, Some(OTHER));
}

/// The delta review of #1349, HIGH 1: a check waits on `llm` while it loads
/// the candidate, and the candidate hangs the boot. The next boot maps the
/// way back, and its first start marks the candidate and rolls it back: it
/// is not tried again.
#[test]
fn a_candidate_that_hangs_a_boot_while_a_check_waits_is_not_tried_again() {
    let record = pending(NEW, Some(OLD));
    let disk = Disk {
        hint: Some(NEW),
        record: Some(record.clone()),
    };
    let begun = start(&at_boot(disk.record.as_ref(), true), 0);
    let checking = begun.probation.clone().expect("a check");
    let disk = disk.all(&begun.writes);
    let waiting = look(
        &checking,
        disk.record.as_ref(),
        Health::NotYet,
        5_000,
        LIMIT,
    );
    let disk = disk.all(&waiting.writes);
    // The boot hangs here: the next one maps the way back.
    assert_eq!(
        disk,
        Disk {
            hint: Some(OLD),
            record: Some(record.clone()),
        }
    );
    let first = start(&at_boot(disk.record.as_ref(), false), 0);
    let disk = disk.all(&first.writes);
    let judging = first.probation.expect("a probation");
    let seen = look(&judging, disk.record.as_ref(), runs(OLD), 5_000, LIMIT);
    assert_eq!(
        disk.all(&seen.writes),
        Disk {
            hint: Some(OLD),
            record: Some(Trial {
                state: State::RolledBack("another model started in its place".into()),
                ..record
            }),
        }
    );
}

/// The delta review of #1349, MEDIUM 2: in the same case, a candidate the
/// kernel could not map leaves `llm` with no model it can name, and `llm`
/// silent for the whole limit may be loading it: the check judges it, and it
/// is rolled back this boot, not the next.
#[test]
fn a_candidate_the_boot_could_not_map_or_load_rolls_back_this_boot() {
    let record = pending(NEW, Some(OLD));
    let disk = Disk {
        hint: Some(NEW),
        record: Some(record.clone()),
    };
    let begun = start(&at_boot(disk.record.as_ref(), true), 0);
    let checking = begun.probation.clone().expect("a check");
    let disk = disk.all(&begun.writes);
    let unmapped = "err the model cannot run: x; status state=failed tps10=0";
    let cases = [
        (Health::NoModel, 5_000, "no model started"),
        (
            health_of(unmapped, Some("failed")),
            5_000,
            "it did not load: ",
        ),
        (Health::NotYet, LIMIT, "it did not load within 300 s"),
    ];
    for (health, at, why) in cases {
        assert_eq!(check(&record, health, at, LIMIT), Checked::Judge);
        let marked = look(&checking, disk.record.as_ref(), health, at, LIMIT);
        let disk1 = disk.all(&marked.writes);
        let judging = marked.after.expect("judged at the next look");
        let back = look(&judging, disk1.record.as_ref(), health, at + 5_000, LIMIT);
        let disk2 = disk1.all(&back.writes);
        assert_eq!(disk2.hint, Some(OLD), "{health:?}");
        let state = disk2.record.map(|t| t.state);
        assert!(
            matches!(&state, Some(State::RolledBack(w)) if w.starts_with(why)),
            "{state:?}"
        );
    }
    assert_eq!(
        check(&record, Health::NotYet, LIMIT - 1, LIMIT),
        Checked::Wait
    );
}

/// A first start whose marks fail checks the pending record instead, and its
/// next look marks it again.
#[test]
fn a_first_start_whose_marks_fail_marks_again_at_its_next_look() {
    let record = pending(NEW, Some(OLD));
    let disk = Disk {
        hint: Some(NEW),
        record: Some(record.clone()),
    };
    let first = start(&at_boot(disk.record.as_ref(), false), 0);
    let retry = first.retry.clone().expect("a retry");
    assert!(retry.checking);
    assert_eq!(retry.trial, record);
    for n in 0..first.writes.len() {
        let cut = disk.made(&first.writes, n);
        let marked = look(&retry, cut.record.as_ref(), runs(NEW), 5_000, LIMIT);
        let cut = cut.all(&marked.writes);
        assert_eq!(cut.hint, Some(OLD), "{n}");
        assert_eq!(cut.record.map(|t| t.state), Some(State::Attempted), "{n}");
    }
}

/// The review of #1349, LOW 7: every plan's writes, in order. Cut after any
/// of them (a write that fails, a boot that hangs), the next boot never maps
/// a model the record neither settles on, nor tries, nor goes back to.
#[test]
fn every_plan_writes_in_an_order_a_cut_leaves_safe() {
    let p = pending(NEW, Some(OLD));
    let attempted = Trial {
        state: State::Attempted,
        ..p.clone()
    };
    let judging = start(&AtBoot::Try(attempted.clone()), 0)
        .probation
        .expect("a probation");
    let checking = start(&AtBoot::Check(p.clone()), 0)
        .probation
        .expect("a check");
    let with = |hint, record: &Trial| Disk {
        hint: Some(hint),
        record: Some(record.clone()),
    };
    let rolled = |why: &str| Trial {
        state: State::RolledBack(why.into()),
        ..p.clone()
    };
    // Each plan: the disk before it, its writes, the writes it must make,
    // and the candidate `llm` said loaded when it is made.
    let plans = [
        (
            "switch",
            with(OLD, &settled(OLD)),
            switch(None, Some(&settled(OLD)), NEW, fnv_of(NEW)).writes(),
            vec![Write::Record(p.clone()), Write::Hint(Some(NEW))],
            None,
        ),
        (
            "cancel",
            with(NEW, &p),
            switch(None, Some(&p), OLD, fnv_of(OLD)).writes(),
            vec![Write::Hint(Some(OLD)), Write::Record(settled(OLD))],
            None,
        ),
        (
            "first start",
            with(NEW, &p),
            start(&at_boot(Some(&p), false), 0).writes,
            vec![Write::Hint(Some(OLD)), Write::Record(attempted.clone())],
            None,
        ),
        (
            "start again, attempted",
            with(OLD, &attempted),
            start(&at_boot(Some(&attempted), true), 0).writes,
            vec![Write::Hint(Some(OLD))],
            None,
        ),
        (
            "start again, pending",
            with(NEW, &p),
            start(&at_boot(Some(&p), true), 0).writes,
            vec![Write::Hint(Some(OLD))],
            None,
        ),
        // From the hint still on the candidate, as when the check's first
        // write failed: a record written first would be caught.
        (
            "check marks",
            with(NEW, &p),
            look(&checking, Some(&p), runs(NEW), 5_000, LIMIT).writes,
            vec![Write::Hint(Some(OLD)), Write::Record(attempted.clone())],
            None,
        ),
        (
            "check leaves",
            with(OLD, &p),
            look(&checking, Some(&p), runs(OTHER), 5_000, LIMIT).writes,
            vec![Write::Hint(Some(NEW))],
            None,
        ),
        (
            "keep",
            with(OLD, &attempted),
            look(&judging, Some(&attempted), runs(NEW), 5_000, LIMIT).writes,
            vec![
                Write::Hint(Some(NEW)),
                Write::Record(Trial {
                    state: State::Kept,
                    ..p.clone()
                }),
            ],
            Some(NEW),
        ),
        (
            "rollback",
            with(OLD, &attempted),
            look(&judging, Some(&attempted), Health::NoModel, 5_000, LIMIT).writes,
            vec![
                Write::Hint(Some(OLD)),
                Write::Record(rolled("no model started")),
            ],
            None,
        ),
        (
            "rollback at boot",
            with(OLD, &attempted),
            start(&at_boot(Some(&attempted), false), 0).writes,
            vec![Write::Hint(Some(OLD)), Write::Record(rolled(HUNG))],
            None,
        ),
    ];
    for (name, before, writes, want, loaded) in plans {
        assert_eq!(writes, want, "{name}");
        for n in 0..=writes.len() {
            assert!(
                !before.made(&writes, n).runs_untried(loaded),
                "{name} cut after {n}"
            );
        }
    }
    // The other orders would not be: a settled record with the hint still on
    // the cancelled candidate, a kept one with it still on the way back, and
    // a mark with it still on the candidate (also the record a mark before
    // #1349 left, the upgrade corner in the decision row).
    assert!(with(NEW, &settled(OLD)).runs_untried(None));
    let kept = Trial {
        state: State::Kept,
        ..p.clone()
    };
    assert!(with(OLD, &kept).runs_untried(Some(NEW)));
    assert!(with(NEW, &attempted).runs_untried(None));
}

/// LOW 7: a switch waits for a probation that judges an attempted trial
/// (`updated` answers it `ON_TRIAL` before it reads anything), not for a
/// check of a pending one, which a switch ends.
#[test]
fn a_switch_waits_for_a_probation_that_judges_and_not_for_a_check() {
    let p = pending(NEW, Some(OLD));
    let attempted = Trial {
        state: State::Attempted,
        ..p.clone()
    };
    let probation = |found: AtBoot| start(&found, 0).probation.expect("a probation");
    let first = start(&AtBoot::Try(attempted.clone()), 0);
    assert!(first.probation.expect("judged").holds_switches());
    assert!(!first.retry.expect("checked").holds_switches());
    assert!(probation(AtBoot::Resume(attempted.clone())).holds_switches());
    let checking: Probation = probation(AtBoot::Check(p.clone()));
    assert!(!checking.holds_switches());
    // A record changed under a probation that judges: no verdict, no write.
    let judging = probation(AtBoot::Try(attempted.clone()));
    let replaced = pending(OTHER, Some(OLD));
    let seen = look(&judging, Some(&replaced), runs(NEW), 5_000, LIMIT);
    assert!(seen.writes.is_empty() && seen.after.is_none());
    // Waiting on llm writes nothing and goes on.
    let seen = look(&judging, Some(&attempted), Health::NotYet, 5_000, LIMIT);
    assert!(seen.writes.is_empty());
    assert_eq!(seen.after, Some(judging));
}

/// The review of #1349, MEDIUM 3: after a rollback while this boot still
/// runs the candidate, a switch and its undo leave a record that names the
/// model the hint is back on, so the next switch goes back to it, not to the
/// candidate `llm` runs; the list shows it as the next restart's.
#[test]
fn a_cancelled_switch_keeps_the_way_back_on_record() {
    let rolled = Trial {
        state: State::RolledBack(HUNG.into()),
        ..pending(NEW, Some(OLD))
    };
    let disk = Disk {
        hint: Some(OLD),
        record: Some(rolled),
    };
    assert_eq!(next(disk.record.as_ref(), Some(Some(NEW))), Some(OLD));
    let used = switch(None, disk.record.as_ref(), OTHER, fnv_of(OTHER));
    assert_eq!(used, Switch::Try(pending(OTHER, Some(OLD))));
    let disk = disk.all(&used.writes());
    let undone = switch(None, disk.record.as_ref(), OLD, fnv_of(OLD));
    assert_eq!(undone, Switch::Cancel(settled(OLD)));
    let disk = disk.all(&undone.writes());
    assert_eq!(
        disk,
        Disk {
            hint: Some(OLD),
            record: Some(settled(OLD)),
        }
    );
    assert_eq!(next(disk.record.as_ref(), Some(Some(NEW))), Some(OLD));
    assert_eq!(
        switch(Some(NEW), disk.record.as_ref(), FOURTH, fnv_of(FOURTH)),
        Switch::Try(pending(FOURTH, Some(OLD)))
    );
    assert_eq!(
        switch(Some(NEW), disk.record.as_ref(), OLD, fnv_of(OLD)),
        Switch::Nothing
    );
}

#[test]
fn the_list_shows_the_next_restarts_model_only_where_it_knows() {
    let p = pending(NEW, Some(OLD));
    let with = |state| Trial { state, ..p.clone() };
    assert_eq!(next(Some(&p), None), Some(NEW));
    assert_eq!(next(Some(&p), Some(Some(OLD))), Some(NEW));
    assert_eq!(next(Some(&with(State::Attempted)), Some(Some(NEW))), None);
    let kept = with(State::Kept);
    assert_eq!(next(Some(&kept), Some(Some(NEW))), None);
    // llm busy: the kept model is not shown as the next restart's.
    assert_eq!(next(Some(&kept), None), None);
    assert_eq!(next(Some(&kept), Some(Some(OLD))), Some(NEW));
    let rolled = with(State::RolledBack("x".into()));
    assert_eq!(next(Some(&rolled), Some(Some(NEW))), Some(OLD));
    assert_eq!(next(Some(&rolled), Some(None)), Some(OLD));
    assert_eq!(next(Some(&rolled), Some(Some(OLD))), None);
    assert_eq!(next(Some(&rolled), None), None);
    assert_eq!(next(None, Some(Some(OLD))), None);
}
