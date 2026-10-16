//! Drives the editor with input messages, one scripted step per tick, the
//! way the window delivers them, and reports what the editor ended with.

use std::sync::{Arc, Mutex};

use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{
    Key, MenuClicked, Modifiers, ModifiersState, MouseWheel, NamedKey, PointerButton, PointerData,
    PointerKind, PointerMoved, PointerPressed, PointerReleased,
};
use lumenui::prelude::*;
use pixl::view::View;
use pixl::{Editor, Tool};
use pixl_core::{Document, Point, Rgba, Selection};

/// Where a pointer step lands.
#[derive(Clone, Copy, Debug)]
pub enum At {
    /// The middle of a document pixel, wherever the view puts it.
    Doc(i32, i32),
    /// A window position.
    Window(Vec2),
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
    /// A key press with Ctrl held, and shift or not.
    Ctrl(&'static str, bool),
    /// A click on the element with this id.
    Click(&'static str),
    /// A click on a menu item.
    Menu(&'static str),
    /// A control writing a number to a signal, as a slider does.
    Number(&'static str, f64),
    /// A control writing text to a signal, as a dropdown does.
    Text(&'static str, &'static str),
    /// Runs a closure on the editor.
    Edit(fn(&mut Editor)),
    /// Waits a tick.
    Wait,
}

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
}

/// The editor's state at the end of a tick.
struct Snapshot {
    doc: Document,
    fg: Rgba,
    bg: Rgba,
    tool: Tool,
    selection: Option<Selection>,
    undo: Vec<String>,
}

fn key(name: &str) -> Key {
    Key::from(name)
}

/// Runs `editor` (the app, possibly with a document inserted) through
/// `steps`, one per tick from the fifth on.
pub fn drive(app: App, steps: Vec<Step>) -> Outcome {
    let _turn = super::turn();
    let views: Arc<Mutex<Vec<View>>> = Arc::default();
    let end: Arc<Mutex<Option<Snapshot>>> = Arc::default();
    let (views_out, end_out) = (Arc::clone(&views), Arc::clone(&end));
    let n = steps.len() as u64;
    let frame = app
        .add_systems(
            TickStage::Systems,
            (move |mut tick: Local<usize>,
                   mut shift: Local<bool>,
                   mut editor: ResMut<Editor>,
                   view: Res<View>,
                   mut modifiers: ResMut<ModifiersState>,
                   mut wheel: MessageWriter<MouseWheel>,
                   mut keys: MessageWriter<KeyPressed>,
                   mut keys_up: MessageWriter<KeyReleased>,
                   mut down: MessageWriter<PointerPressed>,
                   mut moved: MessageWriter<PointerMoved>,
                   mut up: MessageWriter<PointerReleased>,
                   mut clicks: MessageWriter<ClickEvent>,
                   mut menu: MessageWriter<MenuClicked>,
                   elements: Query<(Entity, &LumenId)>,
                   mut signals: Signals| {
                *tick += 1;
                modifiers.0.shift = *shift;
                let Some(step) = tick.checked_sub(5).and_then(|i| steps.get(i)) else {
                    return;
                };
                let place = |at: At| match at {
                    At::Doc(x, y) => pixl::tools::screen_of(&view, Point::new(x, y)),
                    At::Window(w) => w,
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
                        down.write(PointerPressed {
                            position: place(at),
                            button,
                            local: None,
                            pointer: PointerData::mouse(button.bit()),
                        });
                    }
                    Step::Move(at) => {
                        moved.write(PointerMoved {
                            position: place(at),
                            local: None,
                            pointer: PointerData::mouse(1),
                        });
                    }
                    Step::Up(at, button) => {
                        up.write(PointerReleased {
                            position: place(at),
                            button,
                            local: None,
                            pointer: PointerData::mouse(0),
                        });
                    }
                    Step::PenDown(at, p) => {
                        down.write(PointerPressed {
                            position: place(at),
                            button: PointerButton::Primary,
                            local: None,
                            pointer: pen(1, p),
                        });
                    }
                    Step::PenMove(at, p) => {
                        moved.write(PointerMoved {
                            position: place(at),
                            local: None,
                            pointer: pen(1, p),
                        });
                    }
                    Step::PenUp(at) => {
                        up.write(PointerReleased {
                            position: place(at),
                            button: PointerButton::Primary,
                            local: None,
                            pointer: pen(0, 0.0),
                        });
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
                    Step::Click(id) => {
                        let entity = elements
                            .iter()
                            .find(|(_, i)| i.0 == id)
                            .map(|(e, _)| e)
                            .unwrap_or_else(|| panic!("no element {id}"));
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
                    Step::Number(name, v) => signals.set(name, v),
                    Step::Text(name, v) => signals.set(name, v),
                    Step::Edit(f) => f(&mut editor),
                    Step::Wait => {}
                }
            })
            .before(pixl::input::navigate)
            .before(pixl::panels::panel_clicks)
            .before(pixl::files::file_commands),
        )
        .add_systems(
            TickStage::Systems,
            (move |view: Res<View>, editor: Res<Editor>, mut tick: Local<usize>| {
                *tick += 1;
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
                });
            })
            .after(pixl::view::draw_stage),
        )
        .screenshot(HeadlessOptions {
            dpr: 1.0,
            ticks: Some(8 + n),
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
    }
}

/// The editor on the CPU renderer over `doc`.
pub fn editor_over(doc: Document) -> App {
    pixl::editor(Source::Disk {
        dir: super::app_dir("cpu"),
    })
    .insert_resource(Editor::new(doc))
}
