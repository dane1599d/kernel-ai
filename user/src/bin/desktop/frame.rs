//! A frame: the tray and the panel's bar refreshed, the damage drawn and
//! copied to the screen, and the clients' events posted.

use super::*;

/// Logs windows that opened, moved or closed, and focus changes, once each.
pub(super) fn report(state: &mut State) {
    for window in state.desk.windows() {
        match state.known.iter_mut().find(|(id, _)| *id == window.id) {
            None => {
                let f = window.frame;
                println!(
                    "desktop: window {} \"{}\" opened at {},{} size {}x{}",
                    window.id.0, window.title, f.x, f.y, f.w, f.h
                );
                state.known.push((window.id, f));
            }
            Some((_, frame)) if *frame != window.frame => {
                let f = window.frame;
                println!(
                    "desktop: window {} moved to {},{} size {}x{}",
                    window.id.0, f.x, f.y, f.w, f.h
                );
                *frame = f;
            }
            Some(_) => {}
        }
    }
    let desk = &state.desk;
    state.known.retain(|(id, _)| {
        let open = desk.window(*id).is_some();
        if !open {
            println!("desktop: window {} closed", id.0);
        }
        open
    });
    let focus = state.desk.focus();
    if focus != state.focus {
        match focus {
            Focus::Window(id) => println!("desktop: window {} has focus", id.0),
            other => println!("desktop: focus {other:?}"),
        }
        state.focus = focus;
    }
}

/// Posts `work` to the kernel's event box, which wakes the shell to take what
/// was queued for it now, not at its next look.
pub(super) fn wake_shell() {
    if let Err(error) = sys::event_post("work") {
        println!("desktop: cannot wake the shell: {error:?}");
    }
}

/// Posts the window events and idle minutes since last round to the kernel's
/// event box, for the shell's standing instructions.
pub(super) fn post_events(state: &mut State) {
    let mut events = state
        .watch
        .windows(state.desk.windows(), state.desk.focus());
    events.extend(state.watch.idle(now_ms()));
    for event in events {
        let mut line = alloc::string::String::new();
        let _ = event.text(&mut line);
        match sys::event_post(&line) {
            Ok(_) => println!("desktop: event {line}"),
            Err(error) => println!("desktop: cannot post {line}: {error:?}"),
        }
    }
}

/// Reads the model state, memory and clock once a second; the tray repaints
/// what changed before the next frame (`taskbar::damage`).
pub(super) fn refresh_bar(state: &mut State) {
    let now = sys::time_ns();
    if state.bar_at != 0 && now.saturating_sub(state.bar_at) < BAR_EVERY_NS {
        return;
    }
    state.bar_at = now.max(1);
    let Ok(m) = sys::metrics() else {
        return;
    };
    let listed = &m.processes[..(m.process_count as usize).min(m.processes.len())];
    let mib = |frames: u64| frames * 4096 / (1024 * 1024);
    let bar = Bar {
        model: listed.iter().any(|p| p.name() == "llm"),
        mem_used_mib: mib(m.total_frames - m.free_frames),
        // No wall clock (NotFound): no clock in the tray.
        utc_s: sys::clock_now().ok(),
        zone_min: state.appearance.time_zone_min,
    };
    if bar != state.bar {
        let model = bar.model != state.bar.model;
        state.bar = bar;
        // The idle status row shows the model's dot too, and the pane's
        // header its state; the tray's parts repaint as their text changes
        // (`taskbar::damage`).
        if model {
            let bar = paint_panel::PanelChange::Bar;
            let damage = paint_panel::panel_damage(&state.desk, bar);
            state.extra.extend(damage);
        }
    }
}

