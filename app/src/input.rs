//! Pointer and keyboard input over the stage: zoom, pan, and the tools.

use std::time::{Duration, Instant};

use lumenui::ecs::system::SystemParam;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{
    Key, ModifiersState, MouseWheel, NamedKey, PointerButton, PointerData, PointerMoved,
    PointerPressed, PointerReleased, WindowFocused,
};
use lumenui::lumen_core::tick::WakeDeadline;
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

/// How fast holding W, A, S or D moves the view, in screen pixels per
/// second. The speed is on screen, so the document moves as far across the
/// window at every zoom.
pub const KEY_PAN_SPEED: f64 = 900.0;

/// How much faster the view moves with Shift held.
pub const KEY_PAN_FAST: f64 = 3.0;

/// The longest step one tick of a held pan key takes. A tick that comes late
/// (the window was busy) moves the view this far at most instead of jumping.
pub const KEY_PAN_MAX_STEP: Duration = Duration::from_millis(100);

/// The pan keys being held: up (W), left (A), down (S) and right (D).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanKeys {
    pub up: bool,
    pub left: bool,
    pub down: bool,
    pub right: bool,
}

impl PanKeys {
    pub fn any(self) -> bool {
        self.up || self.left || self.down || self.right
    }

    /// The flag of the key `c` (lowercase), if it is a pan key.
    fn flag(&mut self, c: &str) -> Option<&mut bool> {
        match c {
            "w" => Some(&mut self.up),
            "a" => Some(&mut self.left),
            "s" => Some(&mut self.down),
            "d" => Some(&mut self.right),
            _ => None,
        }
    }

    /// How far the view moves, in screen pixels, while these keys are held
    /// for `dt`. The camera moves the way the key points, so the document
    /// moves the other way: W shows more of what is above.
    pub fn offset(self, dt: Duration, fast: bool) -> (f64, f64) {
        let axis = |neg: bool, pos: bool| f64::from(i8::from(neg) - i8::from(pos));
        let (x, y) = (axis(self.left, self.right), axis(self.up, self.down));
        if x == 0.0 && y == 0.0 {
            return (0.0, 0.0);
        }
        // Diagonals move at the same speed as the axes.
        let len = (x * x + y * y).sqrt();
        let speed = KEY_PAN_SPEED * if fast { KEY_PAN_FAST } else { 1.0 };
        let d = speed * dt.min(KEY_PAN_MAX_STEP).as_secs_f64() / len;
        (x * d, y * d)
    }
}

/// Panning in progress, and whether the space bar is held for it.
#[derive(Resource, Default, Debug)]
pub struct Navigation {
    pub space_held: bool,
    /// The button that is dragging the view, and where it last was.
    pub panning: Option<(PointerButton, Vec2)>,
    /// The last pointer position over the window.
    pub hover: Option<Vec2>,
    /// The W, A, S and D keys held to move the view.
    pub keys: PanKeys,
    /// When the held pan keys last moved the view.
    keys_moved: Option<Instant>,
    /// The part of a pixel the key pan has moved and the view not yet: the
    /// view moves in whole pixels.
    keys_carry: (f64, f64),
}

impl Navigation {
    /// Moves `view` for the pan keys held since the last call, at `now`.
    pub fn key_pan(&mut self, view: &mut View, now: Instant, fast: bool) {
        if !self.keys.any() {
            self.keys_moved = None;
            self.keys_carry = (0.0, 0.0);
            return;
        }
        let Some(last) = self.keys_moved.replace(now) else {
            return;
        };
        let (dx, dy) = self.keys.offset(now.saturating_duration_since(last), fast);
        let (cx, cy) = (self.keys_carry.0 + dx, self.keys_carry.1 + dy);
        let (wx, wy) = (cx.trunc(), cy.trunc());
        self.keys_carry = (cx - wx, cy - wy);
        if wx != 0.0 || wy != 0.0 {
            view.pan(wx, wy);
        }
    }
}

/// The character a key press carries, lowercased, if any.
pub fn key_char(key: &Key) -> Option<String> {
    match key {
        Key::Character(c) => Some(c.to_lowercase()),
        Key::Named(_) => None,
    }
}

