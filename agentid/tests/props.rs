//! The roster, the door, the questions and the log's name under random sequences
//! (ADR 0013 section 1). A model of who is on the roster and what is asked is kept
//! beside the real ones, every call's answer is held to the model's, and after each
//! step the rules that make an identity are checked: a badge is never handed out twice,
//! a caller is live only while its agent is, an agent's mark is never a person's, a yes
//! is spent once and only on the call it was for.
//!
//! The generator is splitmix64 on a seed, so a failure names the seed that makes it.

use agentabi::{ROWS, Tier as RowTier};
use agentid::asks::{KAI_ROOM, MAX_OPEN, MAX_PER_AGENT};
use agentid::stamp::KEY;
use agentid::{
    Admission, AgentId, AnswerError, Answered, AskError, Asks, Caller, Denied, InstallError, Line,
    MAX_PACKAGED, Mark, Name, Roster, Source, read_field,
};
use kaiagent::Manifest;

const NOTES: &str = "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass background\n\
    model tokens/day=50000 kv=1024 pool=64m\nactions fs_list fs_read\n\
    memory none\nnet none\n";

/// The rows an agent may be granted in these runs.
const ACTIONS: [&str; 3] = ["fs_list", "fs_read", "fs_create"];
/// More names than the roster has room for, so a full roster comes up.
const NAMES: usize = 40;
/// The entry numbers that questions are asked under, few enough to collide.
const ENTRIES: u64 = 24;
const SHOWN: [&str; 3] = ["fs_read a.txt", "fs_read b.txt", "fs_create c.txt"];
const SEEDS: u64 = 300;
const STEPS: usize = 220;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// The person's own keyboard: this test stands in for the input path.
fn person() -> Source {
    // SAFETY: a test is the input path here.
    unsafe { Source::hardware() }
}

fn manifest(id: &str, mask: usize) -> Manifest {
    let granted: Vec<&str> = ACTIONS
        .iter()
        .enumerate()
        .filter(|(at, _)| mask & (1 << at) != 0)
        .map(|(_, row)| *row)
        .collect();
    let text = NOTES.replace("fs_list fs_read", &granted.join(" "));
    let mut manifest = Manifest::parse(&text).unwrap();
    manifest.id = id.to_string();
    manifest
}

/// What the model knows of an agent on the roster.
#[derive(Clone)]
struct Live {
    id: AgentId,
    name: String,
    /// Which of `ACTIONS` a packaged agent holds; `None`, a built-in.
    mask: Option<usize>,
}

impl Live {
    fn is_kai(&self) -> bool {
        self.id.badge() == 1
    }

    fn holds(&self, row: &str) -> bool {
        match self.mask {
            _ if self.is_kai() => true,
            None => false,
            Some(mask) => ACTIONS
                .iter()
                .enumerate()
                .any(|(at, held)| *held == row && mask & (1 << at) != 0),
        }
    }
}

/// A question the model holds open.
struct Open {
    entry: u64,
    admission: Admission,
    shown: &'static str,
}

/// An answer waiting to be spent, with what it was for.
struct Held {
    answered: Answered,
    admission: Admission,
    shown: &'static str,
}

struct World {
    roster: Roster,
    live: Vec<Live>,
    /// Every id ever handed out, retired or not.
    ids: Vec<AgentId>,
    callers: Vec<Caller>,
    admissions: Vec<Admission>,
    asks: Asks,
    open: Vec<Open>,
    held: Vec<Held>,
    never: Option<&'static str>,
}

