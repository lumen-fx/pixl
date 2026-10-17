//! Pointer and keyboard input over the stage: zoom, pan, and the tools.

use std::time::{Duration, Instant};

use lumenui::ecs::system::SystemParam;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{
    ModifiersState, MouseWheel, PointerButton, PointerData, PointerMoved, PointerPressed,
    PointerReleased,
};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;

use crate::editor::Editor;
use crate::home::Home;
use crate::keymap::KeyCommands;
use crate::settings::{Settings, WheelMode};
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

    /// The pan keys the keymap says are held.
    pub fn held(keys: &KeyCommands) -> Self {
        Self {
            up: keys.holds("pan-up"),
            left: keys.holds("pan-left"),
            down: keys.holds("pan-down"),
            right: keys.holds("pan-right"),
        }
    }

    /// How far the view moves, in screen pixels, while these keys are held
    /// for `dt` at the default speeds. The camera moves the way the key
    /// points, so the document moves the other way: W shows more of what is
    /// above.
    pub fn offset(self, dt: Duration, fast: bool) -> (f64, f64) {
        self.offset_at(dt, fast, KEY_PAN_SPEED, KEY_PAN_FAST)
    }

    /// [`Self::offset`] at `speed` screen pixels per second, `faster` times
    /// that with Shift held.
    pub fn offset_at(self, dt: Duration, fast: bool, speed: f64, faster: f64) -> (f64, f64) {
        let axis = |neg: bool, pos: bool| f64::from(i8::from(neg) - i8::from(pos));
        let (x, y) = (axis(self.left, self.right), axis(self.up, self.down));
        if x == 0.0 && y == 0.0 {
            return (0.0, 0.0);
        }
        // Diagonals move at the same speed as the axes.
        let len = (x * x + y * y).sqrt();
        let speed = speed * if fast { faster } else { 1.0 };
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
    /// The part of a zoom step the wheel has turned and the view not yet.
    wheel_carry: f64,
}

impl Navigation {
    /// Moves `view` for the pan keys held since the last call, at `now`,
    /// at the default speeds.
    pub fn key_pan(&mut self, view: &mut View, now: Instant, fast: bool) {
        self.key_pan_at(view, now, fast, KEY_PAN_SPEED, KEY_PAN_FAST);
    }

    /// [`Self::key_pan`] at the speeds the navigation settings give.
    pub fn key_pan_at(
        &mut self,
        view: &mut View,
        now: Instant,
        fast: bool,
        speed: f64,
        faster: f64,
    ) {
        if !self.keys.any() {
            self.keys_moved = None;
            self.keys_carry = (0.0, 0.0);
            return;
        }
        let Some(last) = self.keys_moved.replace(now) else {
            return;
        };
        let (dx, dy) =
            self.keys
                .offset_at(now.saturating_duration_since(last), fast, speed, faster);
        let (cx, cy) = (self.keys_carry.0 + dx, self.keys_carry.1 + dy);
        let (wx, wy) = (cx.trunc(), cy.trunc());
        self.keys_carry = (cx - wx, cy - wy);
        if wx != 0.0 || wy != 0.0 {
            view.pan(wx, wy);
        }
    }
}

/// Zooms with the wheel and the zoom keys, pans with a middle-button drag,
/// a drag while the pan key (the space bar) is held, the wheel when the
/// navigation settings make it pan, or the pan keys (W, A, S and D) held
/// down. Returns, through the [`Navigation`] resource, whether a pan holds
/// the pointer so the tools leave it alone.
#[allow(clippy::too_many_arguments)]
pub fn navigate(
    mut view: ResMut<View>,
    mut nav: ResMut<Navigation>,
    editor: Res<Editor>,
    settings: Res<Settings>,
    mut wheel: MessageReader<MouseWheel>,
    keys: Res<KeyCommands>,
    mut pointer: PointerInput,
    modifiers: Res<ModifiersState>,
    home: Res<Home>,
    wake: Option<Res<WakeDeadline>>,
    mut status: Signals,
) {
    let s = &settings.navigation;
    let held = modifiers.0;
    for w in wheel.read() {
        if home.open || !view.contains(w.position) {
            continue;
        }
        let flip = if s.invert_wheel { -1.0 } else { 1.0 };
        let (dx, dy) = (f64::from(w.delta.x) * flip, f64::from(w.delta.y) * flip);
        let zooms = match s.wheel {
            WheelMode::Zoom => dy != 0.0,
            WheelMode::Pan => held.ctrl || held.super_,
        };
        if zooms {
            let anchor = if s.zoom_to_cursor {
                view.stage_point(w.position)
            } else {
                view.middle()
            };
            nav.wheel_zoom(&mut view, dy, s.wheel_zoom_speed, anchor);
        }
        // A scroll the wheel does not zoom with pans; with Shift a vertical
        // wheel pans sideways.
        let (px, py) = match (zooms, held.shift) {
            (true, _) => (if s.wheel == WheelMode::Zoom { dx } else { 0.0 }, 0.0),
            (false, true) => (dx + dy, 0.0),
            (false, false) => (dx, dy),
        };
        if px != 0.0 || py != 0.0 {
            view.pan(-px * s.pan_speed, -py * s.pan_speed);
        }
    }
    if home.open {
        // The canvas is not there: what was meant for it is dropped.
        pointer.events();
        nav.keys = PanKeys::default();
        nav.panning = None;
        nav.space_held = false;
        return;
    }
    nav.space_held = keys.holds("pan-drag");
    nav.keys = PanKeys::held(&keys);
    for (id, _) in &keys.pressed {
        let anchor = nav
            .hover
            .filter(|h| view.contains(*h) && s.zoom_to_cursor)
            .map_or_else(|| view.middle(), |h| view.stage_point(h));
        match id.as_str() {
            "zoom-in" => {
                let z = view.step(true);
                view.zoom_at(z, anchor);
            }
            "zoom-out" => {
                let z = view.step(false);
                view.zoom_at(z, anchor);
            }
            "zoom-fit" => view.fit(editor.doc.width(), editor.doc.height()),
            "zoom-100" => view.zoom_at(1.0, anchor),
            _ => {}
        }
    }
    nav.key_pan_at(
        &mut view,
        Instant::now(),
        held.shift,
        s.key_pan_speed,
        s.key_pan_fast,
    );
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

/// The wheel distance of one notch, in logical pixels: the window turns a
/// line of wheel travel into this many.
pub const WHEEL_NOTCH: f64 = 32.0;

impl Navigation {
    /// Zooms `view` by a wheel scroll of `dy` pixels: one zoom step per
    /// notch at a `speed` of 1, smaller scrolls (a trackpad's) adding up
    /// until they make a step.
    pub fn wheel_zoom(&mut self, view: &mut View, dy: f64, speed: f64, anchor: (f64, f64)) {
        // Turning back starts fresh, so the view answers at once.
        if self.wheel_carry * dy < 0.0 {
            self.wheel_carry = 0.0;
        }
        self.wheel_carry += dy * speed / WHEEL_NOTCH;
        while self.wheel_carry.abs() >= 0.999 {
            let inward = self.wheel_carry < 0.0;
            let z = view.step(inward);
            view.zoom_at(z, anchor);
            self.wheel_carry -= self.wheel_carry.signum();
        }
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
