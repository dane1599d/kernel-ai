//! The scheduler's state and its one question, [`Scheduler::next`].

use crate::{
    Agent, AgentId, Ask, BLOCK_TOKENS, Class, Config, Decision, End, KV_BYTES_PER_POSITION, Kind,
    MAX_QUEUED, Now, Place, Quantum, REQUEST_MS, Refusal, RequestId,
};
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

/// What the scheduler knows of an agent.
pub(crate) struct AgentState {
    pub(crate) agent: Agent,
    pub(crate) used_today: u32,
    /// Tokens the agent may still be served in its deficit round robin visit.
    pub(crate) deficit: i64,
}

/// A request waiting or running.
pub(crate) struct Req {
    pub(crate) agent: AgentId,
    pub(crate) class: Class,
    /// Tokens to prefill, then the tokens it has decoded so far.
    pub(crate) prompt: u32,
    pub(crate) made: u32,
    pub(crate) max_new: u32,
    /// KV positions its state holds: all of `prompt + made` when it can decode.
    pub(crate) prefilled: u32,
    pub(crate) arrived_ms: u64,
    /// Since when it has waited without a quantum.
    pub(crate) waiting_since_ms: u64,
    pub(crate) last_run_ms: u64,
    /// Time it has run, counted from the caller's calls.
    pub(crate) active_ms: u64,
    pub(crate) place: Place,
}

impl Req {
    /// Positions that must be in KV before it decodes.
    pub(crate) fn target(&self) -> u32 {
        self.prompt + self.made
    }

    pub(crate) fn decoding(&self) -> bool {
        self.prefilled >= self.target()
    }

    /// The tokens its next quantum would process, before any budget cuts it.
    pub(crate) fn cost(&self) -> u32 {
        match self.decoding() {
            true => 1,
            false => (self.target() - self.prefilled).min(BLOCK_TOKENS),
        }
    }

    /// Bytes of KV its state holds.
    pub(crate) fn bytes(&self) -> u64 {
        u64::from(self.prefilled) * KV_BYTES_PER_POSITION
    }
}

/// How the next request was chosen: only a fair pick pays the agent's deficit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pick {
    Foreground,
    Aged,
    Fair,
}

/// Where each class's deficit round robin visit stands.
#[derive(Clone, Copy, Default)]
pub(crate) struct Visit {
    /// The agent being served, if a visit is under way.
    pub(crate) at: Option<AgentId>,
    /// The agent last visited, whose successor is next.
    pub(crate) last: Option<AgentId>,
}

/// The model scheduler's policy. See the crate docs.
pub struct Scheduler {
    pub(crate) config: Config,
    pub(crate) agents: BTreeMap<AgentId, AgentState>,
    pub(crate) reqs: BTreeMap<RequestId, Req>,
    next_request: u64,
    day: Option<u32>,
    /// When the last call was made and what it ran, to count running time.
    last_call: Option<(u64, Vec<RequestId>)>,
    pub(crate) visits: [Visit; 4],
}

