//! Requests to the `desktop` service from its clients and the system: the
//! panel, the appearance, files, and lines a stale client left.

use super::*;

pub(super) fn serve(state: &mut State, screen: &Screen, bytes: &[u8], info: &RecvInfo) {
    let pid = info.caller_pid;
    if let Some(reply) = perf::request(state, screen, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    if let Some(reply) = panel_request(state, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    if let Some(reply) = appearance_request(state, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    if let Some(reply) = wire_request(&settings::SETTINGS, &mut state.settings, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    let models = &deskcore::models_wire::MODELS;
    if let Some(reply) = wire_request(models, &mut state.models, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    if let Some(reply) = net_request(state, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    if let Some(reply) = agent::request(state, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    if let Some(reply) = effects_request(state, bytes, pid) {
        let _ = sys::reply_h(reply.as_bytes(), &[]);
        return;
    }
    if let Some(reply) = files_request(state, bytes, pid) {
        let _ = sys::reply_h(&reply, &[]);
        return;
    }
    let new = !state.clients.iter().any(|c| c.pid == pid);
    if new {
        let label = name_of(pid);
        let mut client = Client::new(pid, &label);
        client.theme(&state.theme_name);
        client.scale(state.scale);
        // Caps Lock as it is now: a client started with it on types by it.
        client.apply(&[Effect::Locks {
            caps: state.desk.caps(),
        }]);
        state.clients.push(client);
    }
    let Some(at) = state.clients.iter().position(|c| c.pid == pid) else {
        return;
    };
    let handle = (info.handle_count > 0).then(|| Handle(info.handles[0]));
    let result = state.clients[at].handle(&mut state.desk, bytes, handle.is_some());
    match result {
        Ok(needs) => {
            for need in needs {
                apply_need(state, at, pid, need, handle);
            }
            // A window it opened or closed moved the focus: every other client
            // hears it too, so the one that lost it gets its focus_out.
            let shared = state.clients[at].take_shared();
            if !shared.is_empty() {
                for (i, client) in state.clients.iter_mut().enumerate() {
                    if i != at {
                        client.apply(&shared);
                    }
                }
            }
            let reply = state.clients[at].take_events(kabi::MAX_MESSAGE);
            let _ = sys::reply_h(&reply, &[]);
        }
        // A client new to this desktop that names objects it never made was
        // talking to the desktop before a restart: it gets the greeting (the
        // registry's globals), not an empty reply, so it opens its windows
        // again (`term` committed to its old surface each time the console
        // changed, and never heard the greeting).
        Err(surfabi::wire::Error::UnknownObject) if new => {
            println!("desktop: pid {pid} spoke to an earlier desktop; greeting it");
            // Windows the message opened before it failed go with the old
            // connection, not left on the desk.
            let effects = state.clients[at].close_all(&mut state.desk);
            deliver(state, &effects);
            let label = state.clients[at].label.clone();
            state.clients[at] = Client::new(pid, &label);
            state.clients[at].theme(&state.theme_name);
            state.clients[at].scale(state.scale);
            let reply = state.clients[at].take_events(kabi::MAX_MESSAGE);
            let _ = sys::reply_h(&reply, &[]);
        }
        Err(error) => {
            println!("desktop: pid {pid} sent a bad message ({error:?}); closing its windows");
            let mut client = state.clients.remove(at);
            let effects = client.close_all(&mut state.desk);
            deliver(state, &effects);
            state.shown.retain(|(_, owner, ..)| *owner != pid);
            for pool in state.pools.iter().filter(|p| p.pid == pid) {
                let _ = sys::shm_unmap(pool.address);
            }
            state.pools.retain(|p| p.pid != pid);
            let _ = sys::reply_h(b"", &[]);
        }
    }
}

/// The shell's `panel take` and `panel state` (P3 task 7,
/// `deskcore::panel_wire`): its answer, or `None` for a window client's
/// message. Only the shell may take the panel's lines or set its state.
fn panel_request(state: &mut State, bytes: &[u8], pid: u64) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    if !text.starts_with("panel ") {
        return None;
    }
    if name_of(pid) != "shell" {
        return Some(String::from("err only the shell talks to the panel"));
    }
    if text == panel_wire::TAKE {
        let request = state.panel.take();
        if let Some(taken) = &request {
            finish_stale(state);
            let line = matches!(taken, deskcore::panel::Request::Line(_));
            state.running.took_panel(line);
            if let deskcore::panel::Request::Line(line) = taken {
                chat_push(state, Who::You, line);
            }
        }
        if request.is_some() {
            println!("desktop: the shell took the panel's request");
            // From the Enter to the take: the wake's latency (under 100 ms).
            if let deskcore::panel::State::Thinking { since_ms } = *state.panel.state() {
                let ms = now_ms().saturating_sub(since_ms);
                let within = if ms < 100 { "within" } else { "over" };
                println!(
                    "desktop: the panel's request was taken {ms} ms after Enter ({within} 100 ms)"
                );
            }
        }
        return Some(panel_wire::encode_take(request.as_ref()));
    }
    if let Some(ready) = panel_wire::parse_ready(text) {
        if state.panel.set_ready(ready) {
            // Neither line holds the other, so a case can tell them apart.
            let now = if ready { "ready" } else { "getting ready" };
            println!("desktop: the model is {now}");
            state.extra.push(state.desk.pane_area());
            // The tray's chip says it too, from the next read of the bar.
            state.bar_at = 0;
        }
        return Some(String::from("ok"));
    }
    // The running model's name, cleaned as it is read: the pane's header.
    if let Some(name) = panel_wire::parse_model(text) {
        if state.panel.set_model(name) {
            println!("desktop: the shell named the model");
            state
                .extra
                .push(deskcore::paint_pane::header_area(&state.desk));
        }
        return Some(String::from("ok"));
    }
    // A card's result: shown, unless a newer line is thinking, whose own
    // result comes after.
    // A send it allowed comes with its entry: the line is the send's own,
    // so its countdown and undo show in its place.
    if let Some((shown, held)) = panel_wire::parse_card_held(text) {
        let thinking = matches!(state.panel.state(), deskcore::panel::State::Thinking { .. });
        let text = bar_text(&shown).map(String::from);
        if !thinking && state.panel.set_done(shown, held) {
            println!("desktop: the panel shows what a card came to");
            log_bar(state.log_bar, text.as_deref());
            state.extra.push(state.desk.pane_area());
        }
        return Some(String::from("ok"));
    }
    if let Some(said) = panel_wire::parse_said(text) {
        // Its length only, as for a typed line: the AI's words may hold anything.
        if chat_push(state, Who::Kai, said) {
            println!(
                "desktop: the history shows what Kai said ({} characters)",
                said.chars().count()
            );
        }
        return Some(String::from("ok"));
    }
    if let Some(note) = panel_wire::parse_note(text) {
        chat_push(state, Who::Note, note);
        return Some(String::from("ok"));
    }
    if let Some(reply) = sends_request(state, text) {
        return Some(reply);
    }
    let Some((shown, entry)) = panel_wire::parse_done(text) else {
        return Some(String::from("err not a panel request"));
    };
    // A result, or idle: the shell answered the panel's line.
    if !matches!(
        shown,
        deskcore::panel::State::Thinking { .. } | deskcore::panel::State::Acting { .. }
    ) {
        state.running.ended(deskcore::running::Line::Panel);
    }
    // The bar's result line or refusal, code's, joins the history as a note.
    match &shown {
        deskcore::panel::State::Done { line } => {
            chat_push(state, Who::Note, line);
        }
        deskcore::panel::State::Refused { reason } => {
            chat_push(state, Who::Note, reason);
        }
        _ => {}
    }
    let word = pane_word(&shown);
    let text = bar_text(&shown).map(String::from);
    if state.panel.set_done(shown, entry) {
        // Its kind only: the line is the shell's result, which may name a file.
        println!("desktop: the panel shows {word}");
        log_bar(state.log_bar, text.as_deref());
        state.extra.push(state.desk.pane_area());
    }
    Some(String::from("ok"))
}

/// The shell's held sends (`deskcore::sends`): the ones waiting or going,
/// and what one came to, which joins the history as code's note.
fn sends_request(state: &mut State, text: &str) -> Option<String> {
    use deskcore::sends;
    if text.starts_with("panel sends ") {
        let now = now_ms();
        // A report it cannot read leaves no stale countdown or undo behind.
        let (list, answer) = match sends::parse_sends(text) {
            Some(list) => (list, "ok"),
            None => (Vec::new(), "err not a list of sends"),
        };
        let held = list.iter().map(|send| send.held(now)).collect();
        if state.panel.set_sends(held) {
            println!("desktop: the panel holds {} sends", list.len());
            state.extra.push(paint_panel::state_area(&state.desk));
        }
        return Some(String::from(answer));
    }
    let (entry, failed, line) = sends::parse_sent(text)?;
    chat_push(state, Who::Note, &line);
    if state.panel.send_ended(entry, line, failed) {
        println!("desktop: the panel shows what a send came to");
        state.extra.push(state.desk.pane_area());
    }
    Some(String::from("ok"))
}

/// Adds to the panel's history and damages where it is drawn; false when
/// nothing was added.
fn chat_push(state: &mut State, who: Who, text: &str) -> bool {
    let area = paint_chat::chat_area(&state.desk);
    let added = state.painter.chat_push(&mut state.chat, who, text, area);
    if added && matches!(who, Who::Kai) {
        state.desk.kai_said();
    }
    if added {
        state.extra.push(area);
    }
    added
}

/// The shell's saved appearance (`panel_wire::APPEARANCE`): painted in from
/// the next frame, the whole screen and the cursor (`theme`). Only the shell's,
/// by the kernel's name for the caller, and only text `look` takes.
fn appearance_request(state: &mut State, bytes: &[u8], pid: u64) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    let saved = panel_wire::parse_appearance(text)?;
    if name_of(pid) != "shell" {
        return Some(String::from("err only the shell sets the appearance"));
    }
    match Appearance::decode(saved, &Shipped) {
        Ok(appearance) => {
            state.appearance = appearance;
            Some(String::from("ok"))
        }
        Err(why) => {
            println!("desktop: the saved appearance is refused ({})", why.0);
            Some(alloc::format!("err the appearance is refused: {}", why.0))
        }
    }
}

/// The editor's and the shell's file requests (P4, `deskcore::files`): the
/// answer, or `None` for another message. The caller is the kernel's name for it,
/// never anything it sent.
fn files_request(state: &mut State, bytes: &[u8], pid: u64) -> Option<Vec<u8>> {
    let caller = name_of(pid);
    let reply = state.files.request(&caller, bytes, now_ms())?;
    if reply.wake {
        wake_shell();
    }
    Some(reply.bytes)
}

/// Interrupts the shell's line for a person's Stop or Esc (`why`), only
/// while the shell runs that person's `line`: what to tell them.
pub(super) fn interrupt_shell(
    state: &State,
    line: deskcore::running::Line,
    why: &str,
) -> Result<String, String> {
    if !state.running.is(line) {
        println!("desktop: {why} found no line of the person's running");
        return Err(String::from("not running yet"));
    }
    match sys::shell_interrupt() {
        Ok(_) => {
            println!("desktop: {why} interrupted the shell's line");
            Ok(String::from("stopped"))
        }
        Err(error) => {
            println!("desktop: {why} could not interrupt the shell ({error:?})");
            Err(String::from("the line could not be stopped"))
        }
    }
}

/// An app's `effects put` and `effects results`, and the shell's `effects
/// take` and `effects done` (P4, `deskcore::effects`): its answer, or `None`
/// for a window client's message. The app is the kernel's name for the
/// caller, never anything it sent.
/// Before the shell takes new work: a Terminal line whose result never came is
/// finished, so the Terminal does not wait for ever and a later Stop is not
/// taken for it.
fn finish_stale(state: &mut State) {
    let stale = state.running.take_stale();
    finish_lost(state, stale);
}

/// Finishes the Terminal line's tickets `stale`, whose result never came.
pub(super) fn finish_lost(state: &mut State, stale: Vec<u32>) {
    if stale.is_empty() {
        return;
    }
    println!(
        "desktop: the Terminal's line had no result from the shell; {} finished",
        stale.len()
    );
    for ticket in stale {
        state.effects.finish(
            effects::STOPS_FROM,
            effects::Done {
                ticket,
                outcome: Err(String::from("its answer was lost; `show` says what ran")),
            },
        );
    }
}
