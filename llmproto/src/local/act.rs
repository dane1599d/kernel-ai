//! `act` answered by the engine: the registry's pack rows and the three
//! pseudo-actions as `infer::tagged` actions, decoded at the script
//! temperature and seed with jump-forward and token healing.
//!
//! A `script` call's code is then written by today's script prompt and
//! envelope ([`super::script_with`], without `/no_think`), the plan's fallback
//! for M8-style requests: on the M8 50 that scored 48/50 on seeds 42, 1 and 2,
//! against 44-46 for code written inside the call (task 6, run 40028123981).
//! The pack and the script prompt share one cache slot, so a script request
//! pays the script prompt's prefill and the next `act` the pack's.

use crate::act::{self, ActError, ActRequest, Call, History, LANGS, Turn};
use crate::chat;
use crate::script;
use agentabi::ArgType;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use infer::chat::{Ending, Engine, Step};
use infer::tagged::{Action, Arg, Kind, Tagged};
use infer::{Next, Sampler};

const fn text(chars: u16, lines: u8, line_chars: u16, empty: bool) -> Kind<'static> {
    Kind::Text {
        chars,
        lines,
        line_chars,
        empty,
    }
}

/// `say(text)`: at most 240 bytes (ADR 0002 D4).
const SAY: [Arg<'static>; 1] = [Arg {
    name: "text",
    kind: text(240, 1, 240, true),
}];
const OPTION: Kind<'static> = text(40, 1, 40, false);
/// `clarify(question, options)`: at most 6 options.
const CLARIFY: [Arg<'static>; 2] = [
    Arg {
        name: "question",
        kind: text(120, 1, 120, false),
    },
    Arg {
        name: "options",
        kind: Kind::List(&OPTION, 6),
    },
];
/// `script()`: the call only picks it; [`routed`] has today's script prompt write
/// the code and fills in `code` and `lang` (`rhai`; `c` stays out until ADR
/// 0012's gate, `act::LANGS`). Code the call would write is thrown away, and
/// decoding it cost 16-25 s a request in the guest (S02-2), so the grammar
/// closes the call at once.
const SCRIPT: [Arg<'static>; 0] = [];

/// `undo(steps)`: the shell puts back the last change. One step only: a second
/// undo redoes the first, so the grammar offers no more than the door runs.
const UNDO: [Arg<'static>; 1] = [Arg {
    name: "steps",
    kind: Kind::Int { min: 1, max: 1 },
}];

/// Longest path an argument takes.
const PATH_MAX: u16 = 128;

fn leak(kind: Kind<'static>) -> &'static Kind<'static> {
    Box::leak(Box::new(kind))
}

/// The decoder's kind for a registry type. Ints start at 0 at least, as the
/// decoder's do; no pack row takes a negative one.
fn kind(ty: &ArgType) -> Kind<'static> {
    match ty {
        ArgType::Enum(words) => Kind::Enum(words),
        ArgType::Int { min, max } => Kind::Int {
            min: (*min).max(0),
            max: (*max).max(0),
        },
        ArgType::Text { max } => text(*max, 1, *max, false),
        ArgType::Path => text(PATH_MAX, 1, PATH_MAX, false),
        ArgType::Host => Kind::Host,
        ArgType::List(inner, n) => Kind::List(leak(kind(inner)), *n),
        ArgType::Opt(inner) => Kind::Opt(leak(kind(inner))),
    }
}

/// The actions an `act` answer may name: the pack's rows, then `say`,
/// `clarify`, `script` and `undo`. Built once, when the model loads.
pub fn actions(rows: &[&'static agentabi::Action]) -> Vec<Action<'static>> {
    let mut actions: Vec<Action<'static>> = rows
        .iter()
        .map(|row| Action {
            name: row.name,
            args: Vec::leak(
                row.args
                    .iter()
                    .map(|a| Arg {
                        name: a.name,
                        kind: kind(&a.ty),
                    })
                    .collect(),
            ),
        })
        .collect();
    actions.extend([
        Action {
            name: "say",
            args: &SAY,
        },
        Action {
            name: "clarify",
            args: &CLARIFY,
        },
        Action {
            name: "script",
            args: &SCRIPT,
        },
        Action {
            name: "undo",
            args: &UNDO,
        },
    ]);
    actions
}

/// The grammar over the pack's rows (`llmproto::pack::rows`) and the pseudo-actions.
pub fn grammar() -> Result<Tagged<'static>, infer::Error> {
    Tagged::new(Vec::leak(actions(&crate::pack::rows())))
}

/// The engine's KV slots for the two prompts: the act prefix's and the script
/// prompt's (`kabi::model::KV_SLOTS`). One slot for both works too, at the
/// cost of each prompt evicting the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slots {
    pub act: usize,
    pub script: usize,
}

