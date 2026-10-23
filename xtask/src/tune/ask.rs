//! Where the answer comes from: a canned file (tests, CI, replays) or the real
//! model run by Kernel AI's own engine on the host, with the guest's context cap
//! and memory budget, decoded greedily under the answer's schema exactly as the
//! llm service decodes it (`llmproto::local::tune`), so it decides as the OS
//! would. `llmproto::tune::decide` still checks every answer.

use infer::Next;
use infer::chat::{Budget, Engine};
use llmproto::chat::Completion;
use llmproto::tune::{self, ANSWER_TOKENS, Observation, Summary, TEMPLATE_TOKENS, TuneRequest};
use quant::Tier;
use std::path::PathBuf;

/// Something that answers a tune request.
pub trait Ask {
    /// The model's answer, and a line about how it was made.
    fn ask(&mut self, request: &TuneRequest) -> Result<(Completion, String), String>;

    /// The summary to ask about: by the token estimate, unless the asker has the
    /// model's tokenizer and can count exactly.
    fn summarize(&self, observation: &Observation) -> Option<Summary> {
        tune::summary(observation)
    }
}

/// The same text for every request, as if the model had written it.
pub struct Canned {
    pub text: String,
    pub from: String,
}

impl Canned {
    pub fn from_file(path: &PathBuf) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Ok(Self {
            text,
            from: path.display().to_string(),
        })
    }
}

impl Ask for Canned {
    fn ask(&mut self, _: &TuneRequest) -> Result<(Completion, String), String> {
        let completion = Completion {
            text: self.text.clone(),
            cut_off: false,
        };
        Ok((completion, format!("a canned answer from {}", self.from)))
    }
}

/// The model's file in memory, for an engine to borrow, and its tokenizer.
pub struct Local {
    pub label: String,
    pub bytes: Vec<u8>,
    pub tier: Tier,
    tok: bpe::Tokenizer,
}

impl Local {
    pub fn new(label: String, bytes: Vec<u8>, tier: Tier) -> Result<Self, String> {
        let file = gguf::Gguf::parse(&bytes).map_err(|e| format!("{label}: {e}"))?;
        let tok = bpe::Tokenizer::from_gguf(file.header()).map_err(|e| format!("{label}: {e}"))?;
        Ok(Self {
            label,
            bytes,
            tier,
            tok,
        })
    }
}

impl Ask for Local {
    /// Counted with the model's own tokenizer, so the prompt fits the guest's
    /// context whatever the processes are called.
    fn summarize(&self, observation: &Observation) -> Option<Summary> {
        let count = |text: &str| {
            let mut ids = Vec::new();
            self.tok.encode(text, &mut ids);
            ids.len()
        };
        let budget = tune::PROMPT_BUDGET;
        tune::summary_with(observation, &count, budget)
    }

    fn ask(&mut self, request: &TuneRequest) -> Result<(Completion, String), String> {
        let chat = request.chat();
        let (system, user) = chat.single_turn().ok_or("a tune request is one turn")?;
        let budget = Budget {
            kv_bytes: kabi::model::KV_BYTES as usize,
            other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize,
        };
        let cap = kabi::model::CONTEXT_CAP;
        let mut engine = Engine::load(&self.bytes, self.tier, cap, budget)
            .map_err(|e| format!("{}: {e}", self.label))?;
        let context = engine.about().context;
        let prompt = engine.prompt_tokens(system, user);
        let note = format!(
            "{} on the {} tier: a {prompt}-token prompt in the guest's {context} positions, \
             up to {ANSWER_TOKENS} answer tokens (the summary budgets {TEMPLATE_TOKENS} for the \
             template); greedy under the answer's schema, as the llm service decodes",
            self.label,
            self.tier.name()
        );
        // A prompt that leaves no room is no answer, which `decide` makes a keep,
        // as the llm service's error is in the OS.
        match llmproto::local::tune(&mut engine, request, |_| Next::Continue) {
            Ok(completion) => Ok((completion, note)),
            Err(infer::Error::ContextFull | infer::Error::NoRoom) => {
                let completion = Completion {
                    text: String::new(),
                    cut_off: true,
                };
                Ok((completion, format!("{note}; the prompt fills the context")))
            }
            Err(e) => Err(format!("{}: {}", self.label, llmproto::local::describe(&e))),
        }
    }
}
