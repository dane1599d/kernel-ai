//! Where a request's KV state lives (ADR 0013 section 2): a resident slot, a RAM
//! snapshot in the bounded pool, a spill file, or nowhere. A request runs only in a
//! slot; getting it one is a [`Decision::Move`] at a time.

use crate::sched::Scheduler;
use crate::{Class, Decision, IDLE_HOLD_MS, Place, RequestId};

/// What to do with a state that has to leave its slot.
enum Plan {
    /// Move it here.
    To(Place),
    /// Make room in the pool first: this snapshot goes to a spill file or is dropped.
    Demote(RequestId),
}

impl Scheduler {
    /// A slot for `lead`, which is to run: its state comes in if a slot is free, else
    /// a victim's state goes out first. One move per call.
    pub(crate) fn make_resident(&mut self, lead: RequestId, now_ms: u64) -> Decision {
        if self.resident() < self.config.kv_slots {
            let from = self.set_place(lead, Place::Slot);
            return Decision::Move {
                request: lead,
                from,
                to: Place::Slot,
            };
        }
        let victim = self.victim(now_ms);
        match self.plan_park(victim) {
            Plan::To(to) => {
                self.set_place(victim, to);
                Decision::Move {
                    request: victim,
                    from: Place::Slot,
                    to,
                }
            }
            Plan::Demote(snapshot) => {
                let to = self.beyond_ram();
                self.set_place(snapshot, to);
                Decision::Move {
                    request: snapshot,
                    from: Place::Ram,
                    to,
                }
            }
        }
    }

    /// Puts `request`'s state in `to`: where it was. A dropped state holds nothing, so
    /// the request prefills again before it decodes.
    fn set_place(&mut self, request: RequestId, to: Place) -> Place {
        let req = self
            .reqs
            .get_mut(&request)
            .expect("a moved state is a request's");
        let from = req.place;
        req.place = to;
        if to == Place::Cold {
            req.prefilled = 0;
        }
        from
    }

    /// The resident request to give up: one that has held its slot idle for
    /// [`IDLE_HOLD_MS`] first, then the lowest class, then the least recently run.
    pub(crate) fn victim(&self, now_ms: u64) -> RequestId {
        self.reqs
            .iter()
            .filter(|(_, r)| r.place == Place::Slot)
            .min_by_key(|(id, r)| {
                let idle = now_ms.saturating_sub(r.last_run_ms) >= IDLE_HOLD_MS;
                (
                    core::cmp::Reverse(idle),
                    core::cmp::Reverse(r.class),
                    r.last_run_ms,
                    **id,
                )
            })
            .map(|(id, _)| *id)
            .expect("every slot is full, so one is held")
    }

    /// Where `victim`'s state goes. Nothing computed is nothing to keep. An agent
    /// over its pool share spills first; a full pool demotes its lowest-priority
    /// snapshot, which is never above the victim's own class.
    fn plan_park(&self, victim: RequestId) -> Plan {
        let req = &self.reqs[&victim];
        let bytes = req.bytes();
        if bytes == 0 {
            return Plan::To(Place::Cold);
        }
        let share = self.agents[&req.agent]
            .agent
            .budgets
            .map(|b| u64::from(b.pool_mib) << 20);
        if share.is_some_and(|share| self.ram_bytes(Some(req.agent)) + bytes > share) {
            return Plan::To(self.beyond_ram());
        }
        let pool = u64::from(self.config.pool_mib) << 20;
        if self.ram_bytes(None) + bytes <= pool {
            return Plan::To(Place::Ram);
        }
        match self.demotable(req.class, victim) {
            Some(snapshot) => Plan::Demote(snapshot),
            None => Plan::To(self.beyond_ram()),
        }
    }

    /// The RAM snapshot that gives way to a state of class `class`: the lowest class,
    /// the least recently run, never of a higher class than `class`.
    fn demotable(&self, class: Class, victim: RequestId) -> Option<RequestId> {
        self.reqs
            .iter()
            .filter(|(id, r)| r.place == Place::Ram && r.class >= class && **id != victim)
            .min_by_key(|(id, r)| (core::cmp::Reverse(r.class), r.last_run_ms, **id))
            .map(|(id, _)| *id)
    }

    /// Where a state goes that does not fit in RAM.
    fn beyond_ram(&self) -> Place {
        match self.config.spill {
            true => Place::Spill,
            false => Place::Cold,
        }
    }
}
