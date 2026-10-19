//! The color panel: the foreground and background swatches (a click picks
//! which one the panel edits), a saturation and value square with a hue
//! strip or a hue wheel around the square, an alpha strip, sliders in RGB,
//! HSV or HSL, and a hex field.
//!
//! The panel keeps the edited color as hue, saturation and value apart from
//! its 8-bit form, so dragging through gray or black does not lose the hue.
//! A drag on the picker, the strips or a slider previews in the panel and
//! sets the editor's color once, when the pointer comes up; a typed value
//! or an arrow key sets it at once.
//!
//! The picker's gradients are pixel buffers at the screen's resolution,
//! written again only when the hue or the size changes; the marks over
//! them are drawn each time the color moves.

use lumenui::ecs::query::Added;
use lumenui::glam::Vec2;
use lumenui::lumen_core::components::Transform;
use lumenui::lumen_core::input::{
    EditCommitted, Key, ModifiersState, NamedKey, PointerButton, PointerMoved, PointerPressed,
    PointerReleased, PointerState,
};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::lumen_core::window_state;
use lumenui::prelude::*;
use lumenui_canvas::{Canvas, Context2d, PixelBuffer};
use pixl_document::{Hsl, Hsv, Rgba};

use crate::editor::Editor;
use crate::ids::ElementIds;
use crate::settings::{Channels, Picker, Settings};
use crate::tools::{hex, set_if_changed};
use crate::view::{ViewColors, canvas_color, parse_hex};

/// The canvases the panel draws.
const FIELD: &str = "color-field";
const HUE: &str = "color-hue";
const ALPHA: &str = "color-alpha";

/// The ring's width in the wheel, as a share of its radius.
const RING: f64 = 0.2;

/// Which of the two colors the panel edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    Foreground,
    Background,
}

/// A part of the picker a press lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The saturation and value square, alone or inside the wheel.
    Square,
    /// The wheel's hue ring.
    Ring,
    /// The hue strip beside the square.
    Hue,
    Alpha,
    /// One of the channel sliders.
    Slider,
}

/// The panel's edited color and what its controls last showed.
#[derive(Resource, Default)]
pub struct ColorPanel {
    pub target: Target,
    /// The edited color: hue, saturation and value kept apart from the
    /// 8-bit color, and alpha.
    hsv: Hsv,
    alpha: u8,
    /// The editor's color the panel last took up, and whose it was.
    taken: Option<(Target, Rgba)>,
    /// A drag in progress: the part it started on and the color before it.
    drag: Option<(Part, Rgba)>,
    /// The slider positions as last shown, 0 to 1.
    sliders: Option<[f64; 4]>,
    /// The channel texts and the hex as last shown.
    texts: Option<([String; 4], String)>,
    shown_modes: Option<(Picker, Channels)>,
    swatches: Option<(Rgba, Rgba, Target)>,
    field: Drawing,
    hue: Drawing,
    alpha_strip: Drawing,
}

/// One canvas: its gradient buffer, what the buffer was written for, and
/// what the canvas was last drawn with.
#[derive(Default)]
struct Drawing {
    buffer: Option<(PixelBuffer, Vec<u32>)>,
    drawn: Option<Vec<u32>>,
    size: Option<Vec2>,
}

impl Drawing {
    /// The buffer written for `key`, writing it with `fill` when the key
    /// changed. `size` is in screen pixels.
    fn buffer(
        &mut self,
        key: Vec<u32>,
        size: (u32, u32),
        fill: impl FnOnce() -> Vec<u8>,
    ) -> Option<&PixelBuffer> {
        if self.buffer.as_ref().map(|(_, k)| k) != Some(&key) {
            self.buffer = None;
            let buf = PixelBuffer::new(size.0, size.1).ok()?;
            let _ = buf.write_rgba8(0, 0, size.0, size.1, &fill());
            self.buffer = Some((buf, key));
        }
        self.buffer.as_ref().map(|(b, _)| b)
    }

    /// The canvas's context, sized to `size` when that changed, and clear.
    fn context(&mut self, id: &str, size: Vec2) -> Context2d {
        let ctx = Context2d::new(id);
        if self.size != Some(size) {
            ctx.resize(size.x, size.y);
            self.size = Some(size);
        }
        ctx.clear();
        ctx
    }
}

impl ColorPanel {
    /// The edited color in 8 bits.
    pub fn color(&self) -> Rgba {
        Rgba::from_hsv(self.hsv, self.alpha)
    }

    pub fn hsv(&self) -> Hsv {
        self.hsv
    }

    /// Takes up `c`, keeping the hue when `c` is a gray and the saturation
    /// when it is black, as a picker moved there would.
    fn take(&mut self, c: Rgba) {
        if self.color() == c {
            return;
        }
        let mut next = c.to_hsv();
        if next.s <= 0.0 {
            next.h = self.hsv.h;
            if next.v <= 0.0 {
                next.s = self.hsv.s;
            }
        } else if next.v <= 0.0 {
            next.h = self.hsv.h;
            next.s = self.hsv.s;
        }
        self.hsv = next;
        self.alpha = c.a;
    }

