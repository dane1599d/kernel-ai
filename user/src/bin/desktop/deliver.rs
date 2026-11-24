//! The desk's effects carried out: focus, Start, the pane, windows.

use super::*;

/// Hands the desk's effects to the clients they concern: messages, and input
/// posted to their pools' event rings.
pub(super) fn deliver(state: &mut State, effects: &[Effect]) {
    for client in &mut state.clients {
        client.apply(effects);
        for (pool, event) in client.take_posts() {
            post(&state.pools, client.pid, pool, &event);
        }
    }
    let now = now_ms();
    for effect in effects {
        if let Effect::Answer { id, yes } = *effect {
            answer_ask(state, id, yes);
            continue;
        }
        if let Effect::TookOver { window } = *effect {
            agent::took_over(state, window);
            continue;
        }
        let changed = match *effect {
            Effect::PanelClick { at } => {
                use paint_panel::PaneClick;
                match state.painter.pane_click(&state.desk, &state.panel, at) {
                    // The send item: Enter; it and the status row's Stop
                    // while Kai thinks: exactly Esc.
                    Some(PaneClick::Send | PaneClick::Stop) => {
                        let before = state.panel.waiting();
                        let changed = state.panel.send_item(now);
                        pane::after_panel_key(state, before);
                        changed
                    }
                    // A done line's undo or show, sent as though typed.
                    Some(PaneClick::Link(word)) => {
                        // Undo takes back the line beside it, by its entry,
                        // never the newest of another line's.
                        let line = match (word, state.panel.entry()) {
                            ("undo", Some(id)) => format!("undo {id}"),
                            _ => String::from(word),
                        };
                        state.panel.send(line, now);
                        println!("desktop: the panel's {word} was clicked");
                        wake_shell();
                        true
                    }
                    // A held send's undo: that send, by its entry.
                    Some(PaneClick::UndoSend(id)) => {
                        state.panel.send(format!("undo {id}"), now);
                        println!("desktop: a held send's undo was clicked");
                        wake_shell();
                        true
                    }
                    None => false,
                }
            }
            // The taskbar's "more": All apps.
            Effect::ShowApps => {
                apps::show_all(state);
                false
            }
            // Start's host: it opens with the catalog, and takes the keys.
            Effect::Start(open) => {
                match open {
                    true => apps::open(state),
                    false => apps::close(state),
                }
                state.panel.follow_focus(effect, &state.desk)
            }
            Effect::StartClick { at } => {
                apps::click(state, at, now);
                false
            }
            // The wheel over the pane's history: three lines a detent.
            // A send's card is up: its text in the pane scrolls.
            Effect::PreviewScroll { pages, detents } => {
                preview::scroll(state, pages, detents);
                false
            }
            Effect::PaneWheel(detents) => {
                let area = paint_chat::chat_area(&state.desk);
                if state.painter.wheel_history(&mut state.chat, area, detents) {
                    println!("desktop: the wheel moved the pane's history");
                    state.extra.push(area);
                }
                false
            }
            // The wheel over Start: a row a detent.
            Effect::StartWheel(detents) => {
                apps::wheel(state, detents);
                false
            }
            // A pin with no window: its `open <name>`, as typed.
            Effect::PanelLine(ref line) => {
                println!("desktop: the taskbar sent {line:?}");
                state.panel.send(line.clone(), now);
                wake_shell();
                true
            }
            Effect::Locks { caps } => {
                state.panel.set_caps(caps);
                false
            }
            Effect::PanelKey { usage, down } if apps::key(state, usage, down, now) => false,
            // PageUp and PageDown scroll the pane's history by a page.
            Effect::PanelKey { usage, down }
                if usage == pane::PAGE_UP || usage == pane::PAGE_DOWN =>
            {
                if down {
                    pane::page(state, usage == pane::PAGE_UP);
                }
                false
            }
            // A line edit repaints the input alone; a new state the pane.
            Effect::PanelKey { usage, down } => {
                let before = state.panel.waiting();
                let shown = state.panel.state().clone();
                let changed = state.panel.key(usage, down, now);
                pane::after_panel_key(state, before);
                match *state.panel.state() != shown {
                    true => true,
                    false => {
                        if changed {
                            let line = paint_panel::PanelChange::LineEdit;
                            let damage = paint_panel::panel_damage(&state.desk, line);
                            state.extra.extend(damage);
                        }
                        false
                    }
                }
            }
            Effect::Focus(focus) => {
                if focus != Focus::Panel {
                    apps::close(state);
                }
                // The pane's input listens unless Start has the keys.
                state.panel.follow_focus(effect, &state.desk)
            }
            _ => false,
        };
        if changed {
            state.extra.push(state.desk.pane_area());
        }
    }
}
