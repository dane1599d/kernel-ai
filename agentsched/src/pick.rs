//! Who gets the next quantum: the highest class with anything waiting, then within
//! it the foreground's oldest, or aging, then deficit round robin over tokens.

use crate::sched::{Pick, Scheduler};
use crate::{AGING_MS, AgentId, BLOCK_TOKENS, Batch, Class, MAX_BATCH, Place, RequestId};
use alloc::vec::Vec;

impl Scheduler {
    /// The request that gets the next quantum, and how it was chosen, if any waits.
    pub(crate) fn lead(&mut self, now_ms: u64) -> Option<(RequestId, Pick)> {
        for class in Class::ALL {
            if !self.reqs.values().any(|r| r.class == class) {
                continue;
            }
            // The user's own requests go in the order they were made.
            if class == Class::Foreground {
                return self
                    .reqs
                    .iter()
                    .filter(|(_, r)| r.class == class)
                    .min_by_key(|(id, r)| (r.arrived_ms, **id))
                    .map(|(id, _)| (*id, Pick::Foreground));
            }
            // Aging: one that has waited over a minute goes to the front of its
            // class, the longest waiter first, and never above a higher class.
            let aged = self
                .reqs
                .iter()
                .filter(|(_, r)| {
                    r.class == class && now_ms.saturating_sub(r.waiting_since_ms) > AGING_MS
                })
                .min_by_key(|(id, r)| (r.waiting_since_ms, **id))
                .map(|(id, _)| *id);
            if aged.is_some() {
                return aged.map(|id| (id, Pick::Aged));
            }
            return self.round_robin(class).map(|id| (id, Pick::Fair));
        }
        None
    }

    /// The oldest request `agent` has in `class`.
    fn head(&self, agent: AgentId, class: Class) -> Option<RequestId> {
        self.reqs
            .iter()
            .filter(|(_, r)| r.agent == agent && r.class == class)
            .min_by_key(|(id, r)| (r.arrived_ms, **id))
            .map(|(id, _)| *id)
    }

    /// Deficit round robin over tokens: each visit gives an agent its weight times
    /// a block, and the agent is served while that covers its next quantum. Agents
    /// are visited in id order, so the order is the same on every run.
    fn round_robin(&mut self, class: Class) -> Option<RequestId> {
        let at = class.index();
        let ring: Vec<AgentId> = self
            .agents
            .keys()
            .copied()
            .filter(|a| {
                self.reqs
                    .values()
                    .any(|r| r.agent == *a && r.class == class)
            })
            .collect();
        if ring.is_empty() {
            self.visits[at] = Default::default();
            return None;
        }
        // A visit refills at least a block, which covers any quantum, so the second
        // turn of this loop at the latest serves someone.
        for _ in 0..ring.len() * 2 + 2 {
            let agent = match self.visits[at].at.filter(|a| ring.contains(a)) {
                Some(agent) => agent,
                None => {
                    let last = self.visits[at].last;
                    let next = ring
                        .iter()
                        .copied()
                        .find(|a| Some(*a) > last)
                        .unwrap_or(ring[0]);
                    let state = self.agents.get_mut(&next).expect("a ring agent is known");
                    state.deficit += i64::from(state.agent.weight) * i64::from(BLOCK_TOKENS);
                    self.visits[at].at = Some(next);
                    self.visits[at].last = Some(next);
                    next
                }
            };
            let head = self.head(agent, class)?;
            if self.agents[&agent].deficit >= i64::from(self.reqs[&head].cost()) {
                return Some(head);
            }
            self.visits[at].at = None;
        }
        None
    }

    /// The sequences that decode beside `lead` when batching is on: another agent's
    /// request of the same class, resident and ready to decode, with a token of its
    /// day left. At most 4 sequences in all, and the foreground only if allowed.
    pub(crate) fn companions(&self, lead: RequestId) -> Vec<RequestId> {
        let Batch::Up(most) = self.config.batch else {
            return Vec::new();
        };
        let lead = &self.reqs[&lead];
        if lead.class == Class::Foreground && !self.config.batch_foreground {
            return Vec::new();
        }
        let room = usize::from(most.clamp(2, MAX_BATCH)) - 1;
        // After the lead's agent in id order, so the same agent is not always first.
        let after = self.agents.keys().filter(|a| **a > lead.agent);
        let before = self.agents.keys().filter(|a| **a < lead.agent);
        let mut found = Vec::new();
        for agent in after.chain(before) {
            if found.len() == room {
                break;
            }
            let state = &self.agents[agent];
            let budgeted = state
                .agent
                .budgets
                .is_none_or(|b| state.used_today < b.tokens_per_day);
            let ready = self
                .reqs
                .iter()
                .filter(|(_, r)| {
                    r.agent == *agent
                        && r.class == lead.class
                        && r.place == Place::Slot
                        && r.decoding()
                })
                .min_by_key(|(id, r)| (r.arrived_ms, **id));
            if let (true, Some((id, _))) = (budgeted, ready) {
                found.push(*id);
            }
        }
        found
    }
}