    fn set_hsv(&mut self, hsv: Hsv) {
        self.hsv = Hsv {
            h: hsv.h.rem_euclid(360.0),
            s: hsv.s.clamp(0.0, 1.0),
            v: hsv.v.clamp(0.0, 1.0),
        };
    }

    /// The four channels in `mode` as numbers in their own units (0 to
    /// 255 for RGB and alpha, degrees for hue, percent otherwise).
    pub fn channels(&self, mode: Channels) -> [f64; 4] {
        let a = f64::from(self.alpha);
        match mode {
            Channels::Rgb => {
                let c = self.color();
                [f64::from(c.r), f64::from(c.g), f64::from(c.b), a]
            }
            Channels::Hsv => [
                f64::from(self.hsv.h),
                f64::from(self.hsv.s) * 100.0,
                f64::from(self.hsv.v) * 100.0,
                a,
            ],
            Channels::Hsl => {
                let l = self.hsv.to_hsl();
                [
                    f64::from(l.h),
                    f64::from(l.s) * 100.0,
                    f64::from(l.l) * 100.0,
                    a,
                ]
            }
        }
    }

    /// Sets channel `k` of `mode` to `value`, in the channel's units.
    pub fn set_channel(&mut self, mode: Channels, k: usize, value: f64) {
        let mut v = self.channels(mode);
        let Some(slot) = v.get_mut(k) else {
            return;
        };
        let (_, most) = range(mode, k);
        *slot = if most == 360.0 {
            value.rem_euclid(360.0)
        } else {
            value.clamp(0.0, most)
        };
        if k == 3 {
            self.alpha = v[3].round() as u8;
            return;
        }
        match mode {
            Channels::Rgb => {
                let byte = |x: f64| x.round() as u8;
                let c = Rgba::new(byte(v[0]), byte(v[1]), byte(v[2]), self.alpha);
                self.take(c);
            }
            Channels::Hsv => self.set_hsv(Hsv {
                h: v[0] as f32,
                s: (v[1] / 100.0) as f32,
                v: (v[2] / 100.0) as f32,
            }),
            Channels::Hsl => self.set_hsv(Hsv::from_hsl(Hsl {
                h: v[0] as f32,
                s: (v[1] / 100.0) as f32,
                l: (v[2] / 100.0) as f32,
            })),
        }
    }

