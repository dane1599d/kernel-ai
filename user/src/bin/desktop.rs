//! `desktop`, the compositor (ADR 0005 §1, P3 task 6): it claims the screen,
//! serves `kai-surface 1` to its clients, and draws their windows with
//! `deskcore`'s rules, painted by `deskcore::paint` on `gfx2d` in the theme's
//! tokens and type.
//!
//! - A client calls the `desktop` service with a run of `surfabi` messages; the
//!   reply carries the events waiting for it. `pool.create` brings its `Shm`,
//!   mapped here read-only.
//! - Input comes from the kernel's queue (INPUT_READ); Ctrl+Alt+F12 takes the
//!   screen away (a `Screen` event) and gives it back.
//! - Each frame repaints only the desk's damage, grown by what a window's
//!   shadow reaches, into a frame of its own, then copies those rectangles to
//!   the screen and flushes them.
//! - Window events (minimize, restore, focus, close) and idle minutes go to the
//!   kernel's event box for the shell's standing instructions (P3 task 11),
//!   found by `deskcore::watch`.
//! - The tray's model state, memory and clock are read once a second; a screen
//!   without a hardware cursor (GOP) gets one drawn in the frame.
//! - Everything is painted in the theme the [`Appearance`] resolves to (P4
//!   task 8): checked once per frame, and on a change the whole screen and the
//!   hardware cursor are painted again. It starts from the plan line's
//!   arguments (`theme=light`, `legible=on`, ...), each an `appearance_set`
//!   setting.
//! - While the terminal window (`term`'s) has focus, the keys go to the
//!   kernel's terminal instead: a lease (INPUT_TO_CONSOLE, `deskcore::lease`),
//!   asked for again at once after a click and every `lease::ASK_NS` in case
//!   the kernel ended it on Alt+Tab or the screen changing hands, and ended
//!   when the focus moves (P3 task 8). The
//!   kernel, not `desktop`, decides which client is the terminal: it lends the
//!   keys only for the window of a process holding TERM.
//! - A Super tap, Ctrl+Esc or the Kai button opens Start, "Ask Kai or search
//!   apps", with what `pkg` lists; `apps`, "All apps ›" or the taskbar's
//!   "more" open its All apps ([`apps`], Kai Desk PR 8; the Apps sheet of P4
//!   task 3 before it).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
#[path = "desktop/apps.rs"]
mod apps;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use deskcore::askcard::Card;
use deskcore::chat::{Chat, Who};
use deskcore::compose::{Pixels, Source};
use deskcore::desk::{Button, Desk, Effect, Focus, Input, WindowId};
use deskcore::effects;
use deskcore::geom::{Point, Rect};
use deskcore::lease::{Lease, Step};
use deskcore::paint::{self, Bar, Painter, Scene};
use deskcore::paint_chat;
use deskcore::paint_panel;
use deskcore::panel::Panel;
use deskcore::panel_wire;
use deskcore::server::{Buffer, Client, Need};
use deskcore::settings_wire as settings;
use deskcore::watch::Watch;
use gfx2d::{Canvas, Fonts, Image};
use kabi::display::{DisplayInfo, FlushRect};
use kabi::handle::{Handle, RecvInfo};
use kabi::input::{InputEvent, Kind};
use look::Palette;
use look::appearance::{self, Appearance, Shipped};
use surfabi::Format;
use user::{println, sys};

user::entry!(main);

#[path = "desktop/agent.rs"]
mod agent;
#[path = "desktop/keys.rs"]
mod keys;
#[path = "desktop/pane.rs"]
mod pane;
#[path = "desktop/perf.rs"]
mod perf;
#[path = "desktop/pools.rs"]
mod pools;
#[path = "desktop/taskbar.rs"]
mod taskbar;
use keys::{input, lease};
#[path = "desktop/deliver.rs"]
mod deliver;
use deliver::deliver;
#[path = "desktop/asks.rs"]
mod asks;
#[path = "desktop/preview.rs"]
mod preview;
use asks::{answer_ask, post, read_asks};
#[path = "desktop/requests.rs"]
mod requests;
use requests::{finish_lost, interrupt_shell, serve};
#[path = "desktop/wire.rs"]
mod wire;
use wire::{apply_need, effects_request, name_of, net_request, wire_request};
#[path = "desktop/frame.rs"]
mod frame;
use frame::{arrow, draw, post_events, refresh_bar, report, wake_shell};
#[path = "desktop/setup.rs"]
mod setup;
use setup::{
    bar_text, log_bar, logs_bar, logs_first_key, logs_wheel, pane_word, starting_appearance, theme,
    wait_for_screen,
};