/// Whether `text` is an agent's name, by the rule the module states: 2 to 12 bytes of
/// `a-z`, `0-9` and `-`, starting with a letter.
fn shaped(text: &str) -> bool {
    let bytes = text.as_bytes();
    (2..=12).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

impl World {
    fn new() -> Self {
        let roster = Roster::new();
        let live: Vec<Live> = roster
            .iter()
            .map(|agent| Live {
                id: agent.id(),
                name: agent.name().to_string(),
                mask: None,
            })
            .collect();
        let ids = live.iter().map(|agent| agent.id).collect();
        let never = ROWS
            .iter()
            .find(|row| agentabi::derive(row) == RowTier::Never)
            .map(|row| row.name);
        Self {
            roster,
            live,
            ids,
            callers: Vec::new(),
            admissions: Vec::new(),
            asks: Asks::new(),
            open: Vec::new(),
            held: Vec::new(),
            never,
        }
    }

    fn model(&self, id: AgentId) -> Option<&Live> {
        self.live.iter().find(|agent| agent.id == id)
    }

    fn any_id(&self, rng: &mut Rng) -> AgentId {
        self.ids[rng.below(self.ids.len())]
    }

    fn live_id(&self, rng: &mut Rng) -> AgentId {
        self.live[rng.below(self.live.len())].id
    }

    /// Whether `caller` is the agent the model has live under its badge and name.
    fn caller_is_live(&self, caller: &Caller) -> bool {
        self.model(caller.id())
            .is_some_and(|agent| agent.name == caller.name().as_str())
    }

    fn admission_holds(&self, admission: &Admission) -> bool {
        self.model(admission.caller().id()).is_some_and(|agent| {
            agent.name == admission.caller().name().as_str() && agent.holds(admission.row())
        })
    }

    fn install(&mut self, rng: &mut Rng) {
        let name = match rng.below(10) {
            0 => ["kai", "goals", "index", "maint"][rng.below(4)].to_string(),
            1 => ["Bad Name", "x", "ag=1", "a\nb", "-dash"][rng.below(5)].to_string(),
            _ => format!("ag-{}", rng.below(NAMES)),
        };
        let mask = 1 + rng.below(7);
        let packaged = self
            .live
            .iter()
            .filter(|agent| agent.mask.is_some())
            .count();
        let taken = self.live.iter().any(|agent| agent.name == name);
        let got = self.roster.install(manifest(&name, mask));
        match got {
            Err(InstallError::BadName) => assert!(!shaped(&name), "{name} is shaped"),
            Err(InstallError::Taken(_)) => assert!(shaped(&name) && taken, "{name} is free"),
            Err(InstallError::Full) => {
                assert!(shaped(&name) && !taken && packaged >= MAX_PACKAGED);
            }
            Ok(id) => {
                assert!(shaped(&name) && !taken && packaged < MAX_PACKAGED, "{name}");
                assert!(
                    self.ids.iter().all(|old| old.badge() < id.badge()),
                    "badge {} was handed out before or out of order",
                    id.badge()
                );
                self.ids.push(id);
                self.live.push(Live {
                    id,
                    name,
                    mask: Some(mask),
                });
            }
            Err(other) => panic!("install of {name} refused as {other:?}"),
        }
    }

    fn upgrade(&mut self, rng: &mut Rng) {
        let id = self.any_id(rng);
        let same = rng.chance(80);
        let mask = 1 + rng.below(7);
        let known = self.model(id).cloned();
        let name = match (&known, same) {
            (Some(agent), true) => agent.name.clone(),
            _ => "other-name".to_string(),
        };
        let got = self.roster.upgrade(id, manifest(&name, mask));
        match (known, got) {
            (Some(agent), Ok(())) if agent.mask.is_some() => {
                assert!(same, "an upgrade changed the name");
                if let Some(at) = self.live.iter().position(|live| live.id == id) {
                    self.live[at].mask = Some(mask);
                }
            }
            (Some(agent), Err(InstallError::NameChanged)) => {
                assert!(agent.mask.is_some() && !same);
            }
            (known, Err(InstallError::NotPackaged)) => {
                assert!(known.is_none_or(|agent| agent.mask.is_none()));
            }
            (_, other) => panic!("upgrade of {} answered {other:?}", id.badge()),
        }
    }

    fn retire(&mut self, rng: &mut Rng) {
        let id = self.any_id(rng);
        let known = self.model(id).cloned();
        match (known, self.roster.retire(id)) {
            (Some(agent), Ok(gone)) if agent.mask.is_some() => {
                assert_eq!(gone.name().as_str(), agent.name);
                self.live.retain(|live| live.id != id);
                // As the supervisor does when it retires an agent.
                self.asks.withdraw_agent(id);
                self.open.retain(|open| open.admission.caller().id() != id);
            }
            (known, Err(InstallError::NotPackaged)) => {
                assert!(known.is_none_or(|agent| agent.mask.is_none()));
            }
            (_, other) => panic!("retire of {} answered {other:?}", id.badge()),
        }
    }

    fn make_caller(&mut self, rng: &mut Rng) {
        let top = self.ids.iter().map(|id| id.badge()).max().unwrap_or(0);
        let badge = rng.below(top as usize + 3) as u32;
        let live = self.live.iter().any(|agent| agent.id.badge() == badge);
        match self.roster.caller(badge) {
            Ok(caller) => {
                assert!(live, "badge {badge} is not on the roster");
                assert_eq!(caller.id().badge(), badge);
                self.callers.push(caller);
            }
            Err(refused) => {
                assert!(!live, "badge {badge} is on the roster");
                assert_eq!(refused.0, badge);
            }
        }
    }

    fn row(&self, rng: &mut Rng) -> &'static str {
        match rng.below(8) {
            0 => "no_such_row",
            1 => self.never.unwrap_or("fs_list"),
            _ => ACTIONS[rng.below(ACTIONS.len())],
        }
    }

    fn admit(&mut self, rng: &mut Rng) {
        if self.callers.is_empty() {
            return;
        }
        let caller = self.callers[rng.below(self.callers.len())].clone();
        let row = self.row(rng);
        let live = self.caller_is_live(&caller);
        let got = self.roster.admit(&caller, row);
        let action = agentabi::find(&ROWS, row);
        match got {
            Err(Denied::Retired) => assert!(!live, "a live caller was retired at the door"),
            Err(Denied::UnknownRow) => assert!(live && action.is_none()),
            Err(Denied::Never) => {
                assert!(live && action.is_some_and(|a| agentabi::derive(a) == RowTier::Never));
            }
            Err(Denied::OutsideGrants) => {
                let agent = self.model(caller.id()).unwrap();
                assert!(live && !agent.holds(row), "{} holds {row}", agent.name);
            }
            Ok(admission) => {
                let agent = self.model(caller.id()).unwrap();
                assert!(
                    live && agent.holds(row),
                    "{} does not hold {row}",
                    agent.name
                );
                assert_eq!(admission.caller(), &caller);
                // The entry is stamped with the caller's name, and reads back as it.
                let field = admission.stamp().field();
                assert_eq!(field, format!("{KEY}={}", caller.name()));
                assert_eq!(read_field(Some(field.as_str())), Ok(caller.name().clone()));
                self.admissions.push(admission);
            }
        }
    }

    fn ask(&mut self, rng: &mut Rng) {
        if self.admissions.is_empty() {
            return;
        }
        let admission = self.admissions[rng.below(self.admissions.len())].clone();
        let entry = rng.next() % ENTRIES;
        let shown = SHOWN[rng.below(SHOWN.len())];
        let kai = admission.caller().is_kai();
        let mine = self
            .open
            .iter()
            .filter(|open| open.admission.caller().id() == admission.caller().id())
            .count();
        let others = self
            .open
            .iter()
            .filter(|open| !open.admission.caller().is_kai())
            .count();
        let expected = if !self.caller_is_live(admission.caller()) {
            Err(AskError::Retired)
        } else if self.open.iter().any(|open| open.entry == entry) {
            Err(AskError::Duplicate)
        } else if kai && self.open.len() >= MAX_OPEN {
            Err(AskError::Full)
        } else if !kai && mine >= MAX_PER_AGENT {
            Err(AskError::TooMany)
        } else if !kai && (others >= MAX_OPEN - KAI_ROOM || self.open.len() >= MAX_OPEN) {
            Err(AskError::Full)
        } else {
            Ok(())
        };
        let got = self.asks.ask(&self.roster, entry, &admission, shown);
        assert_eq!(got, expected, "ask of entry {entry}");
        if got.is_ok() {
            self.open.push(Open {
                entry,
                admission,
                shown,
            });
        }
    }

    fn answer(&mut self, rng: &mut Rng) {
        let entry = rng.next() % ENTRIES;
        let yes = rng.chance(70);
        let by_person = rng.chance(50);
        let mark = match by_person {
            true => Mark::of(person()),
            false => Mark::of(Source::agent(self.live_id(rng))),
        };
        assert_eq!(mark.is_person(), by_person);
        let at = self.open.iter().position(|open| open.entry == entry);
        let got = self.asks.answer(entry, yes, mark);
        match (by_person, at, got) {
            (false, _, Err(AnswerError::NotThePerson)) => {
                // The refusal left the question as it was, and said nothing of which are open.
                assert_eq!(self.asks.is_open(entry), at.is_some());
            }
            (true, None, Err(AnswerError::NothingAsked)) => {}
            (true, Some(at), Ok(answered)) => {
                assert_eq!(answered.entry(), entry);
                assert_eq!(answered.yes(), yes);
                let open = self.open.remove(at);
                self.held.push(Held {
                    answered,
                    admission: open.admission,
                    shown: open.shown,
                });
            }
            (_, _, other) => panic!("answer of {entry} by person {by_person}: {other:?}"),
        }
    }

    fn spend(&mut self, rng: &mut Rng) {
        if self.held.is_empty() {
            return;
        }
        let held = self.held.remove(rng.below(self.held.len()));
        let other = self.admissions[rng.below(self.admissions.len())].clone();
        let entry = held.answered.entry();
        let yes = held.answered.yes();
        let variant = rng.below(4);
        let (admission, at, shown) = match variant {
            0 => (held.admission.clone(), entry, held.shown.to_string()),
            1 => (held.admission.clone(), entry + 1, held.shown.to_string()),
            2 => (held.admission.clone(), entry, format!("{}x", held.shown)),
            _ => (other, entry, held.shown.to_string()),
        };
        // The yes is for the agent, the row, the entry and the words it was asked for,
        // while that agent is on the roster with grants for the row.
        let same = admission.caller().id() == held.admission.caller().id()
            && admission.row() == held.admission.row()
            && at == entry
            && shown == held.shown;
        let expected = yes && same && self.admission_holds(&admission);
        let got = held.answered.spend(&self.roster, &admission, at, &shown);
        assert_eq!(got, expected, "spend variant {variant}, yes {yes}");
    }

    fn withdraw(&mut self, rng: &mut Rng) {
        let entry = rng.next() % ENTRIES;
        let at = self.open.iter().position(|open| open.entry == entry);
        assert_eq!(self.asks.withdraw(entry), at.is_some());
        if let Some(at) = at {
            self.open.remove(at);
        }
    }

    fn consent(&self, rng: &mut Rng) {
        if self.admissions.is_empty() {
            return;
        }
        let admission = &self.admissions[rng.below(self.admissions.len())];
        let text = ["delete the old files", "yes", ""][rng.below(3)];
        let by_person = rng.chance(50);
        let source = match by_person {
            true => person(),
            false => Source::agent(self.live_id(rng)),
        };
        let words = admission.consent_words(&Line::new(text, source));
        // Only kai, and only from a person; never a name that says it is kai.
        let expected = match admission.caller().is_kai() && by_person {
            true => text,
            false => "",
        };
        assert_eq!(words, expected);
    }

    /// Text made to split a record or pass for a name.
    fn name_text(&self, rng: &mut Rng) {
        const PARTS: [&str; 14] = [
            "ag",
            "kai",
            "-",
            "9",
            "A",
            "=",
            " ",
            "\n",
            "agent=",
            "\u{e9}",
            "x",
            "goals",
            "\t",
            "abcdefghijklm",
        ];
        let text: String = (0..rng.below(7))
            .map(|_| PARTS[rng.below(PARTS.len())])
            .collect();
        let name = Name::new(&text);
        assert_eq!(name.is_ok(), shaped(&text), "{text:?}");
        if let Ok(name) = name {
            assert!(!name.as_str().contains([' ', '=', '\n', '\r', '\t']));
            let field = format!("{KEY}={name}");
            assert_eq!(read_field(Some(field.as_str())), Ok(name));
        }
        // A word that is not `agent=<name>` is no agent's field.
        if let Ok(name) = read_field(Some(text.as_str())) {
            assert!(
                text.starts_with("agent=") && shaped(name.as_str()),
                "{text:?}"
            );
        }
    }

    /// What must be true after every step.
    fn check(&self) {
        let roster: Vec<(u32, String)> = self
            .roster
            .iter()
            .map(|agent| (agent.id().badge(), agent.name().to_string()))
            .collect();
        let mut badges: Vec<u32> = roster.iter().map(|(badge, _)| *badge).collect();
        let mut names: Vec<&String> = roster.iter().map(|(_, name)| name).collect();
        badges.sort_unstable();
        badges.dedup();
        names.sort_unstable();
        names.dedup();
        assert_eq!(badges.len(), roster.len(), "a badge is on the roster twice");
        assert_eq!(names.len(), roster.len(), "a name is on the roster twice");
        let model: Vec<(u32, String)> = self
            .live
            .iter()
            .map(|agent| (agent.id.badge(), agent.name.clone()))
            .collect();
        assert_eq!(roster, model);
        for (at, name) in ["kai", "goals", "index", "maint"].iter().enumerate() {
            assert_eq!(roster[at], (at as u32 + 1, name.to_string()));
        }
        let packaged = self
            .live
            .iter()
            .filter(|agent| agent.mask.is_some())
            .count();
        assert!(packaged <= MAX_PACKAGED);
        for caller in &self.callers {
            assert_eq!(self.roster.is_live(caller), self.caller_is_live(caller));
        }
        assert_eq!(self.asks.len(), self.open.len());
        assert!(self.open.len() <= MAX_OPEN);
        let others = self.open.iter().filter(|o| !o.admission.caller().is_kai());
        assert!(others.count() <= MAX_OPEN - KAI_ROOM);
        for agent in &self.live {
            let mine = self
                .open
                .iter()
                .filter(|open| open.admission.caller().id() == agent.id)
                .count();
            assert!(agent.is_kai() || mine <= MAX_PER_AGENT, "{}", agent.name);
        }
        for entry in 0..ENTRIES {
            let model = self.open.iter().any(|open| open.entry == entry);
            assert_eq!(self.asks.is_open(entry), model, "entry {entry}");
        }
    }
}