/// Zooms with the wheel and the zoom keys, pans with a middle-button drag,
/// a drag while the space bar is held, or W, A, S and D held down. Returns,
/// through the [`Navigation`] resource, whether a pan holds the pointer so
/// the tools leave it alone.
#[allow(clippy::too_many_arguments)]
pub fn navigate(
    mut view: ResMut<View>,
    mut nav: ResMut<Navigation>,
    editor: Res<Editor>,
    mut wheel: MessageReader<MouseWheel>,
    mut keys: KeyInput,
    mut pointer: PointerInput,
    modifiers: Res<ModifiersState>,
    mut focus: MessageReader<WindowFocused>,
    wake: Option<Res<WakeDeadline>>,
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
    // Keys that go to a text field, or to another window, no longer move
    // the view: their release may never arrive here.
    if text || focus.read().any(|f| !f.focused) {
        nav.keys = PanKeys::default();
    }
    let held = modifiers.0;
    for k in keys.pressed.read() {
        if text {
            continue;
        }
        if k.key == Key::Named(NamedKey::Space) {
            nav.space_held = true;
            continue;
        }
        let m = k.modifiers;
        if m.ctrl || m.super_ || m.alt || held.ctrl || held.super_ || held.alt {
            continue;
        }
        if let Some(c) = key_char(&k.key)
            && let Some(held) = nav.keys.flag(&c)
        {
            *held = true;
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
        if let Some(c) = key_char(&k.key)
            && let Some(held) = nav.keys.flag(&c)
        {
            *held = false;
        }
    }
    nav.key_pan(&mut view, Instant::now(), modifiers.0.shift);
    if nav.keys.any() {
        // Keep ticking while a pan key is down, so the view keeps moving.
        crate::repaint_soon(&wake);
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

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(16);

    #[test]
    fn a_held_key_moves_at_the_speed_for_the_time_held() {
        let up = PanKeys {
            up: true,
            ..PanKeys::default()
        };
        let (x, y) = up.offset(TICK, false);
        assert_eq!(x, 0.0);
        assert!((y - KEY_PAN_SPEED * 0.016).abs() < 1e-9);
        let (_, fast) = up.offset(TICK, true);
        assert!((fast - y * KEY_PAN_FAST).abs() < 1e-9);
    }

    #[test]
    fn a_late_tick_moves_no_further_than_the_cap() {
        let left = PanKeys {
            left: true,
            ..PanKeys::default()
        };
        let late = left.offset(Duration::from_secs(2), false);
        assert_eq!(late, left.offset(KEY_PAN_MAX_STEP, false));
    }

    #[test]
    fn diagonals_keep_the_speed_and_opposites_cancel() {
        let diag = PanKeys {
            up: true,
            right: true,
            ..PanKeys::default()
        };
        let (x, y) = diag.offset(TICK, false);
        assert!(x < 0.0 && y > 0.0);
        assert!(((x * x + y * y).sqrt() - KEY_PAN_SPEED * 0.016).abs() < 1e-9);
        let both = PanKeys {
            left: true,
            right: true,
            ..PanKeys::default()
        };
        assert_eq!(both.offset(TICK, false), (0.0, 0.0));
    }

    #[test]
    fn whole_pixels_move_and_the_rest_carries_over() {
        let mut nav = Navigation {
            keys: PanKeys {
                down: true,
                ..PanKeys::default()
            },
            ..Navigation::default()
        };
        let mut view = View::default();
        let t0 = Instant::now();
        nav.key_pan(&mut view, t0, false);
        assert_eq!(view.origin, (0.0, 0.0), "the first tick starts the clock");
        // 1 ms at 900 px/s is 0.9 px: nothing yet, then 1.8 px moves 1.
        nav.key_pan(&mut view, t0 + Duration::from_millis(1), false);
        assert_eq!(view.origin, (0.0, 0.0));
        nav.key_pan(&mut view, t0 + Duration::from_millis(2), false);
        assert_eq!(view.origin, (0.0, -1.0));
        nav.keys = PanKeys::default();
        nav.key_pan(&mut view, t0 + Duration::from_millis(3), false);
        assert_eq!(view.origin, (0.0, -1.0));
    }
}