impl Scheduler {
    pub fn new(config: Config) -> Self {
        let config = Config {
            kv_slots: config.kv_slots.max(1),
            ..config
        };
        Self {
            config,
            agents: BTreeMap::new(),
            reqs: BTreeMap::new(),
            next_request: 1,
            day: None,
            last_call: None,
            visits: [Visit::default(); 4],
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Adds `agent`, or replaces its class, weight and budgets (a raise, or a
    /// lowering) and keeps what it has used today.
    pub fn set_agent(&mut self, agent: Agent) {
        let agent = Agent {
            weight: agent.weight.max(1),
            ..agent
        };
        match self.agents.get_mut(&agent.id) {
            Some(state) => state.agent = agent,
            None => {
                self.agents.insert(
                    agent.id,
                    AgentState {
                        agent,
                        used_today: 0,
                        deficit: 0,
                    },
                );
            }
        }
    }

    /// Tokens `agent` has used today, if it is known.
    pub fn used_today(&self, agent: AgentId) -> Option<u32> {
        self.agents.get(&agent).map(|a| a.used_today)
    }

    /// Where `request`'s state is, if it is still here.
    pub fn place(&self, request: RequestId) -> Option<Place> {
        self.reqs.get(&request).map(|r| r.place)
    }

    /// Requests waiting or running.
    pub fn pending(&self) -> usize {
        self.reqs.len()
    }

    /// Requests whose KV is in a resident slot.
    pub fn resident(&self) -> usize {
        self.reqs
            .values()
            .filter(|r| r.place == Place::Slot)
            .count()
    }

    /// Bytes of RAM snapshots held: all, or `agent`'s.
    pub fn ram_bytes(&self, agent: Option<AgentId>) -> u64 {
        self.reqs
            .values()
            .filter(|r| r.place == Place::Ram && agent.is_none_or(|a| a == r.agent))
            .map(Req::bytes)
            .sum()
    }

    /// A request from `ask`, held to the agent's budgets: refused here, before any
    /// prefill, when it could never run.
    pub fn submit(&mut self, ask: Ask, now: Now) -> Result<RequestId, Refusal> {
        self.roll_day(now.day);
        let state = self.agents.get(&ask.agent).ok_or(Refusal::UnknownAgent)?;
        if ask.prompt_tokens == 0 && ask.max_new_tokens == 0 {
            return Err(Refusal::Empty);
        }
        let positions = ask.prompt_tokens.saturating_add(ask.max_new_tokens);
        let max = state
            .agent
            .budgets
            .map_or(self.config.max_positions, |b| b.kv_positions)
            .min(self.config.max_positions);
        if positions > max {
            return Err(Refusal::TooLong { positions, max });
        }
        if let Some(budget) = state.agent.budgets.map(|b| b.tokens_per_day)
            && state.used_today >= budget
        {
            return Err(Refusal::OverQuota {
                used: state.used_today,
                budget,
            });
        }
        let queued = self.reqs.values().filter(|r| r.agent == ask.agent).count();
        if queued >= MAX_QUEUED {
            return Err(Refusal::QueueFull);
        }
        let id = RequestId(self.next_request);
        self.next_request += 1;
        self.reqs.insert(
            id,
            Req {
                agent: ask.agent,
                class: state.agent.class,
                prompt: ask.prompt_tokens,
                made: 0,
                max_new: ask.max_new_tokens,
                prefilled: 0,
                arrived_ms: now.ms,
                waiting_since_ms: now.ms,
                last_run_ms: now.ms,
                active_ms: 0,
                place: Place::Cold,
            },
        );
        Ok(id)
    }

    /// `request` is over before its last token (the model stopped, or the agent
    /// cancelled it): its slot, snapshot and place in the queue are free.
    pub fn finish(&mut self, request: RequestId) {
        self.remove(request);
    }

    /// The next thing to do. Each call is a safe point: apply what it says, then ask
    /// again.
    pub fn next(&mut self, now: Now) -> Decision {
        self.roll_day(now.day);
        self.count_running(now.ms);
        if let Some(end) = self.expired() {
            return end;
        }
        let Some((lead, pick)) = self.lead(now.ms) else {
            self.last_call = Some((now.ms, Vec::new()));
            return Decision::Idle;
        };
        if self.reqs[&lead].place != Place::Slot {
            self.last_call = Some((now.ms, Vec::new()));
            return self.make_resident(lead, now.ms);
        }
        self.run(lead, pick, now.ms)
    }

    /// A new day: every agent's count starts again.
    fn roll_day(&mut self, day: u32) {
        if self.day != Some(day) {
            self.day = Some(day);
            for state in self.agents.values_mut() {
                state.used_today = 0;
            }
        }
    }

    /// What ran since the last call has run for that long.
    fn count_running(&mut self, now_ms: u64) {
        let Some((at, ran)) = self.last_call.take() else {
            return;
        };
        let elapsed = now_ms.saturating_sub(at);
        for id in ran {
            if let Some(req) = self.reqs.get_mut(&id) {
                req.active_ms += elapsed;
            }
        }
    }

    /// A request that is over: out of time, or its agent's day is spent.
    fn expired(&mut self) -> Option<Decision> {
        let (request, agent, why) = self.reqs.iter().find_map(|(id, req)| {
            if req.active_ms > REQUEST_MS {
                return Some((*id, req.agent, End::Timeout));
            }
            let state = &self.agents[&req.agent];
            let budget = state.agent.budgets?.tokens_per_day;
            (state.used_today >= budget).then_some((
                *id,
                req.agent,
                End::OverQuota {
                    used: state.used_today,
                    budget,
                },
            ))
        })?;
        self.remove(request);
        Some(Decision::End {
            request,
            agent,
            why,
        })
    }

    /// Runs `lead`'s next quantum, with the sequences that decode beside it.
    fn run(&mut self, lead: RequestId, pick: Pick, now_ms: u64) -> Decision {
        let mut batch = vec![lead];
        if self.reqs[&lead].decoding() {
            batch.extend(self.companions(lead));
        }
        let mut quanta = Vec::with_capacity(batch.len());
        for id in &batch {
            // Only the lead of a fair pick pays its deficit: a batch rides along free.
            let pays = pick == Pick::Fair && *id == lead;
            let (kind, tokens, last) = self.charge(*id, pays, now_ms);
            let agent = self.reqs.get(id).map_or(AgentId(0), |r| r.agent);
            quanta.push(Quantum {
                request: *id,
                agent,
                kind,
                tokens,
                last,
            });
        }
        for quantum in quanta.iter().filter(|q| q.last) {
            self.remove(quantum.request);
        }
        self.last_call = Some((now_ms, batch));
        Decision::Run(quanta)
    }

    /// One quantum for `id`: its agent pays the tokens, and the request moves on.
    /// What it was, how many tokens, and whether it is the request's last.
    fn charge(&mut self, id: RequestId, pays: bool, now_ms: u64) -> (Kind, u32, bool) {
        let req = &self.reqs[&id];
        let (kind, nominal) = (
            match req.decoding() {
                true => Kind::Decode,
                false => Kind::Prefill,
            },
            req.cost(),
        );
        let state = self
            .agents
            .get_mut(&req.agent)
            .expect("a request has an agent");
        // Never past the day's budget: a block is cut to what is left.
        let room = state.agent.budgets.map_or(u32::MAX, |b| {
            b.tokens_per_day.saturating_sub(state.used_today)
        });
        let tokens = nominal.min(room);
        state.used_today += tokens;
        if pays {
            state.deficit -= i64::from(tokens);
        }
        let req = self.reqs.get_mut(&id).expect("just read");
        req.waiting_since_ms = now_ms;
        req.last_run_ms = now_ms;
        req.prefilled += tokens;
        if kind == Kind::Decode {
            req.made += tokens;
        }
        let last = req.decoding() && req.made >= req.max_new;
        (kind, tokens, last)
    }

    /// Takes `request` out: its slot and snapshot are free, and an agent with
    /// nothing else in its class starts its next visit with no deficit.
    pub(crate) fn remove(&mut self, request: RequestId) {
        let Some(req) = self.reqs.remove(&request) else {
            return;
        };
        if !self
            .reqs
            .values()
            .any(|r| r.agent == req.agent && r.class == req.class)
            && let Some(state) = self.agents.get_mut(&req.agent)
        {
            state.deficit = 0;
        }
    }
}