/// Repaints the damage into the frame, copies it to the screen and flushes it.
pub(super) fn draw(state: &mut State, screen: &Screen) {
    taskbar::damage(state);
    let mut damage = state.desk.take_damage();
    damage.append(&mut state.extra);
    if damage.is_empty() {
        return;
    }
    let bounds = Rect::new(0, 0, state.width, state.height);
    let damage: Vec<Rect> = damage
        .iter()
        .filter_map(|r| paint::reach(*r).intersect(&bounds))
        .collect();
    // Timed only for `desk-perf`: no TIME calls on the normal path.
    let timed = state.perf.is_some();
    let started = if timed { sys::time_ns() } else { 0 };
    let mut frame = state.frame.canvas();
    let (shown, pools) = (&state.shown, &state.pools);
    let scene = Scene {
        desk: &state.desk,
        bar: &state.bar,
        panel: &state.panel,
        chat: &state.chat,
        start: state.start.as_ref(),
        ask: state.card.as_ref(),
        now_ms: now_ms(),
    };
    // A window with no pixels yet shows the theme's window colour.
    let fallback = state.painter.content_fallback();
    let mut content = |id: WindowId, layer: &mut Canvas<'_>, at: Rect| {
        let Some((_, pid, buffer)) = shown.iter().find(|(w, ..)| *w == id) else {
            return false;
        };
        let Some(source) = source(pools, *pid, buffer) else {
            return false;
        };
        let mut pixels = Pixels {
            width: layer.width as i32,
            height: layer.height as i32,
            stride: layer.stride_px as usize,
            data: layer.pixels,
        };
        pixels.fill(at, at, fallback);
        pixels.draw(&source, (at.x, at.y), at);
        true
    };
    for &clip in &damage {
        // The Apps sheet is the scene's: under the ask card, never over it.
        state.painter.paint(&mut frame, clip, &scene, &mut content);
        if state.draws_cursor {
            state.painter.cursor(&mut frame, clip, state.desk.pointer());
        }
    }
    let rendered = if timed { sys::time_ns() } else { 0 };
    let shown = present(state, screen, &damage);
    let rects: Vec<FlushRect> = shown
        .iter()
        .map(|r| FlushRect {
            x: r.x as u32,
            y: r.y as u32,
            w: r.w as u32,
            h: r.h as u32,
        })
        .collect();
    for chunk in rects.chunks(kabi::display::MAX_FLUSH) {
        let _ = sys::display_flush(chunk);
    }
    if timed {
        let flushed = sys::time_ns();
        perf::drew(state, rendered - started, flushed - rendered, flushed);
    }
    // A card up is on the screen from now: its quiet time can start.
    state.desk.card_drawn(now_ms());
    if !state.drawn {
        state.drawn = true;
        println!("desktop: drew the desk at {}x{}", state.width, state.height);
    }
}

/// A client's buffer, read where its pool is mapped. `None` past the pool.
fn source<'a>(pools: &'a [Pool], pid: u64, buffer: &Buffer) -> Option<Source<'a>> {
    let pool = pools
        .iter()
        .find(|p| p.pid == pid && p.pool == buffer.pool)?;
    let words = buffer.stride as usize / 4;
    let len = words * buffer.height as usize;
    let end = buffer.offset as usize + len * 4;
    if end > pool.size as usize {
        return None;
    }
    // SAFETY: the pool is mapped read-only at `address` for `size` bytes, and the
    // buffer lies inside it (checked above and at `pool.buffer`). The client may
    // write it while it is read: the frame may tear, nothing worse.
    let data = unsafe {
        core::slice::from_raw_parts((pool.address + u64::from(buffer.offset)) as *const u32, len)
    };
    Some(Source {
        data,
        width: buffer.width as i32,
        height: buffer.height as i32,
        stride: words,
        alpha: buffer.format == Format::Argb8888,
    })
}

/// Puts the logical `damage` of the frame on the screen and returns what it
/// covers there, in physical px: the same rectangles at scale 1.0, else each
/// grown to the pixels a resample of it reaches (`deskcore::upscale`). The
/// one place logical px become physical.
fn present(state: &State, screen: &Screen, damage: &[Rect]) -> Vec<Rect> {
    if state.scale == deskcore::scale::ONE {
        copy_to_screen(&state.frame.px, state.width, screen, damage);
        return damage.to_vec();
    }
    let (pw, ph) = (screen.info.width as i32, screen.info.height as i32);
    let words = (screen.info.stride / 4) as usize;
    // SAFETY: the kernel mapped the back buffer's `pages` pages at `address`.
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            screen.address as *mut u32,
            screen.info.pages as usize * 1024,
        )
    };
    let src = deskcore::upscale::Source {
        px: &state.frame.px,
        width: state.width,
        height: state.height,
    };
    let mut dst = deskcore::upscale::Target {
        px: pixels,
        stride: words,
        width: pw,
        height: ph,
    };
    let mut covered = Vec::with_capacity(damage.len());
    for r in damage {
        if let Some(rect) = deskcore::upscale::physical(*r, state.scale, pw, ph) {
            deskcore::upscale::resample(&src, &mut dst, state.scale, rect);
            covered.push(rect);
        }
    }
    covered
}

fn copy_to_screen(frame: &[u32], width: i32, screen: &Screen, damage: &[Rect]) {
    let words = (screen.info.stride / 4) as usize;
    // SAFETY: the kernel mapped the back buffer's `pages` pages at `address`.
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            screen.address as *mut u32,
            screen.info.pages as usize * 1024,
        )
    };
    for r in damage {
        for y in r.y..r.bottom() {
            let from = (y * width + r.x) as usize;
            let to = y as usize * words + r.x as usize;
            let w = r.w as usize;
            if let (Some(src), Some(dst)) = (frame.get(from..from + w), pixels.get_mut(to..to + w))
            {
                dst.copy_from_slice(src);
            }
        }
    }
}

/// The hardware cursor's image: the arrow the frame draws on GOP.
pub(super) fn arrow(palette: &Palette) -> [u32; 64 * 64] {
    let mut image = [0u32; 64 * 64];
    for (i, pixel) in image.iter_mut().enumerate() {
        let (x, y) = ((i % 64) as i32, (i / 64) as i32);
        *pixel = paint::arrow(x, y, palette).unwrap_or(0);
    }
    image
}