/// The fonts ADR 0005 §8 names, built in until P2's files carry them.
const FONTS: Fonts = Fonts {
    sans: include_bytes!("../../../assets/fonts/InterVariable.ttf"),
    mono: include_bytes!("../../../assets/fonts/JetBrainsMono-Variable.ttf"),
};

/// How often the tray's model state, memory and clock are read.
const BAR_EVERY_NS: u64 = 1_000_000_000;

/// Waits for input or a client, in turn, so neither starves.
const INPUT_WAIT_MS: u64 = 2;
const CLIENT_WAIT_MS: u64 = 2;

struct Pool {
    pid: u64,
    pool: u32,
    /// The `Shm` the client moved here, held (with its mapping) until the pool
    /// is released: `pools::release`.
    handle: Handle,
    address: u64,
    size: u32,
    /// Mapped writable, so its header page's event ring takes the client's
    /// input (ADR 0005 §3); a client that gave read-only access gets none.
    writable: bool,
}

struct Screen {
    info: DisplayInfo,
    address: u64,
}

struct State {
    desk: Desk,
    clients: Vec<Client>,
    pools: Vec<Pool>,
    /// Each window's buffer, whose client, and the scale it was drawn at.
    shown: Vec<(WindowId, u64, Buffer, u32)>,
    /// Windows the wheel was last sent to and that have not scrolled since,
    /// each with the hashes of the rows of its picture then
    /// (`frame::wheel_rows`) and how many more pictures to check
    /// (`WHEEL_PICTURES` at first): the first picture of one is logged, and
    /// whether any of them scrolled, which tells a boot case the app took the
    /// wheel and moved its content.
    wheeled: Vec<(WindowId, Vec<u64>, usize)>,
    frame: Image,
    /// Which window's content was painted last at each px of `frame`, 0 for
    /// none or a window drawn at scale 1 (`deskcore::overlay`).
    mask: Vec<u32>,
    /// The desk's size in logical px: the screen's over `scale`.
    width: i32,
    height: i32,
    /// The scale in 120ths (`deskcore::scale`): 120 draws the frame as it is.
    scale: u32,
    painter: Painter,
    bar: Bar,
    bar_at: u64,
    /// What the tray last showed: a change to it repaints that part alone.
    tray: deskcore::tray::Shown,
    /// Damage of desktop's own: the tray's parts, the pane's, the drawn cursor.
    extra: Vec<Rect>,
    /// The screen has no hardware cursor, so the frame shows one.
    draws_cursor: bool,
    /// The first frame is on the screen.
    drawn: bool,
    /// The plan line asks for the first key to be logged (`key_log=on`), which only
    /// a test image's does: the `display-claim` case reads it to know keys reach
    /// `desktop`. No other plan logs a key, so what a person types never reaches
    /// the serial log.
    log_first_key: bool,
    /// The plan line asks for the wheel lines (`wheel_log=on`), a test image's only:
    /// `desk-wheel-app` reads them; a person's wheel logs nothing.
    log_wheel: bool,
    /// The first key has been logged since the claim: only it is, and a frame never
    /// waits on it.
    key_logged: bool,
    /// The plan line asks for the panel's result line to be logged (`log_bar=1`),
    /// which only a test image's does: `founder-walk` reads it. Otherwise only its
    /// kind is, since the line may name a file.
    log_bar: bool,
    /// The AI panel's line and the state the shell reports.
    panel: Panel,
    /// The panel's lines and what came of each, drawn on the desk.
    chat: Chat,
    /// Start, while it is open: it takes the panel's keys.
    start: Option<deskcore::start::Start>,
    /// Apps' effects waiting for the shell, and what came of them.
    effects: effects::Queue,
    /// Which person's line the shell runs, so Stop and Esc interrupt only it.
    running: deskcore::running::Running,
    /// The editor's files on their way to or from the shell, while they are.
    files: deskcore::files::Files,
    /// The Settings app's requests for the shell, and the shell's answers.
    settings: settings::Queue,
    /// Apps' network asks for the shell, and its answers (P5 task 13).
    net: deskcore::net_wire::Queue,
    /// The Models window's switches for the shell, and the shell's answers
    /// (`deskcore::models_wire`, #556).
    models: settings::Queue,
    /// The agent's views of windows, for its `ui` rows (P3 task 10).
    agent: deskcore::agent::Agent,
    /// When the panel's "thinking 4s" was last redrawn.
    ticked_ms: u64,
    /// The ask card up now, as the kernel posted it.
    card: Option<Card>,
    /// A send's text and its card: what the pane shows (`preview`).
    sends: deskcore::sendview::Slot,
    /// The last ask refused as too long to show whole: never asked again.
    too_long: Option<u64>,
    /// When the ask box was last read.
    asked_ms: u64,
    /// Frames as last logged, to report moves and closes once.
    known: Vec<(WindowId, Rect)>,
    focus: Focus,
    /// What standing instructions hear about: window events and idle.
    watch: Watch,
    /// The keyboard lease to the kernel's terminal, as desktop knows it.
    lease: Lease,
    /// What the user chose, and what the painter paints in: kept with the
    /// painter across a lost screen, and the default (what `Painter::new`
    /// paints) at first, so the first frame does no theme work for it.
    appearance: Appearance,
    painted_in: Appearance,
    /// Frame and key timings for `desk-perf`, with `perf=on` only.
    perf: Option<perf::Perf>,
    /// The theme's name, as clients are told it (`surfabi`'s `Theme`).
    theme_name: String,
    /// The appearance clients are told (`surfabi`'s `Look`): what is painted.
    look_text: String,
}