impl Slots {
    /// One slot for everything.
    pub const ONE: Self = Self { act: 0, script: 0 };
    /// The llm service's two.
    pub const TWO: Self = Self { act: 0, script: 1 };
}

/// [`act`] in the act slot, with a `script` call's code written by today's
/// script prompt in the script slot (see the module's note). Words in its
/// envelope's summary instead of code become `say`. `on_step` keeps one time
/// limit over both decodes. Before the script stage, `room` is told how many
/// prompt tokens it would compute; if it says there is no time for them, the
/// call fails at once instead of running out of time half way (S02-2's first
/// request, while the script slot was still warming).
pub fn routed(
    engine: &mut Engine<'_>,
    grammar: &Tagged<'_>,
    request: &ActRequest,
    history: &History,
    slots: Slots,
    mut room: impl FnMut(usize) -> bool,
    mut on_step: impl FnMut(Step) -> Next,
) -> Result<Call, ActError> {
    engine.use_slot(slots.act);
    let call = act(engine, grammar, request, history, &mut on_step)?;
    if call.name != "script" {
        return Ok(call);
    }
    engine.use_slot(slots.script);
    let asked = script::ScriptRequest {
        request: request.line.clone(),
        repair: None,
        timeout_ms: request.timeout_ms,
    };
    let prompt = script::request_with(&asked.request, None, false);
    if !room(engine.uncached_tokens(&super::turns(&prompt))) {
        return Err(ActError::Local(String::from(
            "the AI is still loading its script prompt; try again in a few seconds",
        )));
    }
    let (name, key, value) = match super::script_with(engine, &asked, false, on_step) {
        Ok(answer) => ("script", "code", answer.script),
        Err(script::AnswerError::NoScript(words)) => ("say", "text", words),
        // `ActError::Local` adds its own "local model: ".
        Err(script::AnswerError::Local(why)) => return Err(ActError::Local(why)),
        Err(error) => return Err(ActError::Local(error.describe())),
    };
    let mut arguments = serde_json::Map::new();
    arguments.insert(key.into(), serde_json::Value::String(value));
    if name == "script" {
        arguments.insert("lang".into(), serde_json::Value::String(LANGS[0].into()));
    }
    Ok(Call {
        name: name.into(),
        arguments,
    })
}

/// One call for `request` after `history`, under `grammar`. `on_step` stops it
/// only when time is up.
/// `request` as the prompt takes it: without its State line when the State line
/// and the request together pass [`act::STATE_REQUEST_TOKENS`] real tokens
/// (ADR 0010 §5). A request still over that leaves the answer less room.
pub(crate) fn within_budget(engine: &Engine<'_>, request: &ActRequest) -> ActRequest {
    let text = act::user_text(&request.state, &request.line);
    match !request.state.is_empty() && engine.text_tokens(&text) > act::STATE_REQUEST_TOKENS {
        true => ActRequest {
            state: String::new(),
            ..request.clone()
        },
        false => request.clone(),
    }
}

pub fn act(
    engine: &mut Engine<'_>,
    grammar: &Tagged<'_>,
    request: &ActRequest,
    history: &History,
    on_step: impl FnMut(Step) -> Next,
) -> Result<Call, ActError> {
    let asked = act::request(&within_budget(engine, request), history);
    let turns = super::turns(&asked);
    let mut sampler = Sampler::seeded(script::TEMPERATURE, script::SEED);
    let reply = engine
        .reply_under(
            &turns,
            asked.max_tokens as usize,
            &mut sampler,
            Some(grammar),
            true,
            on_step,
        )
        .map_err(|error| ActError::Local(super::describe(&error)))?;
    match reply.ending {
        Ending::EndToken => act::parse(&String::from_utf8_lossy(&reply.bytes)),
        Ending::Stopped => Err(ActError::Local(String::from(
            "it ran out of time before the call was complete",
        ))),
        Ending::ContextFull => Err(ActError::Local(String::from(
            "the request filled the model's context",
        ))),
        Ending::MaxTokens | Ending::Overflow(_) => Err(ActError::CutOff),
    }
}

/// The tokens the pack and its MODE turn take in `engine`'s prompt.
pub fn pack_tokens(engine: &Engine<'_>) -> usize {
    let prefix = chat::Request {
        messages: act::prefix(&History::new()),
        max_tokens: 1,
    };
    engine.conversation_tokens(&super::turns(&prefix))
}

/// History's budget in `engine`: `act::history::budget` of its context.
pub fn history_budget(engine: &Engine<'_>) -> usize {
    act::history::budget(engine.about().context, pack_tokens(engine))
}

