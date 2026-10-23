//! The Rhai shell: answers each line the kernel terminal sends with `ok <text>`,
//! `err <text>` or `confirm <id> <text>`. The evaluation itself lives in the
//! host-tested `shellcore` crate; this program connects it to the system calls.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use kabi::sched::{SchedStats, TrialRequest};
use llmproto::chat::Completion;
use llmproto::script::{self, Answer, AnswerError, ScriptRequest};
use llmproto::tune::{self, TuneRequest};
use shellcore::{DEFAULT_CONFIG, Host, SCRIPT_ROUND_MS, Shell};
use user::{arg, args, logln, println, sys};

user::entry!(main);

/// How long `ask` waits for the llm service.
const ASK_TIMEOUT_MS: u64 = 140_000;
/// How long one script round waits for the llm service: its own limit and a margin.
const SCRIPT_CALL_MS: u64 = SCRIPT_ROUND_MS + 5_000;

struct System;

/// The route to the llm service: the shell's plan line has `uses=llm`, its only
/// route.
fn llm() -> kabi::handle::Handle {
    user::route(0).unwrap_or(kabi::handle::Handle::NONE)
}

impl Host for System {
    fn now_ns(&self) -> u64 {
        sys::time_ns()
    }

    fn interrupted(&self) -> bool {
        sys::interrupted()
    }

    fn heap_bytes(&self) -> u64 {
        user::heap_bytes()
    }

    fn metrics(&self) -> Result<kabi::Metrics, kabi::Error> {
        sys::metrics()
    }

    fn set_policy(&self, id: u64) -> Result<(), kabi::Error> {
        sys::set_policy(id).map(|_| ())
    }

    fn ask(&self, prompt: &str) -> Result<String, String> {
        let request = alloc::format!("ask {prompt}");
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(llm(), request.as_bytes(), &mut reply, ASK_TIMEOUT_MS)
            .map_err(|error| alloc::format!("llm service: {error:?}"))?;
        let text = String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned();
        match text.split_once(' ').unwrap_or((text.as_str(), "")) {
            ("ok", answer) => Ok(answer.to_string()),
            ("err", why) => Err(why.to_string()),
            _ => Err("llm service: malformed reply".to_string()),
        }
    }

    fn script(&self, request: &ScriptRequest) -> Result<Answer, AnswerError> {
        let server = |why: &str| AnswerError::Server(alloc::format!("llm service: {why}"));
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(
            llm(),
            request.encode().as_bytes(),
            &mut reply,
            SCRIPT_CALL_MS,
        )
        .map_err(|error| server(&alloc::format!("{error:?}")))?;
        let text = core::str::from_utf8(&reply[..len.min(reply.len())])
            .map_err(|_| server("the reply is not UTF-8"))?;
        match text.split_once(' ').unwrap_or((text, "")) {
            ("ok", answer) => {
                script::decode_answer(answer).ok_or_else(|| server("malformed answer"))
            }
            ("err", why) => Err(script::decode_error(why)),
            _ => Err(server("malformed reply")),
        }
    }

    /// The audit trail goes to the log, not the screen: the reply says what happened.
    fn audit(&self, text: &str) {
        logln!("{text}");
    }

    fn sched_stats(&self, window_ms: u64) -> Result<Box<SchedStats>, kabi::Error> {
        let mut stats = Box::new(SchedStats::empty());
        sys::sched_stats(&mut stats, window_ms)?;
        Ok(stats)
    }

    fn sched_trial(&self, request: &TrialRequest) -> Result<u64, kabi::Error> {
        sys::sched_trial(request)
    }

    fn set_spec(&self, id: u64, preset: u64) -> Result<(), kabi::Error> {
        sys::set_spec(id, preset).map(|_| ())
    }

    fn sleep_ms(&self, ms: u64) {
        sys::sleep_ms(ms);
    }

    fn tune(&self, request: &TuneRequest) -> Result<Completion, String> {
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(
            llm(),
            request.encode().as_bytes(),
            &mut reply,
            tune::CALL_TIMEOUT_MS,
        )
        .map_err(|error| alloc::format!("llm service: {error:?}"))?;
        let text = String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned();
        match text.split_once(' ').unwrap_or((text.as_str(), "")) {
            ("ok", completion) => tune::decode_completion(completion)
                .ok_or_else(|| "llm service: malformed answer".to_string()),
            ("err", why) => Err(why.to_string()),
            _ => Err("llm service: malformed reply".to_string()),
        }
    }
}

fn main() -> i32 {
    let args = args();
    let mut shell = Shell::new(Rc::new(System), DEFAULT_CONFIG);
    println!("up (restart {})", arg(&args, "restart").unwrap_or("0"));
    let mut line = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let mut caller = u64::MAX;
        let len = match sys::svc_recv(&mut line, 0, &mut caller) {
            Ok(len) => len.min(line.len()),
            Err(kabi::Error::TimedOut) => continue,
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
                continue;
            }
        };
        // Only the kernel terminal may run shell lines.
        let mut reply = if caller == 0 {
            let text = core::str::from_utf8(&line[..len]).unwrap_or("");
            shell.eval(text).to_wire()
        } else {
            String::from("err the shell answers only the terminal")
        };
        reply.truncate(reply.floor_char_boundary(kabi::MAX_MESSAGE));
        if let Err(error) = sys::svc_reply(reply.as_bytes()) {
            println!("svc_reply failed: {error:?}");
        }
    }
}