fn main() -> i32 {
    let Some(mut painter) = Painter::new(FONTS) else {
        println!("desktop: the built-in fonts do not parse");
        return 1;
    };
    let mut appearance = starting_appearance();
    let mut painted_in = Appearance::default();
    let mut theme_name = Appearance::default().theme.clone();
    let mut look_text = Appearance::default().encode();
    // The Kai pane and the apps' queues outlive a lost screen (H3-25).
    let mut kept = deskcore::kept::Kept::default();
    loop {
        let Some(screen) = claim(painter.palette()) else {
            if !wait_for_screen() {
                return 1;
            }
            continue;
        };
        // The desk works in logical px: the screen's size over the scale
        // (ADR 0005 §9); `frame::present` is the one place they become
        // physical.
        // The screen's size in mm is the EDID's, which only a virtio-gpu gives,
        // and QEMU's says the size of its default mode, not of the screen the
        // case asked for (1920x1080 read as 1.5x). So it is taken only when the
        // plan line says `edid=on`; otherwise the width alone picks the scale.
        let args = user::args();
        let width_mm = match user::arg(&args, "edid") {
            Some("on") => screen.info.width_mm,
            _ => 0,
        };
        let scale = deskcore::scale::for_screen(screen.info.width, width_mm);
        let width = deskcore::upscale::logical(screen.info.width as i32, scale);
        let height = deskcore::upscale::logical(screen.info.height as i32, scale);
        let carried = kept.for_claim();
        let mut state = State {
            desk: Desk::new(width, height),
            clients: Vec::new(),
            pools: Vec::new(),
            shown: Vec::new(),
            wheeled: Vec::new(),
            frame: Image::new(width, height),
            mask: vec![0; (width.max(0) * height.max(0)) as usize],
            width,
            height,
            scale,
            painter,
            bar: Bar::default(),
            bar_at: 0,
            tray: deskcore::tray::Shown::default(),
            extra: Vec::new(),
            draws_cursor: screen.info.cursor == 0,
            drawn: false,
            log_first_key: logs_first_key(),
            log_wheel: logs_wheel(),
            key_logged: false,
            log_bar: logs_bar(),
            panel: carried.panel,
            chat: carried.chat,
            start: None,
            effects: carried.effects,
            running: carried.running,
            files: carried.files,
            settings: carried.settings,
            net: carried.net,
            models: carried.models,
            agent: deskcore::agent::Agent::default(),
            ticked_ms: 0,
            card: None,
            sends: deskcore::sendview::Slot::default(),
            too_long: None,
            asked_ms: 0,
            known: Vec::new(),
            focus: Focus::Nothing,
            watch: Watch::new(now_ms()),
            lease: Lease::new(),
            appearance: appearance.clone(),
            painted_in,
            perf: perf::from_args(),
            theme_name: theme_name.clone(),
            look_text: look_text.clone(),
        };
        // The taskbar's pins: the image's own apps, until `pkg` lists more.
        state.desk.set_installed(apps::built_in_names());
        println!("desktop: claimed {width}x{height}");
        if scale != deskcore::scale::ONE {
            println!(
                "desktop: scale {scale}/120 on {}x{} px",
                screen.info.width, screen.info.height
            );
        }
        let result = run(&mut state, &screen);
        // The clients' pools go with this connection's state; they greet again.
        for pool in &state.pools {
            pools::release(pool);
        }
        painter = state.painter;
        painted_in = state.painted_in;
        appearance = state.appearance;
        theme_name = state.theme_name;
        look_text = state.look_text;
        kept = deskcore::kept::Kept {
            panel: state.panel,
            chat: state.chat,
            effects: state.effects,
            running: state.running,
            files: state.files,
            settings: state.settings,
            net: state.net,
            models: state.models,
        };
        let Some(back) = result else {
            return 1;
        };
        let _ = sys::shm_unmap(screen.address);
        println!("desktop: lost the screen to the console");
        if !back && !wait_for_screen() {
            return 1;
        }
        println!("desktop: the screen is back");
        // The draft's length, never its text: the line is the person's.
        println!(
            "desktop: the Kai pane came back with a {}-character line",
            kept.panel.text().chars().count()
        );
    }
}