/// The tokens `turn` adds to the prompt, template included (a little over,
/// since the assistant's start is counted too).
pub fn turn_tokens(engine: &Engine<'_>, turn: &Turn) -> usize {
    let messages = chat::Request {
        messages: turn.messages().into(),
        max_tokens: 1,
    };
    engine.conversation_tokens(&super::turns(&messages))
}

/// Keeps `turn` in `history` within `engine`'s budget. Returns whether older
/// turns were dropped.
pub fn remember(engine: &Engine<'_>, history: &mut History, turn: Turn) -> bool {
    let cost = turn_tokens(engine, &turn);
    history.push(turn, cost, history_budget(engine))
}

/// Compaction (P1 plan section 4): computes the pack, the MODE turn and the
/// history into the cache while no request waits, so the next request
/// prefills only its own turn. `on_step` may stop it early; what was computed
/// stays.
pub fn warm(
    engine: &mut Engine<'_>,
    history: &History,
    on_step: impl FnMut(Step) -> Next,
) -> Result<(), infer::Error> {
    let prefix = chat::Request {
        messages: act::prefix(history),
        max_tokens: 1,
    };
    engine
        .reply(
            &super::turns(&prefix),
            1,
            &mut Sampler::greedy(),
            None,
            on_step,
        )
        .map(|_| ())
}

/// Fills the slot in use from a `kai-kv 1` file read from `input`, for the
/// `act` prefix [`warm`] computes with `history`: the positions an `act`
/// request will reuse, or why the file was refused (the slot is then empty).
pub fn restore(
    engine: &mut Engine<'_>,
    history: &History,
    key: &infer::segments::Key,
    input: &mut dyn FnMut(&mut [u8]) -> bool,
) -> Result<usize, infer::segments::Refused> {
    let prefix = chat::Request {
        messages: act::prefix(history),
        max_tokens: 1,
    };
    engine.restore_slot(key, &super::turns(&prefix), input)
}

