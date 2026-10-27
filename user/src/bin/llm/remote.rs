//! The HTTP backend: OpenAI-compatible chat JSON to the LLM server the kernel names
//! on its command line (`llm=IP:PORT`), asked only when no local model runs, which
//! makes it CI's test double. The kernel carries the bytes to that one server;
//! this module builds and parses them.

use alloc::format;
use alloc::string::{String, ToString};
use kabi::HttpRequest;
use llmproto::act::{self, ActError, ActRequest, Call, History};
use llmproto::chat::Completion;
use llmproto::openai::CompletionError;
use llmproto::script::{self, ScriptRequest};
use llmproto::text::printable;
use llmproto::tune::TuneRequest;
use llmproto::{chat, explain, http, openai};
use user::{arg, sys};

const DEFAULT_MODEL: &str = "qwen3-8b";
const CHAT_TIMEOUT_MS: u64 = 120_000;
const HEALTH_TIMEOUT_MS: u64 = 5_000;
const MAX_ERROR_CHARS: usize = 160;

pub struct Server {
    /// `IP:PORT` of the server, for the Host header only.
    pub host: String,
    pub model: String,
}

impl Server {
    /// The server the plan line names with `host=` (and `model=`), if any.
    pub fn from_args(args: &str) -> Option<Self> {
        Some(Self {
            host: arg(args, "host")?.to_string(),
            model: arg(args, "model").unwrap_or(DEFAULT_MODEL).to_string(),
        })
    }

    /// `HTTP <status> in <ms> ms`, from `GET /health`.
    pub fn health(&self, response: &mut [u8]) -> Result<String, String> {
        let started = sys::time_ns();
        let reply = exchange(
            &http::get(&self.host, "/health"),
            HEALTH_TIMEOUT_MS,
            response,
        )?;
        let rtt_ms = (sys::time_ns() - started) / 1_000_000;
        Ok(format!("HTTP {} in {rtt_ms} ms", reply.status))
    }

    pub fn ask(&self, prompt: &str, response: &mut [u8]) -> Result<String, String> {
        let body = openai::chat_request(&self.model, &chat::ask(prompt));
        let reply = self.post(&body, CHAT_TIMEOUT_MS, response)?;
        let completion = openai::completion(&reply).map_err(|error| match error {
            CompletionError::Status(status, body) => format!(
                "server answered HTTP {status}: {}",
                printable(&body, MAX_ERROR_CHARS, 1)
            ),
            CompletionError::Reply(error) => format!("bad chat reply: {error:?}"),
        })?;
        chat::answer(&completion)
    }

    /// Asks the model to explain a rollback from the kernel's facts; prose back.
    pub fn explain(&self, facts: &str, response: &mut [u8]) -> Result<String, String> {
        let body = explain::chat_request(&self.model, facts);
        let reply = self.post(&body, CHAT_TIMEOUT_MS, response)?;
        let completion =
            openai::completion(&reply).map_err(|error| explain::server_error(&error))?;
        explain::answer(&completion)
    }

    /// Asks the model for a script. The answer goes back framed by length, exactly
    /// as the model wrote it after the text checks; the shell checks it again and
    /// runs it.
    pub fn script(&self, request: &ScriptRequest, response: &mut [u8]) -> Result<String, String> {
        let repair = request
            .repair
            .as_ref()
            .map(|(script, error)| script::Repair { script, error });
        let body = script::chat_request(&self.model, &request.request, repair);
        let timeout = request.timeout_ms.min(script::SERVICE_TIMEOUT_MS);
        self.post(&body, timeout, response)
            .map_err(script::AnswerError::Server)
            .and_then(|reply| openai::completion(&reply).map_err(script::server_error))
            .and_then(|completion| script::answer(&completion))
            .map(|answer| script::encode_answer(&answer))
            .map_err(|error| script::encode_error(&error))
    }

    /// The model's answer to a `tune` request, under `response_format`.
    pub fn tune(&self, request: &TuneRequest, response: &mut [u8]) -> Result<Completion, String> {
        let reply = self.post(&request.chat_body(&self.model), CHAT_TIMEOUT_MS, response)?;
        openai::completion(&reply).map_err(|error| match error {
            CompletionError::Status(status, body) => format!(
                "server answered HTTP {status}: {}",
                printable(&body, MAX_ERROR_CHARS, 1)
            ),
            CompletionError::Reply(error) => format!("bad chat reply: {error:?}"),
        })
    }

    /// One tagged call: the `act` prompt after the conversation llm keeps for a
    /// server, and the reply's text read as a call.
    pub fn act(
        &self,
        request: &ActRequest,
        history: &History,
        response: &mut [u8],
    ) -> Result<Call, ActError> {
        let body = openai::chat_request(&self.model, &act::request(request, history));
        let reply = self
            .post(&body, CHAT_TIMEOUT_MS, response)
            .map_err(ActError::Unavailable)?;
        let completion = openai::completion(&reply).map_err(|_| ActError::NotACall)?;
        act::parse(completion.text.trim())
    }

    fn post(
        &self,
        body: &[u8],
        timeout_ms: u64,
        response: &mut [u8],
    ) -> Result<http::Response, String> {
        let request = http::post_json(&self.host, openai::CHAT_PATH, None, body);
        exchange(&request, timeout_ms, response)
    }
}

fn exchange(
    request: &[u8],
    timeout_ms: u64,
    response: &mut [u8],
) -> Result<http::Response, String> {
    let call = HttpRequest {
        request_ptr: request.as_ptr() as u64,
        request_len: request.len() as u64,
        response_ptr: response.as_mut_ptr() as u64,
        response_cap: response.len() as u64,
        timeout_ms,
    };
    let len = sys::http(&call).map_err(|error| format!("network: {error:?}"))?;
    http::parse_response(&response[..len.min(response.len())], true)
        .map_err(|error| format!("bad HTTP response: {error:?}"))
}