/// The screen, its hardware cursor drawn in `palette`: the theme painted in
/// last, so a screen given back keeps the person's theme.
fn claim(palette: &Palette) -> Option<Screen> {
    let mut info = DisplayInfo::default();
    let shm = sys::display_claim(&mut info).ok()?;
    let address = sys::shm_map(shm, true).ok()?;
    if info.cursor != 0 {
        let _ = sys::display_cursor(&arrow(palette), (0, 0));
        let _ = sys::display_cursor_move(info.width / 2, info.height / 2);
    }
    Some(Screen { info, address })
}

/// Serves until the screen is taken: `Some(back)` then, `back` when the same
/// read already gave it back (both Ctrl+Alt+F12 presses at once); `None` on
/// an error.
fn run(state: &mut State, screen: &Screen) -> Option<bool> {
    let mut events = [InputEvent {
        time_ns: 0,
        kind: Kind::Screen,
        code: 0,
        value: 0,
    }; 32];
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let Ok(n) = sys::input_read(&mut events, INPUT_WAIT_MS) else {
            return None;
        };
        for (at, event) in events[..n].iter().enumerate() {
            if event.kind == Kind::Screen && event.value == 0 {
                let back = events[at + 1..n]
                    .iter()
                    .any(|e| e.kind == Kind::Screen && e.value == 1);
                return Some(back);
            }
            input(state, screen, event);
        }
        let mut info = RecvInfo::default();
        if let Ok(len) = sys::recv_h(&mut request, CLIENT_WAIT_MS, &mut info) {
            serve(state, screen, &request[..len.min(request.len())], &info);
        }
        report(state);
        post_events(state);
        refresh_bar(state);
        tick_panel(state);
        // A taskbar tooltip shows once its 500 ms are up.
        state.desk.tick(now_ms());
        read_asks(state);
        lease(state);
        theme(state, screen);
        draw(state, screen);
    }
}

fn now_ms() -> u64 {
    sys::time_ns() / 1_000_000
}

/// Redraws the panel's state once a second while it counts ("thinking 4s").
fn tick_panel(state: &mut State) {
    let now = now_ms();
    if paint_panel::ticks(&state.panel) && now.saturating_sub(state.ticked_ms) >= 1_000 {
        state.ticked_ms = now;
        state.extra.push(paint_panel::state_area(&state.desk));
    }
}
