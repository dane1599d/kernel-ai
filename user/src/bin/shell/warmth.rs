//! Whether the model is warm, for the panel (founder test 2). After a rebuild
//! that changes the prompt, llm fills its two prompt slots again before a
//! request runs at full speed, about 17 s. Until it says both are warm, the
//! panel says "Kai is getting ready" instead of a bare "thinking".
//!
//! The shell asks llm's `warm` now and then, with a short wait: llm answers it
//! from its serve loop between warm-up steps, and each ask pauses a warm-up
//! for a moment, so it is asked every few seconds while cold and seldom after.
//! What it tells `desktop` is `deskcore::panel_wire`'s `panel ready`, sent
//! only when the answer changes.

use super::{ask_desktop, desktop, llm};
use deskcore::panel_wire;
use user::{println, sys};

/// How often llm is asked while its slots are cold.
const COLD_ASK_NS: u64 = 3_000_000_000;
/// How often once they are warm: a switch to `c>` cools them again.
const WARM_ASK_NS: u64 = 30_000_000_000;
/// How long the shell waits for the answer; llm busy with a request answers
/// later, and that pass is skipped.
const WARM_CALL_MS: u64 = 200;

/// What the shell last told the panel, and when it last asked.
#[derive(Default)]
pub(crate) struct Warmth {
    told: Option<bool>,
    asked_ns: u64,
}

impl Warmth {
    /// Asks llm, when it is time, and tells the panel when the answer changed.
    pub(crate) fn report(&mut self) {
        let now = sys::time_ns();
        let gap = match self.told {
            Some(true) => WARM_ASK_NS,
            _ => COLD_ASK_NS,
        };
        if self.asked_ns != 0 && now.saturating_sub(self.asked_ns) < gap {
            return;
        }
        self.asked_ns = now.max(1);
        let Some(ready) = ask_llm() else {
            return;
        };
        if self.told == Some(ready) {
            return;
        }
        let Some(route) = desktop() else {
            return;
        };
        match ask_desktop(route, &panel_wire::encode_ready(ready)) {
            Ok(answer) if answer == "ok" => self.told = Some(ready),
            Ok(answer) => println!("the panel was not told the model's warmth: {answer}"),
            Err(why) => println!("the panel was not told the model's warmth: {why}"),
        }
    }
}

/// llm's answer to `warm`: whether both prompt slots hold their prompts. None
/// when llm did not answer in time or has no route here.
fn ask_llm() -> Option<bool> {
    let mut reply = [0u8; 16];
    let len = sys::call_route(llm(), b"warm", &mut reply, WARM_CALL_MS).ok()?;
    match reply.get(..len)? {
        b"ok yes" => Some(true),
        b"ok no" => Some(false),
        _ => None,
    }
}