fn run(seed: u64) {
    let mut rng = Rng(seed);
    let mut world = World::new();
    // A caller of another roster is nobody's on this one, whatever its badge and name.
    let stranger = Roster::new().caller(1).unwrap();
    for _ in 0..STEPS {
        match rng.below(100) {
            0..25 => world.install(&mut rng),
            25..32 => world.upgrade(&mut rng),
            32..38 => world.retire(&mut rng),
            38..48 => world.make_caller(&mut rng),
            48..60 => world.admit(&mut rng),
            60..72 => world.ask(&mut rng),
            72..80 => world.answer(&mut rng),
            80..88 => world.spend(&mut rng),
            88..91 => world.withdraw(&mut rng),
            91..95 => world.consent(&mut rng),
            _ => world.name_text(&mut rng),
        }
        world.check();
        assert!(!world.roster.is_live(&stranger), "seed {seed}");
        assert_eq!(
            world.roster.admit(&stranger, "fs_list").map(|_| ()),
            Err(Denied::Retired)
        );
    }
}

#[test]
fn the_rules_of_identity_hold_after_every_step_of_random_sequences() {
    for seed in 0..SEEDS {
        // A failure names its seed.
        let outcome = std::panic::catch_unwind(|| run(seed));
        if let Err(cause) = outcome {
            eprintln!("seed {seed} failed");
            std::panic::resume_unwind(cause);
        }
    }
}

