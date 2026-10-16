//! Pointer and keyboard input over the stage: zoom, pan, and the tools.

use lumenui::ecs::system::SystemParam;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{
    Key, MouseWheel, NamedKey, PointerButton, PointerData, PointerMoved, PointerPressed,
    PointerReleased,
};
use lumenui::prelude::*;

use crate::editor::Editor;
use crate::view::View;

/// One pointer event, in the order it happened.
#[derive(Clone, Copy, Debug)]
pub enum Pointer {
    Down {
        at: Vec2,
        button: PointerButton,
        data: PointerData,
    },
    Move {
        at: Vec2,
        data: PointerData,
    },
    Up {
        at: Vec2,
        button: PointerButton,
        data: PointerData,
    },
}

impl Pointer {
    pub fn at(&self) -> Vec2 {
        match self {
            Pointer::Down { at, .. } | Pointer::Move { at, .. } | Pointer::Up { at, .. } => *at,
        }
    }

    pub fn data(&self) -> PointerData {
        match self {
            Pointer::Down { data, .. } | Pointer::Move { data, .. } | Pointer::Up { data, .. } => {
                *data
            }
        }
    }
}

/// The pointer messages of a tick.
#[derive(SystemParam)]
pub struct PointerInput<'w, 's> {
    pressed: MessageReader<'w, 's, PointerPressed>,
    moved: MessageReader<'w, 's, PointerMoved>,
    released: MessageReader<'w, 's, PointerReleased>,
}

impl PointerInput<'_, '_> {
    /// This tick's pointer events, in the order they happened.
    pub fn events(&mut self) -> Vec<Pointer> {
        pointer_events(&mut self.pressed, &mut self.moved, &mut self.released)
    }
}

/// The key messages of a tick, and whether a text field has the keys.
#[derive(SystemParam)]
pub struct KeyInput<'w, 's> {
    pub pressed: MessageReader<'w, 's, KeyPressed>,
    pub released: MessageReader<'w, 's, KeyReleased>,
    focused: Query<'w, 's, (), (With<Focused>, With<TextInput>)>,
}

impl KeyInput<'_, '_> {
    /// Whether keyboard focus is in a text field, where keys type text.
    pub fn typing(&self) -> bool {
        !self.focused.is_empty()
    }
}

/// This tick's pointer events, merged from the three message kinds and put
/// back in the order they happened: a quick stroke can press, move and
/// release inside one frame.
pub fn pointer_events(
    pressed: &mut MessageReader<PointerPressed>,
    moved: &mut MessageReader<PointerMoved>,
    released: &mut MessageReader<PointerReleased>,
) -> Vec<Pointer> {
    let mut out: Vec<(u64, usize, Pointer)> = Vec::new();
    let mut seq = 0;
    let mut push = |order: u64, p: Pointer| {
        out.push((order, seq, p));
        seq += 1;
    };
    for m in moved.read() {
        push(
            m.pointer.order,
            Pointer::Move {
                at: m.position,
                data: m.pointer,
            },
        );
    }
    for p in pressed.read() {
        push(
            p.pointer.order,
            Pointer::Down {
                at: p.position,
                button: p.button,
                data: p.pointer,
            },
        );
    }
    for r in released.read() {
        push(
            r.pointer.order,
            Pointer::Up {
                at: r.position,
                button: r.button,
                data: r.pointer,
            },
        );
    }
    // Unstamped events (order 0) keep the order they were read in.
    if out.iter().all(|(o, _, _)| *o != 0) {
        out.sort_by_key(|(o, s, _)| (*o, *s));
    }
    out.into_iter().map(|(_, _, p)| p).collect()
}

/// Panning in progress, and whether the space bar is held for it.
#[derive(Resource, Default, Debug)]
pub struct Navigation {
    pub space_held: bool,
    /// The button that is dragging the view, and where it last was.
    pub panning: Option<(PointerButton, Vec2)>,
    /// The last pointer position over the window.
    pub hover: Option<Vec2>,
}

/// The character a key press carries, lowercased, if any.
pub fn key_char(key: &Key) -> Option<String> {
    match key {
        Key::Character(c) => Some(c.to_lowercase()),
        Key::Named(_) => None,
    }
}

/// Zooms with the wheel and the zoom keys, pans with a middle-button drag
/// or a drag while the space bar is held. Returns, through the
/// [`Navigation`] resource, whether a pan holds the pointer so the tools
/// leave it alone.
pub fn navigate(
    mut view: ResMut<View>,
    mut nav: ResMut<Navigation>,
    editor: Res<Editor>,
    mut wheel: MessageReader<MouseWheel>,
    mut keys: KeyInput,
    mut pointer: PointerInput,
    mut status: Signals,
) {
    for w in wheel.read() {
        if !view.contains(w.position) || w.delta.y == 0.0 {
            continue;
        }
        let anchor = view.stage_point(w.position);
        let zoom = view.step(w.delta.y < 0.0);
        view.zoom_at(zoom, anchor);
    }
    let text = keys.typing();
    for k in keys.pressed.read() {
        if text {
            continue;
        }
        if k.key == Key::Named(NamedKey::Space) {
            nav.space_held = true;
            continue;
        }
        if k.modifiers.ctrl || k.modifiers.super_ || k.modifiers.alt {
            continue;
        }
        let anchor = nav
            .hover
            .filter(|h| view.contains(*h))
            .map_or_else(|| view.middle(), |h| view.stage_point(h));
        match key_char(&k.key).as_deref() {
            Some("+" | "=") => {
                let z = view.step(true);
                view.zoom_at(z, anchor);
            }
            Some("-") => {
                let z = view.step(false);
                view.zoom_at(z, anchor);
            }
            Some("0") => view.fit(editor.doc.width(), editor.doc.height()),
            Some("1") => view.zoom_at(1.0, anchor),
            _ => {}
        }
    }
    for k in keys.released.read() {
        if k.key == Key::Named(NamedKey::Space) {
            nav.space_held = false;
        }
    }
    for p in pointer.events() {
        nav.hover = Some(p.at());
        match p {
            Pointer::Down { at, button, .. } => {
                let drags = button == PointerButton::Middle
                    || (button == PointerButton::Primary && nav.space_held);
                if drags && view.contains(at) && nav.panning.is_none() {
                    nav.panning = Some((button, at));
                }
            }
            Pointer::Move { at, .. } => {
                if let Some((button, last)) = nav.panning {
                    let d = at - last;
                    view.pan(f64::from(d.x), f64::from(d.y));
                    nav.panning = Some((button, at));
                }
            }
            Pointer::Up { button, .. } => {
                if nav.panning.is_some_and(|(b, _)| b == button) {
                    nav.panning = None;
                }
            }
        }
    }
    let zoom = format!("{}%", (view.zoom * 100.0 * 10.0).round() / 10.0);
    if status.get::<String>("status_zoom").as_deref() != Some(zoom.as_str()) {
        status.set("status_zoom", zoom);
    }
}
