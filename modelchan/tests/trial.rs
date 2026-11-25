//! `kai-modeltrial 1` and the probation a switched model is on.

use modelchan::trial::{
    AtBoot, Decision, HEALTH_WAIT_MS, HUNG, Health, MAX_WHY, State, Switch, Trial, at_boot, decide,
    health_of, started_again, switch, verdict,
};

const OLD: [u8; 32] = [0x11; 32];
const NEW: [u8; 32] = [0x22; 32];
const OTHER: [u8; 32] = [0x33; 32];

/// A model's FNV-1a64, made up for the test.
fn fnv_of(model: [u8; 32]) -> u64 {
    u64::from(model[0]) * 0x0101_0101_0101_0101
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
        Switch::Cancel
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
        Switch::Cancel
    );
}

/// Bug hunt 1, #27: `updated` started again within a boot (the supervisor's
/// restart, or one the user asked for) took this boot's own attempt for a
/// boot that hung, and rolled back a candidate that was loading.
#[test]
fn a_start_again_within_the_boot_resumes_the_probation() {
    let record = pending(NEW, Some(OLD));
    let attempted = Trial {
        state: State::Attempted,
        ..record.clone()
    };
    assert_eq!(
        at_boot(Some(&attempted), true),
        AtBoot::Resume(attempted.clone())
    );
    // A pending trial found by a start again was written this boot, for the
    // next restart: this boot does not judge it.
    assert_eq!(at_boot(Some(&record), true), AtBoot::Nothing);
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