#[test]
fn a_roster_is_made_full_and_emptied_and_its_badges_still_go_up() {
    let mut roster = Roster::new();
    let mut badges = Vec::new();
    for round in 0..3 {
        let ids: Vec<AgentId> = (0..MAX_PACKAGED)
            .map(|at| {
                roster
                    .install(manifest(&format!("rd{round}-{at}"), 1))
                    .unwrap()
            })
            .collect();
        assert_eq!(
            roster.install(manifest("one-too-many", 1)),
            Err(InstallError::Full)
        );
        for id in ids {
            badges.push(id.badge());
            roster.retire(id).unwrap();
        }
        assert_eq!(roster.len(), 4, "only the built-ins are left");
    }
    assert!(badges.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(badges.len(), 3 * MAX_PACKAGED);
}

#[test]
fn kai_filling_every_place_leaves_none_for_an_agent_either() {
    let mut roster = Roster::new();
    let id = roster.install(manifest("ag-0", 0b11)).unwrap();
    let kai = roster.admit(&roster.caller(1).unwrap(), "fs_read").unwrap();
    let agent = roster
        .admit(&roster.caller(id.badge()).unwrap(), "fs_read")
        .unwrap();
    let mut asks = Asks::new();
    for entry in 0..MAX_OPEN as u64 {
        assert_eq!(asks.ask(&roster, entry, &kai, "fs_read a"), Ok(()));
    }
    let full = Err(AskError::Full);
    assert_eq!(asks.ask(&roster, 100, &kai, "fs_read a"), full);
    // The most questions open at once counts every one, kai's among them.
    assert_eq!(asks.ask(&roster, 101, &agent, "fs_read b"), full);
    assert_eq!(asks.len(), MAX_OPEN);
    // One answered, and a place is free for whoever asks first.
    asks.answer(0, true, Mark::of(person())).unwrap();
    assert_eq!(asks.ask(&roster, 101, &agent, "fs_read b"), Ok(()));
    assert_eq!(asks.ask(&roster, 102, &agent, "fs_read b"), full);
    assert_eq!(asks.len(), MAX_OPEN);
}
