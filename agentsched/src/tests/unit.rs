//! What the scheduler decides, one rule at a time.

use super::fake::*;
use crate::*;
use std::vec;
use std::vec::Vec;

const BIG: Budgets = Budgets {
    tokens_per_day: u32::MAX,
    kv_positions: 32_768,
    pool_mib: 512,
};

fn big(id: u32, class: Class, weight: u32) -> Agent {
    agent(id, class, weight, BIG)
}

/// The requests of the `Run` events in `log`, in order, with their quanta.
fn runs(log: &[Event]) -> Vec<(RequestId, Kind, u32)> {
    log.iter()
        .filter_map(|e| match e {
            Event::Run(q) => Some(q.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn the_classes_run_in_order_and_each_to_its_end() {
    let agents = [
        kai(),
        big(2, Class::Interactive, 1),
        big(3, Class::Background, 1),
        big(4, Class::Index, 1),
    ];
    let mut e = Engine::new(
        Config {
            kv_slots: 4,
            ..Config::default()
        },
        &agents,
    );
    // Asked lowest class first, so only the classes can put them in order.
    let index = e.ask(AgentId(4), 64, 0).unwrap();
    let background = e.ask(AgentId(3), 64, 6).unwrap();
    let interactive = e.ask(AgentId(2), 64, 6).unwrap();
    let foreground = e.ask(KAI, 64, 6).unwrap();
    e.run_idle(200);
    let order: Vec<RequestId> = runs(&e.log).into_iter().map(|(r, _, _)| r).collect();
    let first = |r: RequestId| order.iter().position(|x| *x == r).unwrap();
    let last = |r: RequestId| order.iter().rposition(|x| *x == r).unwrap();
    assert!(last(foreground) < first(interactive));
    assert!(last(interactive) < first(background));
    assert!(last(background) < first(index));
    assert_eq!(e.sched.pending(), 0);
}

#[test]
fn a_foreground_request_runs_at_the_next_safe_point_and_the_background_goes_on_exactly() {
    let mut e = Engine::new(Config::default(), &[kai(), big(2, Class::Background, 1)]);
    let b = e.ask(AgentId(2), 64, 40).unwrap();
    // The background request prefills and decodes a few tokens.
    for _ in 0..8 {
        e.step();
    }
    assert!(
        runs(&e.log)
            .iter()
            .any(|(r, k, _)| *r == b && *k == Kind::Decode)
    );
    let before = e.log.len();
    let f = e.ask(KAI, 32, 3).unwrap();
    e.run_idle(200);
    // The second slot is free, so the foreground takes it and nothing is copied.
    let after: Vec<&Event> = e.log[before..].iter().collect();
    assert_eq!(after[0], &Event::Move(f, Place::Cold, Place::Slot));
    assert!(
        after
            .iter()
            .all(|ev| !matches!(ev, Event::Move(_, Place::Slot, _)))
    );
    // Its own quanta come first, all of them, before the background's next.
    let order: Vec<RequestId> = runs(&e.log[before..]).iter().map(|(r, _, _)| *r).collect();
    let f_end = order.iter().rposition(|r| *r == f).unwrap();
    assert!(order[..=f_end].iter().all(|r| *r == f));
    // The background request made every token it asked for, none twice.
    let decoded: u32 = runs(&e.log)
        .iter()
        .filter(|(r, k, _)| *r == b && *k == Kind::Decode)
        .map(|(_, _, t)| t)
        .sum();
    let prefilled: u32 = runs(&e.log)
        .iter()
        .filter(|(r, k, _)| *r == b && *k == Kind::Prefill)
        .map(|(_, _, t)| t)
        .sum();
    assert_eq!((decoded, prefilled), (40, 64));
}

#[test]
fn with_one_slot_the_background_state_goes_to_ram_and_comes_back_whole() {
    let config = Config {
        kv_slots: 1,
        ..Config::default()
    };
    let mut e = Engine::new(config, &[kai(), big(2, Class::Background, 1)]);
    let b = e.ask(AgentId(2), 64, 20).unwrap();
    for _ in 0..6 {
        e.step();
    }
    assert_eq!(e.sched.place(b), Some(Place::Slot));
    let before = e.log.len();
    let f = e.ask(KAI, 32, 2).unwrap();
    assert_eq!(e.step(), Event::Move(b, Place::Slot, Place::Ram));
    assert_eq!(e.sched.place(b), Some(Place::Ram));
    assert!(e.sched.ram_bytes(None) > 0);
    assert_eq!(e.step(), Event::Move(f, Place::Cold, Place::Slot));
    e.run_idle(200);
    // It came back from RAM: no prefill again, and every token was made once.
    assert!(e.log[before..].contains(&Event::Move(b, Place::Ram, Place::Slot)));
    let total: u32 = runs(&e.log)
        .iter()
        .filter(|(r, _, _)| *r == b)
        .map(|(_, _, t)| t)
        .sum();
    assert_eq!(total, 64 + 20);
    assert_eq!(e.sched.ram_bytes(None), 0);
}

#[test]
fn a_state_that_does_not_fit_in_the_pool_spills_and_without_spill_is_dropped_and_charged_again() {
    let agents = [kai(), big(2, Class::Background, 1)];
    // No pool: the state spills, and comes back as it was.
    let spill = Config {
        kv_slots: 1,
        pool_mib: 0,
        ..Config::default()
    };
    let mut e = Engine::new(spill, &agents);
    let b = e.ask(AgentId(2), 64, 10).unwrap();
    for _ in 0..5 {
        e.step();
    }
    e.ask(KAI, 32, 1).unwrap();
    assert_eq!(e.step(), Event::Move(b, Place::Slot, Place::Spill));
    e.run_idle(200);
    assert_eq!(e.sched.used_today(AgentId(2)), Some(64 + 10));
    // Without spill it is dropped, so it prefills again, and the day counts it again.
    let drop = Config {
        spill: false,
        ..spill
    };
    let mut e = Engine::new(drop, &agents);
    let b = e.ask(AgentId(2), 64, 10).unwrap();
    for _ in 0..5 {
        e.step();
    }
    e.ask(KAI, 32, 1).unwrap();
    assert_eq!(e.step(), Event::Move(b, Place::Slot, Place::Cold));
    e.run_idle(200);
    assert!(e.log.contains(&Event::Move(b, Place::Cold, Place::Slot)));
    assert!(e.sched.used_today(AgentId(2)).unwrap() > 64 + 10);
}

#[test]
fn an_agent_over_its_pool_share_spills_first_and_a_full_pool_demotes_its_lowest_snapshot() {
    // 32 positions are 4.5 MiB; the share is 1 MiB, so this agent's state spills.
    let tight = agent(2, Class::Background, 1, budgets(1_000_000, 2048, 1));
    let config = Config {
        kv_slots: 1,
        ..Config::default()
    };
    let mut e = Engine::new(config, &[kai(), tight]);
    let b = e.ask(AgentId(2), 32, 100).unwrap();
    for _ in 0..4 {
        e.step();
    }
    e.ask(KAI, 32, 1).unwrap();
    assert_eq!(e.step(), Event::Move(b, Place::Slot, Place::Spill));
    // A pool of 6 MiB holds one 4.5 MiB snapshot: the second demotes the first.
    let small = Config {
        kv_slots: 1,
        pool_mib: 6,
        ..Config::default()
    };
    let agents = [
        kai(),
        big(2, Class::Background, 1),
        big(3, Class::Background, 1),
    ];
    let mut e = Engine::new(small, &agents);
    let a = e.ask(AgentId(2), 32, 500).unwrap();
    for _ in 0..4 {
        e.step();
    }
    let c = e.ask(AgentId(3), 32, 500).unwrap();
    let f = e.ask(KAI, 32, 1).unwrap();
    // The user's request takes the slot from `a`, whose state fits the pool.
    assert_eq!(e.step(), Event::Move(a, Place::Slot, Place::Ram));
    assert_eq!(e.step(), Event::Move(f, Place::Cold, Place::Slot));
    while e.sched.place(f).is_some() {
        e.step();
    }
    // `c` now runs; when the user comes back, `c`'s state needs the pool `a` holds.
    for _ in 0..40 {
        e.step();
    }
    e.ask(KAI, 32, 1).unwrap();
    let moves: Vec<Event> = (0..4)
        .map(|_| e.step())
        .filter(|ev| matches!(ev, Event::Move(..)))
        .collect();
    assert!(
        moves
            .iter()
            .any(|m| matches!(m, Event::Move(_, Place::Ram, Place::Spill))),
        "{moves:?} (a is {a:?}, c is {c:?})"
    );
}

#[test]
fn a_victim_is_an_idle_holder_first_then_the_lowest_class_then_the_least_recent() {
    let agents = [
        kai(),
        big(2, Class::Background, 1),
        big(3, Class::Background, 1),
        big(4, Class::Index, 1),
    ];
    let mut e = Engine::new(
        Config {
            kv_slots: 3,
            ..Config::default()
        },
        &agents,
    );
    let recent_bg = e.ask(AgentId(2), 32, 50).unwrap();
    let old_bg = e.ask(AgentId(3), 32, 50).unwrap();
    let index = e.ask(AgentId(4), 32, 0).unwrap();
    // Put all three in slots, as if they had run, at different times.
    for (id, last) in [(recent_bg, 900_000), (old_bg, 800_000), (index, 899_000)] {
        let req = e.sched.reqs.get_mut(&id).unwrap();
        req.place = Place::Slot;
        req.prefilled = 32;
        req.last_run_ms = last;
    }
    e.ms = 900_000;
    // None has idled five minutes: the lowest class goes first.
    assert_eq!(e.sched.victim(e.ms), index);
    e.sched.reqs.get_mut(&index).unwrap().last_run_ms = 900_000;
    e.ms = 900_000;
    // Now `old_bg` has held its slot 100 s: not idle yet, so class, then least recent.
    e.sched.reqs.get_mut(&index).unwrap().class = Class::Background;
    assert_eq!(e.sched.victim(e.ms), old_bg);
    // An idle holder goes before a lower class that ran a moment ago.
    e.sched.reqs.get_mut(&index).unwrap().class = Class::Index;
    e.ms = 1_150_000;
    e.sched.reqs.get_mut(&index).unwrap().last_run_ms = 1_149_000;
    e.sched.reqs.get_mut(&recent_bg).unwrap().last_run_ms = 1_149_000;
    assert_eq!(e.sched.victim(e.ms), old_bg);
}

#[test]
fn a_request_over_the_quota_is_refused_before_prefill_and_the_day_is_counted() {
    let b = budgets(100, 128, 64);
    let mut e = Engine::new(
        Config::default(),
        &[kai(), agent(2, Class::Background, 1, b)],
    );
    // 129 positions are over a quota of 128.
    assert_eq!(
        e.ask(AgentId(2), 100, 29),
        Err(Refusal::TooLong {
            positions: 129,
            max: 128
        })
    );
    assert_eq!(e.ask(AgentId(9), 1, 1), Err(Refusal::UnknownAgent));
    assert_eq!(e.ask(AgentId(2), 0, 0), Err(Refusal::Empty));
    e.ask(AgentId(2), 64, 36).unwrap();
    e.run_idle(300);
    assert_eq!(e.sched.used_today(AgentId(2)), Some(100));
    assert_eq!(
        e.ask(AgentId(2), 1, 1),
        Err(Refusal::OverQuota {
            used: 100,
            budget: 100
        })
    );
    // The foreground has no daily budget, and a new day starts the count again.
    assert!(e.ask(KAI, 64, 36).is_ok());
    e.day = 1;
    assert!(e.ask(AgentId(2), 1, 1).is_ok());
    assert_eq!(e.sched.used_today(AgentId(2)), Some(0));
}

#[test]
fn a_block_is_cut_to_what_is_left_of_the_day_and_the_request_ends_over_quota() {
    let b = budgets(40, 2048, 64);
    let mut e = Engine::new(Config::default(), &[agent(2, Class::Background, 1, b)]);
    let r = e.ask(AgentId(2), 64, 0).unwrap();
    assert_eq!(e.step(), Event::Move(r, Place::Cold, Place::Slot));
    assert_eq!(e.step(), Event::Run(vec![(r, Kind::Prefill, 32)]));
    assert_eq!(e.step(), Event::Run(vec![(r, Kind::Prefill, 8)]));
    assert_eq!(
        e.step(),
        Event::End(
            r,
            End::OverQuota {
                used: 40,
                budget: 40
            }
        )
    );
    assert_eq!(e.sched.pending(), 0);
    assert_eq!(e.step(), Event::Idle);
}

#[test]
fn a_raise_keeps_what_the_day_has_used() {
    let mut e = Engine::new(
        Config::default(),
        &[agent(2, Class::Background, 1, budgets(50, 2048, 64))],
    );
    e.ask(AgentId(2), 50, 0).unwrap();
    e.run_idle(20);
    assert_eq!(e.sched.used_today(AgentId(2)), Some(50));
    assert!(matches!(
        e.ask(AgentId(2), 1, 0),
        Err(Refusal::OverQuota { .. })
    ));
    e.sched
        .set_agent(agent(2, Class::Background, 1, budgets(80, 2048, 64)));
    assert_eq!(e.sched.used_today(AgentId(2)), Some(50));
    e.ask(AgentId(2), 30, 0).unwrap();
    e.run_idle(20);
    assert_eq!(e.sched.used_today(AgentId(2)), Some(80));
}

#[test]
fn an_agent_has_at_most_so_many_requests_waiting() {
    let mut e = Engine::new(Config::default(), &[big(2, Class::Background, 1)]);
    for _ in 0..MAX_QUEUED {
        e.ask(AgentId(2), 1, 1).unwrap();
    }
    assert_eq!(e.ask(AgentId(2), 1, 1), Err(Refusal::QueueFull));
}

#[test]
fn deficit_round_robin_shares_tokens_by_weight() {
    let config = Config {
        kv_slots: 2,
        ..Config::default()
    };
    let agents = [big(2, Class::Background, 1), big(3, Class::Background, 3)];
    let mut e = Engine::new(config, &agents);
    e.ask(AgentId(2), 32, 2_000).unwrap();
    e.ask(AgentId(3), 32, 2_000).unwrap();
    while e.ms < 100_000 {
        e.step();
    }
    let light = f64::from(e.sched.used_today(AgentId(2)).unwrap());
    let heavy = f64::from(e.sched.used_today(AgentId(3)).unwrap());
    let ratio = heavy / light;
    assert!((2.4..3.6).contains(&ratio), "{light} {heavy} {ratio}");
}

#[test]
fn a_request_that_has_waited_over_a_minute_goes_to_the_front_of_its_class_and_no_further() {
    let config = Config {
        kv_slots: 2,
        ..Config::default()
    };
    // The heavy agent's visit is so long that the light one would wait close to an hour.
    let agents = [
        big(2, Class::Background, 1_000),
        big(3, Class::Background, 1),
        kai(),
    ];
    let mut e = Engine::new(config, &agents);
    e.ask(AgentId(2), 32, 2_000).unwrap();
    let light = e.ask(AgentId(3), 32, 5).unwrap();
    let mut served_at = None;
    while e.ms < 70_000 && served_at.is_none() {
        if let Event::Run(q) = e.step()
            && q.iter().any(|(r, _, _)| *r == light)
        {
            served_at = Some(e.ms);
        }
    }
    let at = served_at.expect("the light request ran within a minute and a bit");
    assert!(at <= AGING_MS + 2 * PREFILL_MS + RAM_MOVE_MS, "{at}");
    // A foreground request arriving now still goes first.
    let user = e.ask(KAI, 32, 2).unwrap();
    let next = (0..4).find_map(|_| match e.step() {
        Event::Run(q) => Some(q[0].0),
        _ => None,
    });
    assert_eq!(next, Some(user));
}

#[test]
fn serial_decode_runs_one_sequence_and_a_batch_runs_up_to_four_of_one_class() {
    let agents: Vec<Agent> = (2..=7).map(|i| big(i, Class::Background, 1)).collect();
    let mk = |batch| Config {
        kv_slots: 6,
        batch,
        ..Config::default()
    };
    for (batch, widest) in [(Batch::Serial, 1), (Batch::Up(4), 4), (Batch::Up(2), 2)] {
        let mut e = Engine::new(mk(batch), &agents);
        for i in 2..=7 {
            e.ask(AgentId(i), 32, 60).unwrap();
        }
        e.run_idle(2_000);
        let width = e
            .log
            .iter()
            .filter_map(|ev| match ev {
                Event::Run(q) => Some(q.len()),
                _ => None,
            })
            .max()
            .unwrap();
        assert_eq!(width, widest, "{batch:?}");
        let decoded: u32 = runs(&e.log)
            .iter()
            .filter(|(_, k, _)| *k == Kind::Decode)
            .map(|(_, _, t)| t)
            .sum();
        assert_eq!(decoded, 6 * 60, "{batch:?}");
    }
}

#[test]
fn the_foreground_decodes_alone_unless_it_is_allowed_to_batch() {
    let agents = [
        Agent::builtin(AgentId(1), Class::Foreground, 1),
        Agent::builtin(AgentId(2), Class::Foreground, 1),
        big(3, Class::Background, 1),
    ];
    for allowed in [false, true] {
        let mut e = Engine::new(
            Config {
                kv_slots: 4,
                batch: Batch::Up(4),
                batch_foreground: allowed,
                ..Config::default()
            },
            &agents,
        );
        let first = e.ask(AgentId(1), 32, 20).unwrap();
        let second = e.ask(AgentId(2), 32, 20).unwrap();
        let background = e.ask(AgentId(3), 32, 20).unwrap();
        // The others have their state in a slot already, as ones that were prefilled
        // would (the foreground is served one request at a time, oldest first).
        for id in [second, background] {
            let req = e.sched.reqs.get_mut(&id).unwrap();
            req.place = Place::Slot;
            req.prefilled = 32;
        }
        e.run_idle(500);
        let wide: Vec<Vec<RequestId>> = e
            .log
            .iter()
            .filter_map(|ev| match ev {
                Event::Run(q) if q.len() > 1 => Some(q.iter().map(|(r, _, _)| *r).collect()),
                _ => None,
            })
            .collect();
        match allowed {
            // Alone, however ready the others are.
            false => assert!(wide.is_empty(), "{wide:?}"),
            // Together, and the background request is never in the user's batch.
            true => {
                assert!(!wide.is_empty());
                assert!(wide.iter().all(|ids| ids == &[first, second]), "{wide:?}");
            }
        }
    }
}

#[test]
fn a_request_that_runs_over_two_minutes_ends_with_a_timeout() {
    let mut e = Engine::new(Config::default(), &[big(2, Class::Background, 1)]);
    let r = e.ask(AgentId(2), 32, 2_000).unwrap();
    let mut ended = None;
    for _ in 0..3_000 {
        if let Event::End(id, why) = e.step() {
            ended = Some((id, why, e.ms));
            break;
        }
    }
    let (id, why, at) = ended.expect("it timed out");
    assert_eq!((id, why), (r, End::Timeout));
    assert!((REQUEST_MS..REQUEST_MS + 2_000).contains(&at), "{at}");
    assert_eq!(e.sched.pending(), 0);
}

#[test]
fn a_request_that_is_waiting_does_not_run_out_of_time() {
    let agents = [kai(), big(2, Class::Background, 1)];
    let mut e = Engine::new(Config::default(), &agents);
    let b = e.ask(AgentId(2), 32, 20).unwrap();
    // The user's work, one request after another, keeps the model busy for 3 minutes.
    let mut user = None;
    while e.ms < 180_000 {
        if user.is_none_or(|u| e.sched.place(u).is_none()) {
            user = Some(e.ask(KAI, 32, 600).unwrap());
        }
        e.step();
    }
    assert!(e.sched.place(b).is_some(), "it waited, and was not ended");
    assert!(!e.done.contains(&b));
}

#[test]
fn finish_frees_a_slot_and_a_place_in_the_queue() {
    let mut e = Engine::new(
        Config {
            kv_slots: 1,
            ..Config::default()
        },
        &[big(2, Class::Background, 1)],
    );
    let r = e.ask(AgentId(2), 32, 10).unwrap();
    e.step();
    e.step();
    assert_eq!(e.sched.resident(), 1);
    e.sched.finish(r);
    assert_eq!((e.sched.pending(), e.sched.resident()), (0, 0));
    assert_eq!(e.step(), Event::Idle);
}

#[test]
fn a_packaged_agent_takes_its_class_and_budgets_from_its_manifest() {
    let text = "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass interactive\n\
        model tokens/day=50000 kv=1024 pool=64m\nactions fs_list\nmemory none\nnet none\n";
    let manifest = kaiagent::Manifest::parse(text).unwrap();
    let a = Agent::packaged(AgentId(7), &manifest, 0);
    assert_eq!(a.class, Class::Interactive);
    assert_eq!(a.weight, 1, "a weight is at least 1");
    assert_eq!(a.budgets, Some(budgets(50_000, 1_024, 64)));
    let background = kaiagent::Manifest::parse(&text.replace("interactive", "background")).unwrap();
    assert_eq!(
        Agent::packaged(AgentId(8), &background, 2).class,
        Class::Background
    );
    // And the scheduler holds it to them.
    let mut e = Engine::new(Config::default(), &[a]);
    assert_eq!(
        e.ask(AgentId(7), 1_000, 100),
        Err(Refusal::TooLong {
            positions: 1_100,
            max: 1_024
        })
    );
}
