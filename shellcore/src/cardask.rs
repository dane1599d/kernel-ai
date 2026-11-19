//! An ask only the trusted card answers (G2, M2): the goal and memory rows'
//! questions. The question is code-written, Deny is highlighted, and only the
//! card's Allow with its ONCE carries the call out; a question too long for
//! the card is refused, never asked as y/n.

use crate::actions::{How, Pending};
use crate::script::Checked;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use termcore::menu::{Menu, Origin};

impl Shell {
    /// Asks `question` (which shows `call`) on the card for request `id`;
    /// `note` says in full what the call does, on `show`. `Ok` is the card,
    /// with the ask pending; `Err` says why it was not asked.
    pub(crate) fn card_ask(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        note: String,
        question: &str,
    ) -> Result<Reply, Reply> {
        let menu = Menu::new(id, question, Some(""), Some(""), Origin::Added)
            .and_then(|menu| menu.with_call(call));
        if menu.is_err() {
            return Err(Reply::err(format!(
                "not asked: {call} does not fit its card"
            )));
        }
        self.host.audit(&format!("request {id}: {call} asks"));
        let mut checked = Checked::shown_only(call);
        checked.note = Some(note);
        self.shown = Some(format!(
            "{}\nasked first: it is answered on its card",
            crate::script::shown(id, &checked)
        ));
        self.pending = Some(Pending {
            id,
            request: request.to_string(),
            checked,
            asking: Vec::new(),
            created_ns: self.host.now_ns(),
            how: How {
                form: crate::actlog::Form::Call,
                tier: agentabi::Tier::Ask,
                consent: String::from("asked"),
            },
            call: None,
            offer: None,
            menu: Some(String::from(call)),
            pkg: None,
            standing: None,
            model: None,
            app: None,
            install: None,
            net: None,
            send: None,
            power: false,
            goal: None,
            memory: None,
        });
        Ok(Reply::menu(
            id,
            Menu::encode(id, Origin::Added, question, "", "", call),
        ))
    }
}
