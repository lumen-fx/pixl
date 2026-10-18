//! Drives the editor with input messages, one scripted step per tick, the
//! way the window delivers them, and reports what the editor ended with.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lumenui::ecs::hierarchy::Children;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{
    ContextMenuEvent, Key, MenuClicked, Modifiers, ModifiersState, MouseWheel, NamedKey,
    PointerButton, PointerData, PointerKind, PointerMoved, PointerPressed, PointerReleased,
    PointerState,
};
use lumenui::prelude::*;
use pixl::recents::Recent;
use pixl::view::View;
use pixl::{Editor, Home, Recents, Settings, Tool};
use pixl_core::{Document, LayerId, Point, Rgba, Selection};

/// Where a pointer step lands.
#[derive(Clone, Copy, Debug)]
pub enum At {
    /// The middle of a document pixel, wherever the view puts it.
    Doc(i32, i32),
    /// A window position.
    Window(Vec2),
    /// The middle of the element with this id, where it is laid out.
    Element(&'static str),
}

/// One scripted input.
#[derive(Clone, Debug)]
pub enum Step {
    Wheel(At, f32),
    /// A key press and its release on the same tick, with shift held or not.
    Key(&'static str, bool),
    KeyDown(NamedKey),
    KeyUp(NamedKey),
    Down(At, PointerButton),
    Move(At),
    Up(At, PointerButton),
    /// A pen press, move or release at a pressure.
    PenDown(At, f32),
    PenMove(At, f32),
    PenUp(At),
    /// Holds or lets go of shift for the pointer steps that follow.
    Shift(bool),
    /// Holds or lets go of Alt for the pointer steps that follow.
    Alt(bool),
    /// A key press with Ctrl held, and shift or not.
    Ctrl(&'static str, bool),
    /// A click on the element with this id.
    Click(&'static str),
    /// A click on the first child of the element with this id.
    ClickInside(&'static str),
    /// A click on a menu item.
    Menu(&'static str),
    /// A request for a context menu on the element with this id, at its
    /// middle.
    ContextMenu(&'static str),
    /// A named key pressed and released with Alt held.
    AltKey(NamedKey),
    /// A control writing a number to a signal, as a slider does.
    Number(&'static str, f64),
    /// A control writing text to a signal, as a dropdown does.
    Text(&'static str, &'static str),
    /// A control writing a flag to a signal, as a checkbox does.
    Flag(&'static str, bool),
    /// Waits this many milliseconds of real time, for what runs on a clock.
    Sleep(u64),
    /// Runs a closure on the editor.
    Edit(fn(&mut Editor)),
    /// Waits a tick at a time, up to [`UNTIL_TICKS`] ticks of
    /// [`UNTIL_PAUSE`] each, until the editor shows what a background job
    /// finishes with.
    Until(fn(&Editor) -> bool),
    /// Waits a tick.
    Wait,
}

/// The most ticks one [`Step::Until`] waits, and the pause each waiting
/// tick gives the worker threads.
pub const UNTIL_TICKS: u64 = 200;
pub const UNTIL_PAUSE: std::time::Duration = std::time::Duration::from_millis(10);

/// What the editor ended with.
pub struct Outcome {
    pub doc: Document,
    pub fg: Rgba,
    pub bg: Rgba,
    pub tool: Tool,
    pub selection: Option<Selection>,
    pub undo: Vec<String>,
    pub views: Vec<View>,
    pub frame: Frame,
    pub title: String,
    pub modified: bool,
    pub message: Option<String>,
    pub home_open: bool,
    pub recents: Vec<Recent>,
    /// The thumbnail file of each recent document, once drawn.
    pub thumbs: Vec<Option<PathBuf>>,
    pub settings: Settings,
    /// Whether the settings dialog is open.
    pub settings_open: bool,
    /// The frame and the layer the editor ended on.
    pub at: (usize, LayerId),
    /// Frames the performance tracking recorded, the last stroke's time to
    /// the screen, and the overlay's lines.
    pub perf_frames: u64,
    pub last_stroke: Option<std::time::Duration>,
    pub perf_lines: Vec<String>,
    /// The pixels the tool preview draws over the document.
    pub preview: usize,
}

/// The editor's state at the end of a tick.
struct Snapshot {
    doc: Document,
    fg: Rgba,
    bg: Rgba,
    tool: Tool,
    selection: Option<Selection>,
    undo: Vec<String>,
    title: String,
    modified: bool,
    message: Option<String>,
    home_open: bool,
    recents: Vec<Recent>,
    thumbs: Vec<Option<PathBuf>>,
    settings: Settings,
    settings_open: bool,
    at: (usize, LayerId),
    perf_frames: u64,
    last_stroke: Option<std::time::Duration>,
    perf_lines: Vec<String>,
    preview: usize,
}

fn key(name: &str) -> Key {
    Key::from(name)
}

/// Runs `editor` (the app, possibly with a document inserted) through
/// `steps`, one per tick from the fifth on; a [`Step::Until`] holds its
/// turn until it is met.
/// Puts the pointer where a scripted pointer step happens, as a window does
/// before it sends the step, so Lumen hit-tests it: hover, press and click
/// all follow. `down` is the primary button's state after the step, or
/// `None` for a move.
fn press(state: &mut PointerState, at: Vec2, down: Option<bool>) {
    state.position = Some(at);
    if let Some(down) = down {
        state.primary_down = down;
    }
}

pub fn drive(app: App, steps: Vec<Step>) -> Outcome {
    let _turn = super::turn();
    let waits = steps.iter().filter(|s| matches!(s, Step::Until(_))).count() as u64;
    let views: Arc<Mutex<Vec<View>>> = Arc::default();
    let end: Arc<Mutex<Option<Snapshot>>> = Arc::default();
    let (views_out, end_out) = (Arc::clone(&views), Arc::clone(&end));
    let n = steps.len() as u64;
    let frame = app
        .add_systems(
            TickStage::Systems,
            (move |mut state: Local<(usize, (usize, u64))>,
                   mut held: Local<(bool, bool)>,
                   mut editor: ResMut<Editor>,
                   view: Res<View>,
                   (mut modifiers, mut pointer_state): (
                ResMut<ModifiersState>,
                ResMut<PointerState>,
            ),
                   mut wheel: MessageWriter<MouseWheel>,
                   mut keys: MessageWriter<KeyPressed>,
                   mut keys_up: MessageWriter<KeyReleased>,
                   mut down: MessageWriter<PointerPressed>,
                   mut moved: MessageWriter<PointerMoved>,
                   mut up: MessageWriter<PointerReleased>,
                   mut clicks: MessageWriter<ClickEvent>,
                   (mut menu, mut context): (
                MessageWriter<MenuClicked>,
                MessageWriter<ContextMenuEvent>,
            ),
                   elements: Query<(Entity, &LumenId)>,
                   children: Query<&Children>,
                   mut signals: Signals| {
                let (tick, cursor) = &mut *state;
                *tick += 1;
                let (shift, alt) = &mut *held;
                modifiers.0.shift = *shift;
                modifiers.0.alt = *alt;
                if *tick < 5 {
                    return;
                }
                // The step in turn, and the ticks it has waited.
                let (index, waited) = cursor;
                let Some(step) = steps.get(*index) else {
                    return;
                };
                if let Step::Until(ready) = step
                    && !ready(&editor)
                    && *waited < UNTIL_TICKS
                {
                    *waited += 1;
                    std::thread::sleep(UNTIL_PAUSE);
                    return;
                }
                *index += 1;
                *waited = 0;
                let place = |at: At| match at {
                    At::Doc(x, y) => pixl::tools::screen_of(&view, Point::new(x, y)),
                    At::Window(w) => w,
                    At::Element(id) => {
                        let r = dom::get_by_id(id)
                            .and_then(|n| n.rect())
                            .unwrap_or_else(|| panic!("no element {id} laid out"));
                        Vec2::new(r.client_x + r.width / 2.0, r.client_y + r.height / 2.0)
                    }
                };
                let pen = |buttons: u32, pressure: f32| PointerData {
                    id: 2,
                    kind: PointerKind::Pen,
                    pressure,
                    ..PointerData::mouse(buttons)
                };
                let mods = Modifiers {
                    shift: *shift,
                    ..Modifiers::default()
                };
                match step.clone() {
                    Step::Wheel(at, dy) => {
                        wheel.write(MouseWheel {
                            delta: Vec2::new(0.0, dy),
                            position: place(at),
                            local: None,
                        });
                    }
                    Step::Key(name, with_shift) => {
                        let m = Modifiers {
                            shift: with_shift,
                            ..Modifiers::default()
                        };
                        keys.write(KeyPressed {
                            key: key(name),
                            modifiers: m,
                            repeat: false,
                        });
                        keys_up.write(KeyReleased {
                            key: key(name),
                            modifiers: m,
                        });
                    }
                    Step::KeyDown(k) => {
                        keys.write(KeyPressed {
                            key: Key::Named(k),
                            modifiers: mods,
                            repeat: false,
                        });
                    }
                    Step::KeyUp(k) => {
                        keys_up.write(KeyReleased {
                            key: Key::Named(k),
                            modifiers: mods,
                        });
                    }
                    Step::Down(at, button) => {
                        press(&mut pointer_state, place(at), Some(true));
                        down.write(PointerPressed {
                            position: place(at),
                            button,
                            local: None,
                            pointer: PointerData::mouse(button.bit()),
                        });
                    }
                    Step::Move(at) => {
                        press(&mut pointer_state, place(at), None);
                        moved.write(PointerMoved {
                            position: place(at),
                            local: None,
                            pointer: PointerData::mouse(1),
                        });
                    }
                    Step::Up(at, button) => {
                        press(&mut pointer_state, place(at), Some(false));
                        up.write(PointerReleased {
                            position: place(at),
                            button,
                            local: None,
                            pointer: PointerData::mouse(0),
                        });
                    }
                    Step::PenDown(at, p) => {
                        press(&mut pointer_state, place(at), Some(true));
                        down.write(PointerPressed {
                            position: place(at),
                            button: PointerButton::Primary,
                            local: None,
                            pointer: pen(1, p),
                        });
                    }
                    Step::PenMove(at, p) => {
                        press(&mut pointer_state, place(at), None);
                        moved.write(PointerMoved {
                            position: place(at),
                            local: None,
                            pointer: pen(1, p),
                        });
                    }
                    Step::PenUp(at) => {
                        press(&mut pointer_state, place(at), Some(false));
                        up.write(PointerReleased {
                            position: place(at),
                            button: PointerButton::Primary,
                            local: None,
                            pointer: pen(0, 0.0),
                        });
                    }
                    Step::Alt(held) => {
                        *alt = held;
                        modifiers.0.alt = held;
                    }
                    Step::Shift(held) => {
                        *shift = held;
                        modifiers.0.shift = held;
                    }
                    Step::Ctrl(name, with_shift) => {
                        let m = Modifiers {
                            shift: with_shift,
                            ctrl: true,
                            ..Modifiers::default()
                        };
                        keys.write(KeyPressed {
                            key: key(name),
                            modifiers: m,
                            repeat: false,
                        });
                        keys_up.write(KeyReleased {
                            key: key(name),
                            modifiers: m,
                        });
                    }
                    Step::Click(id) | Step::ClickInside(id) => {
                        let mut entity = elements
                            .iter()
                            .find(|(_, i)| i.0 == id)
                            .map(|(e, _)| e)
                            .unwrap_or_else(|| panic!("no element {id}"));
                        if matches!(step, Step::ClickInside(_)) {
                            entity = children
                                .get(entity)
                                .ok()
                                .and_then(|c| c.first().copied())
                                .unwrap_or_else(|| panic!("{id} has no children"));
                        }
                        clicks.write(ClickEvent {
                            entity,
                            position: Vec2::ZERO,
                            button: PointerButton::Primary,
                            local: None,
                        });
                        // A real click ends with the button coming up.
                        up.write(PointerReleased {
                            position: Vec2::ZERO,
                            button: PointerButton::Primary,
                            local: None,
                            pointer: PointerData::mouse(0),
                        });
                    }
                    Step::Menu(id) => {
                        menu.write(MenuClicked { id: id.to_string() });
                    }
                    Step::ContextMenu(id) => {
                        let entity = elements
                            .iter()
                            .find(|(_, i)| i.0 == id)
                            .map(|(e, _)| e)
                            .unwrap_or_else(|| panic!("no element {id}"));
                        context.write(ContextMenuEvent {
                            entity,
                            position: place(At::Element(id)),
                            button: PointerButton::Secondary,
                            local: None,
                        });
                    }
                    Step::AltKey(k) => {
                        let m = Modifiers {
                            alt: true,
                            ..Modifiers::default()
                        };
                        keys.write(KeyPressed {
                            key: Key::Named(k),
                            modifiers: m,
                            repeat: false,
                        });
                        keys_up.write(KeyReleased {
                            key: Key::Named(k),
                            modifiers: m,
                        });
                    }
                    Step::Number(name, v) => signals.set(name, v),
                    Step::Text(name, v) => signals.set(name, v),
                    Step::Flag(name, v) => signals.set(name, v),
                    Step::Sleep(ms) => std::thread::sleep(std::time::Duration::from_millis(ms)),
                    Step::Edit(f) => f(&mut editor),
                    Step::Until(_) | Step::Wait => {}
                }
            })
            // After the tick's introspection snapshot, which an element's
            // place is read from: before it, an element laid out on the
            // last tick has no place yet.
            .after(lumenui::lumen_script::runtime::ScriptSet::Dispatch)
            .before(pixl::keymap::read_keys)
            .before(pixl::input::navigate)
            .before(pixl::panels::panel_clicks)
            .before(pixl::files::file_commands),
        )
        .add_systems(
            TickStage::Systems,
            (move |view: Res<View>,
                   editor: Res<Editor>,
                   home: Res<Home>,
                   mut recents: ResMut<Recents>,
                   settings: Res<Settings>,
                   (perf, preview): (Res<pixl::perf::Perf>, Res<pixl::view::Preview>),
                   signals: Signals,
                   mut tick: Local<usize>| {
                *tick += 1;
                // Thumbnails draw on worker threads; the harness waits for
                // them so what it reports is settled.
                recents.finish_thumbnails();
                if *tick >= 5 {
                    views_out.lock().unwrap().push(view.clone());
                }
                *end_out.lock().unwrap() = Some(Snapshot {
                    doc: editor.doc.clone(),
                    fg: editor.fg,
                    bg: editor.bg,
                    tool: editor.tool,
                    selection: editor.selection.clone(),
                    undo: editor.history.undo_labels().map(str::to_string).collect(),
                    title: editor.title(),
                    modified: editor.is_modified(),
                    message: editor.message.clone(),
                    home_open: home.open,
                    recents: recents.entries.clone(),
                    thumbs: recents
                        .entries
                        .iter()
                        .map(|e| recents.thumb_path(e))
                        .collect(),
                    settings: settings.clone(),
                    settings_open: signals.get::<bool>("settings_open").unwrap_or(false),
                    at: (editor.frame, editor.layer),
                    perf_frames: perf.count,
                    last_stroke: perf.last_stroke,
                    perf_lines: (0..6)
                        .filter_map(|i| signals.get::<String>(&format!("perf_line_{i}")))
                        .collect(),
                    preview: preview.pixels.len(),
                });
            })
            .after(pixl::view::draw_stage)
            .after(pixl::home::show_home),
        )
        .screenshot(HeadlessOptions {
            dpr: 1.0,
            ticks: Some(8 + n + waits * UNTIL_TICKS),
        })
        .expect("the editor runs");
    let end = end.lock().unwrap().take().expect("the editor ran");
    let views = views.lock().unwrap().clone();
    Outcome {
        doc: end.doc,
        fg: end.fg,
        bg: end.bg,
        tool: end.tool,
        selection: end.selection,
        undo: end.undo,
        views,
        frame,
        title: end.title,
        modified: end.modified,
        message: end.message,
        home_open: end.home_open,
        recents: end.recents,
        thumbs: end.thumbs,
        settings: end.settings,
        settings_open: end.settings_open,
        at: end.at,
        perf_frames: end.perf_frames,
        last_stroke: end.last_stroke,
        perf_lines: end.perf_lines,
        preview: end.preview,
    }
}

/// The editor on the CPU renderer over `doc`.
pub fn editor_over(doc: Document) -> App {
    pixl::editor(Source::Disk {
        dir: super::app_dir("cpu"),
    })
    .insert_resource(Editor::new(doc))
}