/// The `act` prefix's `pack_id`, for its `kai-kv 1` file's key.
pub fn pack_id() -> u64 {
    crate::pack::act_pack_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use infer::json::Constraint;

    #[test]
    fn the_kv_key_names_the_pack_as_the_shells_log_does() {
        // One function for both, so llm's `kai-kv 1` key and the shell's
        // `actlog 1` entries cannot drift apart.
        assert_eq!(pack_id(), crate::pack::act_pack_id());
        let pack = crate::pack::render(crate::pack::Format::Dsl1);
        assert_eq!(
            pack_id(),
            crate::pack::pack_id(crate::pack::Format::Dsl1, "", &pack)
        );
    }

    #[test]
    fn every_pack_row_and_pseudo_action_decodes() {
        let g = grammar().unwrap();
        for text in [
            r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#,
            r#"{"name":"cpus","arguments":{}}"#,
            r#"{"name":"component_revert","arguments":{"component":"shell","back":null}}"#,
            r#"{"name":"tune","arguments":{"target":"build-3","intent":null}}"#,
            r#"{"name":"say","arguments":{"text":"Hello."}}"#,
            r#"{"name":"clarify","arguments":{"question":"Which?","options":["a","b"]}}"#,
            // The call only picks `script`; llm writes its code (`routed`).
            r#"{"name":"script","arguments":{}}"#,
            r#"{"name":"undo","arguments":{"steps":1}}"#,
        ] {
            let state = g.advance(g.start(), text.as_bytes()).unwrap();
            assert!(g.is_done(&state), "{text}");
            assert!(act::parse(text).is_ok());
        }
        // The shell's pure helpers and code inside a script call are not offered.
        for text in [
            r#"{"name":"fixed""#,
            r#"{"name":"script","arguments":{"code""#,
            r#"{"name":"undo","arguments":{"steps":0"#,
            r#"{"name":"undo","arguments":{"steps":2"#,
        ] {
            assert!(g.advance(g.start(), text.as_bytes()).is_err(), "{text}");
        }
    }

    /// The State line stays while it and the request fit 224 real tokens, and
    /// goes once they pass it; the request itself is never cut.
    #[test]
    fn the_state_line_goes_when_state_and_request_pass_224_tokens() {
        let bytes = infer::fixture::local_script(&crate::local::system_prompts());
        let budget = infer::chat::Budget {
            kv_bytes: 64 << 20,
            other_bytes: 64 << 20,
        };
        let engine = Engine::load(&bytes, quant::Tier::best(), 2048, budget).unwrap();
        let short = ActRequest {
            line: "stop this".into(),
            state: "#{dir: \"/home/user\", net: \"up\"}".into(),
            timeout_ms: 1,
        };
        assert_eq!(within_budget(&engine, &short), short);
        let windows = (0..20)
            .map(|i| alloc::format!("\"w{i}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let long = ActRequest {
            // Words the fixture's merges never saw, so each is several tokens.
            line: (0..60)
                .map(|i| alloc::format!("q{}z", i * 37 % 101))
                .collect::<Vec<_>>()
                .join(" "),
            state: alloc::format!("#{{windows: [{windows}]}}"),
            ..short.clone()
        };
        let text = act::user_text(&long.state, &long.line);
        assert!(engine.text_tokens(&text) > act::STATE_REQUEST_TOKENS);
        let kept = within_budget(&engine, &long);
        assert_eq!(
            (kept.state.as_str(), kept.line.as_str()),
            ("", long.line.as_str())
        );
    }

    /// On the generated local-script model the answer is always a call that
    /// parses or a cut-off, the same on every tier.
    #[test]
    fn the_engine_answers_with_a_call_alike_on_every_tier() {
        let text = [
            crate::local::system_prompts(),
            crate::pack::render(crate::pack::Format::Dsl1),
        ]
        .join("\n");
        let bytes = infer::fixture::local_script(&text);
        let g = grammar().unwrap();
        let request = ActRequest {
            line: "switch to round robin".into(),
            state: String::new(),
            timeout_ms: 1,
        };
        let budget = infer::chat::Budget {
            kv_bytes: 64 << 20,
            other_bytes: 64 << 20,
        };
        let mut first = None;
        for tier in quant::Tier::all() {
            let mut engine = Engine::load(&bytes, tier, 3072, budget).unwrap();
            engine.add_slot().unwrap();
            let answer = routed(
                &mut engine,
                &g,
                &request,
                &History::new(),
                Slots::TWO,
                |_| true,
                |_| Next::Continue,
            );
            assert!(
                matches!(answer, Ok(_) | Err(ActError::CutOff)),
                "{answer:?}"
            );
            match &first {
                None => first = Some(answer),
                Some(seen) => assert_eq!(&answer, seen, "{} tier", tier.name()),
            }
        }
    }

    /// History in the engine: the budget from its context and pack, turns costed
    /// by the real template, and compaction leaves the next request to prefill
    /// only its own turn, with the same answer as a cold engine.
    #[test]
    fn history_is_budgeted_compacted_and_changes_no_answer_but_by_its_text() {
        let text = [
            crate::local::system_prompts(),
            crate::pack::render(crate::pack::Format::Dsl1),
        ]
        .join(
            "
",
        );
        let bytes = infer::fixture::local_script(&text);
        let budget = infer::chat::Budget {
            kv_bytes: 64 << 20,
            other_bytes: 64 << 20,
        };
        let g = grammar().unwrap();
        let mut engine = Engine::load(&bytes, quant::Tier::best(), 3072, budget).unwrap();
        // The fixture's trained context is 2,048, which leaves history nothing;
        // the budget is the plan's formula over this engine's numbers.
        let (context, pack) = (engine.about().context, pack_tokens(&engine));
        assert_eq!(history_budget(&engine), act::history::budget(context, pack));
        assert!(pack > 100, "{pack}");
        let mut history = History::new();
        let turn = Turn {
            line: "switch to round robin".into(),
            call: r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#.into(),
            result: "Done: policy round-robin (was interactive). undo".into(),
        };
        let cost = turn_tokens(&engine, &turn);
        assert!(cost > 10 && cost < 100, "{cost}");
        assert!(!history.push(turn, cost, act::history::HISTORY_MAX));
        warm(&mut engine, &history, |_| Next::Continue).unwrap();
        let request = ActRequest {
            line: "undo that".into(),
            state: String::new(),
            timeout_ms: 1,
        };
        let mut cached = 0;
        let warm_answer = act(&mut engine, &g, &request, &history, |step| {
            if let Step::Cached(n) = step {
                cached = n;
            }
            Next::Continue
        });
        // Everything but the new turn came from the cache: all of the warmed
        // prefix but the generation prompt it ended with (the assistant's start
        // and the empty think block, which the next request's turn replaces).
        let prefix = chat::Request {
            messages: act::prefix(&history),
            max_tokens: 1,
        };
        let warmed = engine.conversation_tokens(&super::super::turns(&prefix));
        assert!(cached + 16 >= warmed, "{cached} of {warmed}");
        let mut cold = Engine::load(&bytes, quant::Tier::best(), 3072, budget).unwrap();
        let cold_answer = act(&mut cold, &g, &request, &history, |_| Next::Continue);
        assert_eq!(warm_answer, cold_answer);
        // `new`: history gone, and the request is the plain one again.
        history.clear();
        assert_eq!(act::request(&request, &history).messages.len(), 3);
    }
}