    /// Moves the part `part` of the picker to `local`, a point in the
    /// canvas of `size`. The square in the wheel keeps to its bounds.
    pub fn pick(&mut self, picker: Picker, part: Part, local: Vec2, size: Vec2) {
        let (x, y) = (f64::from(local.x), f64::from(local.y));
        let (w, h) = (f64::from(size.x).max(1.0), f64::from(size.y).max(1.0));
        let mut hsv = self.hsv;
        match part {
            Part::Square => {
                let (sx, sy, side_w, side_h) = square_rect(picker, w, h);
                hsv.s = ((x - sx) / side_w).clamp(0.0, 1.0) as f32;
                hsv.v = (1.0 - (y - sy) / side_h).clamp(0.0, 1.0) as f32;
            }
            Part::Ring => {
                let (dx, dy) = (x - w / 2.0, y - h / 2.0);
                if dx != 0.0 || dy != 0.0 {
                    hsv.h = ring_hue(dx, dy) as f32;
                }
            }
            Part::Hue => hsv.h = ((y / h).clamp(0.0, 1.0) * 360.0) as f32,
            Part::Alpha => {
                self.alpha = ((x / w).clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            Part::Slider => {}
        }
        self.set_hsv(hsv);
    }

    /// Steps the part under the keys by `steps`: the square's saturation
    /// across and value up and down, the hue a degree a step, the alpha
    /// one level a step.
    pub fn nudge(&mut self, part: Part, across: i32, up: i32) {
        let mut hsv = self.hsv;
        match part {
            Part::Square => {
                hsv.s += across as f32 / 100.0;
                hsv.v += up as f32 / 100.0;
            }
            Part::Ring | Part::Hue => hsv.h += (across - up) as f32,
            Part::Alpha => {
                let a = i32::from(self.alpha) + across + up;
                self.alpha = a.clamp(0, 255) as u8;
            }
            Part::Slider => {}
        }
        self.set_hsv(hsv);
    }
}

/// The lowest and highest value of channel `k` in `mode`.
pub fn range(mode: Channels, k: usize) -> (f64, f64) {
    match (mode, k) {
        (_, 3) | (Channels::Rgb, _) => (0.0, 255.0),
        (_, 0) => (0.0, 360.0),
        _ => (0.0, 100.0),
    }
}

/// The names of the first three channels in `mode`.
pub fn channel_names(mode: Channels) -> [&'static str; 3] {
    match mode {
        Channels::Rgb => ["R", "G", "B"],
        Channels::Hsv => ["H", "S", "V"],
        Channels::Hsl => ["H", "S", "L"],
    }
}

/// A channel value as its field shows it: whole units.
fn channel_text(mode: Channels, k: usize, v: f64) -> String {
    let n = v.round();
    // A hue a hair short of the full turn shows as the turn's start.
    let n = if range(mode, k).1 == 360.0 && n >= 360.0 {
        0.0
    } else {
        n
    };
    format!("{n}")
}

/// The hue at `(dx, dy)` from the wheel's middle: red to the right, turning
/// counterclockwise through yellow and green.
fn ring_hue(dx: f64, dy: f64) -> f64 {
    (-dy).atan2(dx).to_degrees().rem_euclid(360.0)
}

/// The wheel in a canvas of `w` by `h`: its middle, outer and inner radius.
fn wheel(w: f64, h: f64) -> (f64, f64, f64, f64) {
    let outer = (w.min(h) / 2.0 - 1.0).max(1.0);
    (w / 2.0, h / 2.0, outer, outer * (1.0 - RING))
}

/// Where the saturation and value square sits in a canvas of `w` by `h`:
/// all of it in the square picker, inside the ring in the wheel.
fn square_rect(picker: Picker, w: f64, h: f64) -> (f64, f64, f64, f64) {
    match picker {
        Picker::Square => (0.0, 0.0, w, h),
        Picker::Wheel => {
            let (cx, cy, _, inner) = wheel(w, h);
            // Inscribed in the inner circle, a pixel clear of the ring.
            let side = ((inner - 1.0) * std::f64::consts::SQRT_2).max(1.0);
            (cx - side / 2.0, cy - side / 2.0, side, side)
        }
    }
}

/// The part of the field canvas `local` is on.
fn field_part(picker: Picker, local: Vec2, size: Vec2) -> Option<Part> {
    let (x, y) = (f64::from(local.x), f64::from(local.y));
    let (w, h) = (f64::from(size.x), f64::from(size.y));
    match picker {
        Picker::Square => Some(Part::Square),
        Picker::Wheel => {
            let (cx, cy, outer, inner) = wheel(w, h);
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            let (sx, sy, sw, sh) = square_rect(picker, w, h);
            if d >= inner && d <= outer + 2.0 {
                Some(Part::Ring)
            } else if x >= sx && x <= sx + sw && y >= sy && y <= sy + sh {
                Some(Part::Square)
            } else {
                None
            }
        }
    }
}

/// The pixels of the field: the square for `hue` alone, or the hue ring
/// around it, `pw` by `ph` screen pixels for a canvas `scale` times smaller.
pub fn field_pixels(picker: Picker, hue: f32, pw: u32, ph: u32, scale: f64) -> Vec<u8> {
    let mut out = vec![0u8; (pw * ph * 4) as usize];
    let (w, h) = (f64::from(pw) / scale, f64::from(ph) / scale);
    let (sx, sy, sw, sh) = square_rect(picker, w, h);
    let (cx, cy, outer, inner) = wheel(w, h);
    for py in 0..ph {
        for px in 0..pw {
            // The middle of the screen pixel, in canvas units.
            let x = (f64::from(px) + 0.5) / scale;
            let y = (f64::from(py) + 0.5) / scale;
            let i = ((py * pw + px) * 4) as usize;
            let in_square = x >= sx && x < sx + sw && y >= sy && y < sy + sh;
            let c = if in_square {
                let s = ((x - sx) / sw) as f32;
                let v = (1.0 - (y - sy) / sh) as f32;
                Rgba::from_hsv(Hsv { h: hue, s, v }, 255)
            } else if picker == Picker::Wheel {
                let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                // Coverage of the ring's two edges, a screen pixel wide.
                let edge = 0.5 / scale;
                let cover = ((d - inner + edge) * scale).clamp(0.0, 1.0)
                    * ((outer - d + edge) * scale).clamp(0.0, 1.0);
                if cover <= 0.0 {
                    continue;
                }
                let hue = ring_hue(x - cx, y - cy) as f32;
                let mut c = Rgba::from_hsv(
                    Hsv {
                        h: hue,
                        s: 1.0,
                        v: 1.0,
                    },
                    255,
                );
                c.a = (cover * 255.0).round() as u8;
                c
            } else {
                continue;
            };
            out[i..i + 4].copy_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    }
    out
}

/// The hue strip's pixels: red at the top, the full turn to the bottom.
pub fn hue_pixels(pw: u32, ph: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((pw * ph * 4) as usize);
    for py in 0..ph {
        let h = (f64::from(py) + 0.5) / f64::from(ph) * 360.0;
        let c = Rgba::from_hsv(
            Hsv {
                h: h as f32,
                s: 1.0,
                v: 1.0,
            },
            255,
        );
        for _ in 0..pw {
            out.extend_from_slice(&[c.r, c.g, c.b, 255]);
        }
    }
    out
}

/// The alpha strip's pixels: `c` from clear on the left to opaque on the
/// right over a checkerboard of `checker` colors, cells `cell` screen
/// pixels wide.
pub fn alpha_pixels(c: Rgba, checker: (Rgba, Rgba), pw: u32, ph: u32, cell: u32) -> Vec<u8> {
    let cell = cell.max(1);
    let mut out = Vec::with_capacity((pw * ph * 4) as usize);
    for py in 0..ph {
        for px in 0..pw {
            let back = if (px / cell + py / cell).is_multiple_of(2) {
                checker.0
            } else {
                checker.1
            };
            let a = (f64::from(px) + 0.5) / f64::from(pw);
            let mix = |f: u8, b: u8| (f64::from(f) * a + f64::from(b) * (1.0 - a)).round() as u8;
            out.extend_from_slice(&[mix(c.r, back.r), mix(c.g, back.g), mix(c.b, back.b), 255]);
        }
    }
    out
}

fn picker_name(p: Picker) -> &'static str {
    match p {
        Picker::Square => "Square",
        Picker::Wheel => "Wheel",
    }
}

fn channels_name(c: Channels) -> &'static str {
    match c {
        Channels::Rgb => "RGB",
        Channels::Hsv => "HSV",
        Channels::Hsl => "HSL",
    }
}

fn picker_from(name: &str) -> Option<Picker> {
    [Picker::Square, Picker::Wheel]
        .into_iter()
        .find(|p| picker_name(*p) == name)
}

fn channels_from(name: &str) -> Option<Channels> {
    [Channels::Rgb, Channels::Hsv, Channels::Hsl]
        .into_iter()
        .find(|c| channels_name(*c) == name)
}

/// Where an element is laid out: its top left and size, in window units.
fn rect_of(id: &str) -> Option<(Vec2, Vec2)> {
    let r = dom::get_by_id(id)?.rect()?;
    Some((
        Vec2::new(r.client_x, r.client_y),
        Vec2::new(r.width, r.height),
    ))
}

/// What the panel reads from the input of a tick.
#[derive(lumenui::ecs::system::SystemParam)]
pub struct ColorInput<'w, 's> {
    clicks: MessageReader<'w, 's, ClickEvent>,
    pressed: MessageReader<'w, 's, PointerPressed>,
    moved: MessageReader<'w, 's, PointerMoved>,
    released: MessageReader<'w, 's, PointerReleased>,
    keys: MessageReader<'w, 's, KeyPressed>,
    committed: Query<'w, 's, &'static LumenId, Added<EditCommitted>>,
    hovered: Query<'w, 's, &'static LumenId, With<Hovered>>,
    typing: Query<'w, 's, &'static LumenId, (With<Focused>, With<TextInput>)>,
    modifiers: Res<'w, ModifiersState>,
    pointer: Res<'w, PointerState>,
    ids: ElementIds<'w, 's>,
}

fn color_of(editor: &Editor, t: Target) -> Rgba {
    match t {
        Target::Foreground => editor.fg,
        Target::Background => editor.bg,
    }
}

fn set_color(editor: &mut Editor, t: Target, c: Rgba) {
    match t {
        Target::Foreground => editor.fg = c,
        Target::Background => editor.bg = c,
    }
}

/// The color panel's input: which color it edits, the modes, drags on the
/// picker and the strips, the sliders, typed values and arrow keys.
pub fn color_input(
    mut editor: ResMut<Editor>,
    mut panel: ResMut<ColorPanel>,
    mut settings: ResMut<Settings>,
    signals: Signals,
    mut input: ColorInput,
) {
    let editor = &mut *editor;
    let panel = &mut *panel;
    // The modes: a dropdown showing other than the panel showed is the
    // user's choice, which the settings keep.
    if let Some((shown_picker, shown_channels)) = panel.shown_modes {
        let picker = signals
            .get::<String>("color_picker")
            .and_then(|n| picker_from(&n))
            .unwrap_or(shown_picker);
        let channels = signals
            .get::<String>("color_channels")
            .and_then(|n| channels_from(&n))
            .unwrap_or(shown_channels);
        if picker != shown_picker {
            settings.color.picker = picker;
        }
        if channels != shown_channels {
            settings.color.channels = channels;
        }
    }
    let (picker, channels) = (settings.color.picker, settings.color.channels);

    for click in input.clicks.read() {
        match input.ids.of(click.entity).map(|id| id.0.as_str()) {
            Ok("color-fg") => panel.target = Target::Foreground,
            Ok("color-bg") => panel.target = Target::Background,
            Ok("color-swap") => std::mem::swap(&mut editor.fg, &mut editor.bg),
            _ => {}
        }
    }

    // The editor's color, unless a drag is previewing over it.
    let current = color_of(editor, panel.target);
    if panel.drag.is_none() && panel.taken != Some((panel.target, current)) {
        panel.take(current);
        panel.taken = Some((panel.target, current));
    }
    let before = panel.color();
    let mut commit = false;

    // A press on the picker or a strip starts a drag there.
    let hovered = |id: &str| input.hovered.iter().any(|h| h.0 == id);
    for press in input.pressed.read() {
        if press.button != PointerButton::Primary {
            continue;
        }
        let hit = [FIELD, HUE, ALPHA].into_iter().find_map(|id| {
            let (at, size) = rect_of(id)?;
            let local = press.position - at;
            let inside = local.x >= 0.0 && local.y >= 0.0 && local.x < size.x && local.y < size.y;
            if !inside || !hovered(id) {
                return None;
            }
            let part = match id {
                FIELD => field_part(picker, local, size)?,
                HUE => Part::Hue,
                _ => Part::Alpha,
            };
            Some((id, part, local, size))
        });
        if let Some((_, part, local, size)) = hit {
            panel.drag = Some((part, color_of(editor, panel.target)));
            panel.pick(picker, part, local, size);
        }
    }
    let last_move = input.moved.read().last().map(|m| m.position);
    if let (Some((part, _)), Some(at)) = (panel.drag, last_move)
        && part != Part::Slider
    {
        let id = match part {
            Part::Square | Part::Ring => FIELD,
            Part::Hue => HUE,
            _ => ALPHA,
        };
        if let Some((origin, size)) = rect_of(id) {
            panel.pick(picker, part, at - origin, size);
        }
    }

    // The sliders: a position other than the one shown is the user's.
    if let Some(shown) = panel.sliders {
        for (k, was) in shown.iter().enumerate() {
            let name = format!("color_c{k}");
            let Some(v) = signals.get::<f64>(&name) else {
                continue;
            };
            if (v - was).abs() > 1e-6 {
                let (lo, hi) = range(channels, k);
                let value = lo + v.clamp(0.0, 1.0) * (hi - lo);
                // The hue slider's right end is the last hue before the
                // turn, not the turn's start, so the knob stays where it
                // was let go.
                let value = if hi == 360.0 { value.min(359.9) } else { value };
                panel.set_channel(channels, k, value);
                if panel.drag.is_none() {
                    panel.drag = Some((Part::Slider, color_of(editor, panel.target)));
                }
            }
        }
    }

    // Typed values land as the field is committed.
    for id in input.committed.iter() {
        let id = id.0.as_str();
        if id == "color-hex" {
            if let Some(text) = signals.get::<String>("color_hex") {
                let text = if text.starts_with('#') {
                    text
                } else {
                    format!("#{text}")
                };
                match parse_hex(&text) {
                    Some(c) => {
                        panel.take(c);
                        commit = true;
                    }
                    // A hex that does not read shows the color again.
                    None => panel.texts = None,
                }
            }
        } else if let Some(k) = id
            .strip_prefix("color-c")
            .and_then(|r| r.strip_suffix("-field"))
            .and_then(|n| n.parse::<usize>().ok())
        {
            let typed = signals
                .get::<String>(&format!("color_c{k}_text"))
                .and_then(|t| t.trim().trim_end_matches('%').parse::<f64>().ok());
            match typed {
                Some(v) if v.is_finite() => {
                    panel.set_channel(channels, k, v);
                    commit = true;
                }
                _ => {}
            }
            // The field shows the value as the panel holds it.
            panel.texts = None;
        }
    }

    // Arrow keys: in a channel field they step its value; over the picker
    // or a strip they step what lies under the pointer.
    let shift = input.modifiers.0.shift;
    let focused: Option<usize> = input.typing.iter().find_map(|id| {
        id.0.strip_prefix("color-c")
            .and_then(|r| r.strip_suffix("-field"))
            .and_then(|n| n.parse().ok())
    });
    let typing_elsewhere = focused.is_none() && !input.typing.is_empty();
    for press in input.keys.read() {
        let (across, up) = match press.key {
            Key::Named(NamedKey::ArrowLeft) => (-1, 0),
            Key::Named(NamedKey::ArrowRight) => (1, 0),
            Key::Named(NamedKey::ArrowUp) => (0, 1),
            Key::Named(NamedKey::ArrowDown) => (0, -1),
            _ => continue,
        };
        if press.modifiers.alt || press.modifiers.ctrl || input.modifiers.0.alt {
            continue;
        }
        let step = if shift || press.modifiers.shift {
            10
        } else {
            1
        };
        if let Some(k) = focused {
            if up != 0 {
                let v = panel.channels(channels)[k] + f64::from(up * step);
                panel.set_channel(channels, k, v);
                panel.texts = None;
                commit = true;
            }
            continue;
        }
        if typing_elsewhere {
            continue;
        }
        let under = [FIELD, HUE, ALPHA].into_iter().find_map(|id| {
            if !hovered(id) {
                return None;
            }
            match id {
                FIELD => {
                    let (at, size) = rect_of(id)?;
                    let local = input.pointer.position? - at;
                    field_part(picker, local, size)
                }
                HUE => Some(Part::Hue),
                _ => Some(Part::Alpha),
            }
        });
        if let Some(part) = under {
            panel.nudge(part, across * step, up * step);
            commit = true;
        }
    }

    // The end of a drag sets the editor's color, once.
    if input.released.read().count() > 0 && panel.drag.take().is_some() {
        commit = true;
    }
    // A slider moved with no button down (from the keyboard) sets it now.
    if panel.drag.is_some_and(|(p, _)| p == Part::Slider) && !input.pointer.primary_down {
        panel.drag = None;
        commit = true;
    }
    if commit || (panel.drag.is_none() && panel.color() != before) {
        let c = panel.color();
        set_color(editor, panel.target, c);
        panel.taken = Some((panel.target, c));
    }
}

/// Shows the panel: the swatches, the modes, the channel rows and the hex,
/// and draws the picker and the strips.
pub fn show_color(
    editor: Res<Editor>,
    mut panel: ResMut<ColorPanel>,
    settings: Res<Settings>,
    colors: Res<ViewColors>,
    canvases: Query<(&Canvas, &Transform)>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let panel = &mut *panel;
    let mut changed = false;
    let (picker, channels) = (settings.color.picker, settings.color.channels);
    let edited = panel.color();
    let (fg, bg) = match panel.target {
        Target::Foreground => (edited, editor.bg),
        Target::Background => (editor.fg, edited),
    };
    // Before the first input tick the panel holds no color yet.
    let (fg, bg) = if panel.taken.is_none() {
        (editor.fg, editor.bg)
    } else {
        (fg, bg)
    };
    let swatches = (fg, bg, panel.target);
    if panel.swatches != Some(swatches)
        && let (Some(f), Some(b)) = (dom::get_by_id("color-fg"), dom::get_by_id("color-bg"))
    {
        f.set_style("bg", &hex(fg));
        b.set_style("bg", &hex(bg));
        crate::set_class(f, "editing", panel.target == Target::Foreground);
        crate::set_class(b, "editing", panel.target == Target::Background);
        panel.swatches = Some(swatches);
        changed = true;
    }
    if panel.taken.is_none() {
        return;
    }
    if panel.shown_modes != Some((picker, channels)) {
        signals.set("color_picker", picker_name(picker));
        signals.set("color_channels", channels_name(channels));
        for (k, name) in channel_names(channels).into_iter().enumerate() {
            signals.set(&format!("color_c{k}_name"), name);
        }
        if let Some(row) = dom::get_by_id("color-picker") {
            crate::set_class(row, "wheel", picker == Picker::Wheel);
        }
        panel.shown_modes = Some((picker, channels));
        panel.sliders = None;
        panel.texts = None;
        changed = true;
    }
    let values = panel.channels(channels);
    let sliders: [f64; 4] = std::array::from_fn(|k| {
        let (lo, hi) = range(channels, k);
        ((values[k] - lo) / (hi - lo)).clamp(0.0, 1.0)
    });
    let slid = panel
        .sliders
        .is_none_or(|s| s.iter().zip(&sliders).any(|(a, b)| (a - b).abs() > 1e-6));
    if slid {
        for (k, v) in sliders.iter().enumerate() {
            signals.set(&format!("color_c{k}"), *v);
        }
        panel.sliders = Some(sliders);
        changed = true;
    }
    let texts: [String; 4] = std::array::from_fn(|k| channel_text(channels, k, values[k]));
    let now = (texts, hex(edited));
    if panel.texts.as_ref() != Some(&now) {
        for (k, t) in now.0.iter().enumerate() {
            signals.set(&format!("color_c{k}_text"), t.clone());
        }
        set_if_changed(&mut signals, "color_hex", now.1.clone());
        panel.texts = Some(now);
        changed = true;
    }

    let dpr = f64::from(window_state::dpr()).max(0.25);
    let size_of = |id: &str| {
        canvases
            .iter()
            .find(|(c, _)| c.id == id)
            .map(|(_, t)| t.size)
            .filter(|s| s.x >= 1.0 && s.y >= 1.0)
    };
    let px = |s: Vec2| {
        (
            (f64::from(s.x) * dpr).round().max(1.0) as u32,
            (f64::from(s.y) * dpr).round().max(1.0) as u32,
        )
    };
    let (Some(ring_a), Some(ring_b)) = (colors.marquee_a, colors.marquee_b) else {
        if changed {
            crate::repaint_soon(&wake);
        }
        return;
    };
    let hsv = panel.hsv;
    let marks = [ring_a, ring_b].map(Rgba::to_u32);

    if let Some(size) = size_of(FIELD) {
        let (pw, ph) = px(size);
        let picker_key = picker as u32;
        let key = vec![picker_key, hsv.h.to_bits(), pw, ph];
        let drawn = [
            key.clone(),
            vec![
                hsv.s.to_bits(),
                hsv.v.to_bits(),
                size.x.to_bits(),
                size.y.to_bits(),
            ],
            marks.to_vec(),
        ]
        .concat();
        if panel.field.drawn.as_ref() != Some(&drawn) {
            let ctx = panel.field.context(FIELD, size);
            let (w, h) = (f64::from(size.x), f64::from(size.y));
            if let Some(buf) = panel
                .field
                .buffer(key, (pw, ph), || field_pixels(picker, hsv.h, pw, ph, dpr))
            {
                ctx.set_image_smoothing(false);
                ctx.draw_buffer_scaled(buf, 0.0, 0.0, w, h);
            }
            let (sx, sy, sw, sh) = square_rect(picker, w, h);
            let at = (
                sx + f64::from(hsv.s) * sw,
                sy + (1.0 - f64::from(hsv.v)) * sh,
            );
            mark_circle(&ctx, at, (ring_a, ring_b));
            if picker == Picker::Wheel {
                let (cx, cy, outer, inner) = wheel(w, h);
                let r = (outer + inner) / 2.0;
                let t = f64::from(hsv.h).to_radians();
                mark_circle(&ctx, (cx + r * t.cos(), cy - r * t.sin()), (ring_a, ring_b));
            }
            panel.field.drawn = Some(drawn);
            changed = true;
        }
    }
    if picker == Picker::Square
        && let Some(size) = size_of(HUE)
    {
        let (pw, ph) = px(size);
        let key = vec![pw, ph];
        let drawn = [
            key.clone(),
            vec![hsv.h.to_bits(), size.x.to_bits(), size.y.to_bits()],
            marks.to_vec(),
        ]
        .concat();
        if panel.hue.drawn.as_ref() != Some(&drawn) {
            let ctx = panel.hue.context(HUE, size);
            let (w, h) = (f64::from(size.x), f64::from(size.y));
            if let Some(buf) = panel.hue.buffer(key, (pw, ph), || hue_pixels(pw, ph)) {
                ctx.set_image_smoothing(false);
                ctx.draw_buffer_scaled(buf, 0.0, 0.0, w, h);
            }
            mark_bar(
                &ctx,
                f64::from(hsv.h) / 360.0 * h,
                w,
                (ring_a, ring_b),
                false,
            );
            panel.hue.drawn = Some(drawn);
            changed = true;
        }
    }
    if let (Some(size), Some(ca), Some(cb)) = (size_of(ALPHA), colors.checker_a, colors.checker_b) {
        let (pw, ph) = px(size);
        let opaque = Rgba { a: 255, ..edited };
        let key = vec![opaque.to_u32(), ca.to_u32(), cb.to_u32(), pw, ph];
        let drawn = [
            key.clone(),
            vec![u32::from(edited.a), size.x.to_bits(), size.y.to_bits()],
            marks.to_vec(),
        ]
        .concat();
        if panel.alpha_strip.drawn.as_ref() != Some(&drawn) {
            let ctx = panel.alpha_strip.context(ALPHA, size);
            let (w, h) = (f64::from(size.x), f64::from(size.y));
            let cell = ((f64::from(size.y) * dpr) / 2.0).round().max(1.0) as u32;
            if let Some(buf) = panel.alpha_strip.buffer(key, (pw, ph), || {
                alpha_pixels(opaque, (ca, cb), pw, ph, cell)
            }) {
                ctx.set_image_smoothing(false);
                ctx.draw_buffer_scaled(buf, 0.0, 0.0, w, h);
            }
            mark_bar(
                &ctx,
                f64::from(edited.a) / 255.0 * w,
                h,
                (ring_a, ring_b),
                true,
            );
            panel.alpha_strip.drawn = Some(drawn);
            changed = true;
        }
    }
    if changed {
        crate::repaint_soon(&wake);
    }
}

/// The picker's mark: a ring in the theme's two marquee colors, so it reads
/// on any color under it.
fn mark_circle(ctx: &Context2d, (x, y): (f64, f64), (a, b): (Rgba, Rgba)) {
    for (color, width) in [(b, 3.0), (a, 1.5)] {
        ctx.set_stroke_color(canvas_color(color));
        ctx.set_line_width(width);
        ctx.begin_path();
        ctx.arc(x, y, 4.0, 0.0, std::f64::consts::TAU);
        ctx.stroke();
    }
}

/// A strip's mark: a bar across the strip at `at`, along its length.
fn mark_bar(ctx: &Context2d, at: f64, across: f64, (a, b): (Rgba, Rgba), vertical: bool) {
    for (color, width) in [(b, 3.0), (a, 1.0)] {
        ctx.set_stroke_color(canvas_color(color));
        ctx.set_line_width(width);
        ctx.begin_path();
        if vertical {
            ctx.move_to(at, 0.0);
            ctx.line_to(at, across);
        } else {
            ctx.move_to(0.0, at);
            ctx.line_to(across, at);
        }
        ctx.stroke();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_with(c: Rgba) -> ColorPanel {
        let mut p = ColorPanel::default();
        p.take(c);
        p
    }

    #[test]
    fn a_gray_keeps_the_hue_it_was_reached_from() {
        let mut p = panel_with(Rgba::opaque(0, 128, 255));
        let hue = p.hsv().h;
        p.take(Rgba::opaque(90, 90, 90));
        assert_eq!(p.hsv().h, hue);
        assert_eq!(p.color(), Rgba::opaque(90, 90, 90));
        // Black keeps the saturation as well.
        p.take(Rgba::opaque(200, 40, 40));
        let s = p.hsv().s;
        p.take(Rgba::BLACK);
        assert_eq!((p.hsv().h, p.hsv().s), (0.0, s));
    }

    #[test]
    fn channels_read_back_in_every_mode() {
        let c = Rgba::new(12, 200, 99, 140);
        for mode in [Channels::Rgb, Channels::Hsv, Channels::Hsl] {
            let mut p = panel_with(c);
            let v = p.channels(mode);
            for (k, x) in v.iter().enumerate() {
                p.set_channel(mode, k, *x);
            }
            assert_eq!(p.color(), c, "{mode:?}");
        }
        let mut p = panel_with(c);
        p.set_channel(Channels::Hsv, 0, 365.0);
        assert!((p.hsv().h - 5.0).abs() < 1e-4, "hue turns around");
        p.set_channel(Channels::Rgb, 0, 300.0);
        assert_eq!(p.color().r, 255, "clamped");
        p.set_channel(Channels::Hsl, 2, 100.0);
        assert_eq!(p.color(), Rgba::new(255, 255, 255, 140));
    }

    #[test]
    fn the_square_and_the_strips_map_their_corners() {
        let size = Vec2::new(100.0, 50.0);
        let mut p = panel_with(Rgba::opaque(255, 0, 0));
        p.pick(Picker::Square, Part::Square, Vec2::new(0.0, 0.0), size);
        assert_eq!(p.color(), Rgba::WHITE);
        p.pick(Picker::Square, Part::Square, Vec2::new(100.0, 0.0), size);
        assert_eq!(p.color(), Rgba::opaque(255, 0, 0));
        p.pick(Picker::Square, Part::Square, Vec2::new(150.0, 80.0), size);
        assert_eq!(p.color(), Rgba::BLACK, "clamped to the corner");
        p.pick(Picker::Square, Part::Hue, Vec2::new(0.0, 50.0 / 3.0), size);
        assert!((p.hsv().h - 120.0).abs() < 1e-3);
        p.pick(Picker::Square, Part::Alpha, Vec2::new(50.0, 0.0), size);
        assert_eq!(p.color().a, 128);
    }

    #[test]
    fn the_wheel_reads_hue_on_the_ring_and_the_square_inside() {
        let size = Vec2::new(100.0, 100.0);
        let (_, _, outer, inner) = wheel(100.0, 100.0);
        let r = ((outer + inner) / 2.0) as f32;
        let at = |x: f32, y: f32| Vec2::new(50.0 + x, 50.0 + y);
        assert_eq!(
            field_part(Picker::Wheel, at(r, 0.0), size),
            Some(Part::Ring)
        );
        assert_eq!(
            field_part(Picker::Wheel, at(0.0, 0.0), size),
            Some(Part::Square)
        );
        assert_eq!(field_part(Picker::Wheel, Vec2::new(1.0, 1.0), size), None);
        let mut p = panel_with(Rgba::opaque(255, 0, 0));
        p.pick(Picker::Wheel, Part::Ring, at(0.0, -r), size);
        assert!((p.hsv().h - 90.0).abs() < 1e-3, "up is a quarter turn");
        p.pick(Picker::Wheel, Part::Ring, at(-r, 0.0), size);
        assert!((p.hsv().h - 180.0).abs() < 1e-3);
    }

    #[test]
    fn nudges_step_and_stay_in_range() {
        let mut p = panel_with(Rgba::opaque(255, 0, 0));
        p.nudge(Part::Square, -10, 0);
        assert!((p.hsv().s - 0.9).abs() < 1e-5);
        p.nudge(Part::Square, 0, 10);
        assert_eq!(p.hsv().v, 1.0, "value is already at its top");
        p.nudge(Part::Hue, 0, 1);
        assert!((p.hsv().h - 359.0).abs() < 1e-3, "up turns back");
        p.nudge(Part::Alpha, -300, 0);
        assert_eq!(p.color().a, 0);
    }

    #[test]
    fn pixels_cover_their_buffers() {
        assert_eq!(
            field_pixels(Picker::Square, 0.0, 8, 4, 2.0).len(),
            8 * 4 * 4
        );
        let wheel = field_pixels(Picker::Wheel, 0.0, 40, 40, 1.0);
        // The corners are clear and the middle is the square.
        assert_eq!(wheel[3], 0);
        let mid = (20 * 40 + 20) * 4;
        assert_eq!(wheel[mid + 3], 255);
        // Red at the top, half a degree in at one row a degree.
        let hue = hue_pixels(2, 360);
        assert_eq!((hue[0], hue[2], hue[3]), (255, 0, 255));
        assert!(hue[1] <= 3);
        let alpha = alpha_pixels(Rgba::WHITE, (Rgba::BLACK, Rgba::BLACK), 10, 1, 1);
        assert!(alpha[0] < 20 && alpha[36] > 235);
    }
}
