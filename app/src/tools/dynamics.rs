//! Brush dynamics in the editor: what pen pressure and the pointer's speed
//! do to each brush-like tool's strokes, kept per tool in the settings and
//! carried by brushes and tool presets, and the Dynamics popup of the tool
//! options that edits them. `pixl-paint` turns the inputs into each point's
//! brush and color; this module feeds it the pointer.

use std::collections::HashMap;
use std::time::Instant;

use lumenui::ecs::system::SystemParam;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{PointerButton, PointerData, PointerKind};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::Context2d;
use pixl_document::Rgba;
use pixl_paint::tools::dynamics::{Dab, DabBrushes, Dynamics, Reading, Response, Sensor, Velocity};
use pixl_paint::tools::gradient::DitherPattern;
use pixl_paint::{Brush, Ink};
use serde::{Deserialize, Serialize};

use crate::editor::{Editor, Tool};
use crate::home::Home;
use crate::ids::ElementIds;
use crate::settings::Settings;
use crate::settings::keymap::KeyCommands;
use crate::tools::paint_color;
use crate::tools::toolbox::Toolbox;
use crate::view::input::{Pointer, PointerInput};
use crate::view::parse_hex;

/// The tools with dynamics: the brushes that lay a brush along the stroke.
pub fn dynamic(tool: Tool) -> bool {
    use Tool::*;
    matches!(
        tool,
        Pencil
            | Eraser
            | Brush
            | Shade
            | DitherBrush
            | CloneStamp
            | PatternStamp
            | Smudge
            | Blur
            | ReplaceColor
    )
}

/// The tools whose color can follow an input: those that paint the
/// foreground color with a hard brush.
pub fn colors(tool: Tool) -> bool {
    tool == Tool::Pencil
}

/// The tools whose brush can turn: every dynamic tool but the soft brush,
/// whose dab is round.
pub fn turns(tool: Tool) -> bool {
    dynamic(tool) && tool != Tool::Brush
}

/// What an input does before it reaches the brush, as the settings keep
/// it: thresholds in percent and the curve's exponent.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct InputResponse {
    /// Below this the input reads as nothing.
    pub min: u32,
    /// From this on the input reads as full.
    pub max: u32,
    /// Below 1 a little input goes far; above 1 it takes a lot.
    pub curve: f64,
}

impl Default for InputResponse {
    fn default() -> Self {
        Self {
            min: 0,
            max: 100,
            curve: 1.0,
        }
    }
}

impl InputResponse {
    fn clamp(&mut self) {
        self.min = self.min.min(100);
        self.max = self.max.min(100);
        self.curve = if self.curve.is_finite() {
            self.curve.clamp(0.1, 10.0)
        } else {
            1.0
        };
    }

    fn response(&self) -> Response {
        Response {
            min: self.min as f32 / 100.0,
            max: self.max as f32 / 100.0,
            curve: self.curve as f32,
        }
    }
}

/// The input a part of the brush follows, as the settings keep it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Follow {
    #[default]
    Off,
    Pressure,
    Velocity,
}

impl Follow {
    const ALL: [Follow; 3] = [Follow::Off, Follow::Pressure, Follow::Velocity];

    pub fn label(self) -> &'static str {
        match self {
            Follow::Off => "Off",
            Follow::Pressure => "Pressure",
            Follow::Velocity => "Velocity",
        }
    }

    fn named(name: &str) -> Option<Follow> {
        Follow::ALL.into_iter().find(|f| f.label() == name)
    }
}

impl From<Follow> for Sensor {
    fn from(f: Follow) -> Sensor {
        match f {
            Follow::Off => Sensor::Off,
            Follow::Pressure => Sensor::Pressure,
            Follow::Velocity => Sensor::Velocity,
        }
    }
}

/// The ways the color between the two can be made, by their menu names:
/// a blend, or a dither of the two colors.
pub const DITHERS: [&str; 6] = [
    "Blend",
    "Bayer 2x2",
    "Bayer 4x4",
    "Bayer 8x8",
    "Blue noise",
    "Pattern",
];

/// One tool's dynamics as the settings, brushes and presets keep them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ToolDynamics {
    pub pressure: InputResponse,
    pub velocity: InputResponse,
    pub size: Follow,
    /// The size at no input and at full input, in percent of the tool's.
    pub min_size: u32,
    pub max_size: u32,
    pub angle: Follow,
    /// The brush angle at no input and at full input, in degrees.
    pub min_angle: i32,
    pub max_angle: i32,
    pub color: Follow,
    /// How the colors between the background and the foreground are made,
    /// one of [`DITHERS`].
    pub dither: String,
}

impl Default for ToolDynamics {
    fn default() -> Self {
        Self {
            pressure: InputResponse::default(),
            velocity: InputResponse::default(),
            size: Follow::Off,
            min_size: 0,
            max_size: 100,
            angle: Follow::Off,
            min_angle: 0,
            max_angle: 90,
            color: Follow::Off,
            dither: DITHERS[0].to_string(),
        }
    }
}

impl ToolDynamics {
    /// The dynamics a tool starts with: the pencil and the eraser size
    /// their brush by pen pressure, the others follow no input.
    pub fn for_tool(tool: Tool) -> Self {
        let size = if matches!(tool, Tool::Pencil | Tool::Eraser) {
            Follow::Pressure
        } else {
            Follow::Off
        };
        Self {
            size,
            ..Self::default()
        }
    }

    /// Pulls every value into the range the editor uses.
    pub fn clamp(&mut self) {
        self.pressure.clamp();
        self.velocity.clamp();
        self.min_size = self.min_size.min(400);
        self.max_size = self.max_size.min(400);
        self.min_angle = self.min_angle.clamp(-360, 360);
        self.max_angle = self.max_angle.clamp(-360, 360);
        if !DITHERS.contains(&self.dither.as_str()) {
            self.dither = DITHERS[0].to_string();
        }
    }

    /// The dynamics the pen settings of an older pixl gave the pencil and
    /// the eraser: pressure on or off, its curve, and the sizes at the
    /// lightest and the firmest touch.
    pub fn from_pen(pen: &serde_json::Value) -> Option<Self> {
        let pen = pen.as_object()?;
        let num = |k: &str| pen.get(k).and_then(serde_json::Value::as_f64);
        let on = pen
            .get("pressure")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let mut d = Self {
            size: if on { Follow::Pressure } else { Follow::Off },
            min_size: num("min_size").map_or(0, |v| v.max(0.0) as u32),
            max_size: num("max_size").map_or(100, |v| v.max(0.0) as u32),
            pressure: InputResponse {
                curve: num("curve").unwrap_or(1.0),
                ..InputResponse::default()
            },
            ..Self::default()
        };
        d.clamp();
        Some(d)
    }

    /// The dynamics `pixl-paint` paints with; `pattern` gives the dither a
    /// name stands for.
    pub fn to_paint(&self, pattern: impl FnOnce(&str) -> DitherPattern) -> Dynamics {
        Dynamics {
            pressure: self.pressure.response(),
            velocity: self.velocity.response(),
            size: self.size.into(),
            min_size: self.min_size as f32 / 100.0,
            max_size: self.max_size as f32 / 100.0,
            angle: self.angle.into(),
            min_angle: self.min_angle as f32,
            max_angle: self.max_angle as f32,
            color: self.color.into(),
            dither: (self.dither != DITHERS[0]).then(|| pattern(&self.dither)),
        }
    }

    /// What follows an input, for a brush's summary: "size by pressure".
    pub fn summary(&self) -> Option<String> {
        let parts: Vec<String> = [
            ("size", self.size),
            ("angle", self.angle),
            ("color", self.color),
        ]
        .into_iter()
        .filter(|(_, f)| *f != Follow::Off)
        .map(|(what, f)| format!("{what} by {}", f.label().to_lowercase()))
        .collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// A stroke's dynamics while it is drawn: the inputs measured so far and
/// the brushes made for it.
pub struct Live {
    dynamics: Dynamics,
    velocity: Velocity,
    brushes: DabBrushes,
    /// The tool's size, which the size dynamics scale.
    size: u32,
    started: Instant,
    /// The colors the color runs between: full input, then none.
    fg: Rgba,
    bg: Rgba,
    last: Option<Dab>,
}

impl Live {
    /// A stroke's dynamics, or `None` when nothing follows an input.
    pub fn start(dynamics: Dynamics, size: u32, fg: Rgba, bg: Rgba) -> Option<Self> {
        dynamics.is_dynamic().then(|| Self {
            dynamics,
            velocity: Velocity::new(),
            brushes: DabBrushes::new(),
            size: size.max(1),
            started: Instant::now(),
            fg,
            bg,
            last: None,
        })
    }

    /// Feeds a pointer event at `screen`, and returns what the stroke
    /// paints with from here when it changed. A mouse presses at full
    /// pressure.
    pub fn feed(&mut self, screen: Vec2, data: &PointerData) -> Option<Dab> {
        let t = self.started.elapsed().as_secs_f64();
        let velocity = self
            .velocity
            .feed(f64::from(screen.x), f64::from(screen.y), t);
        let pressure = if data.kind == PointerKind::Pen {
            data.pressure
        } else {
            1.0
        };
        let dab = self.dynamics.dab(self.size, Reading { pressure, velocity });
        (self.last != Some(dab)).then(|| {
            self.last = Some(dab);
            dab
        })
    }

    /// The brush of `dab`; `make` builds it at the dab's size.
    pub fn brush(&mut self, dab: &Dab, make: impl FnOnce(u32) -> Brush) -> Brush {
        self.brushes.get(dab, make)
    }

    /// The ink of `dab` when its color follows an input.
    pub fn ink(&self, dab: &Dab) -> Option<Ink> {
        dab.mix.map(|mix| self.dynamics.ink(self.fg, self.bg, mix))
    }
}

/// The dynamics a stroke of `tool` with `button` paints with, or `None`
/// when nothing follows an input. What the tool cannot do (turn a round
/// dab, color an eraser) is left out.
pub fn live_for(
    editor: &Editor,
    toolbox: &Toolbox,
    settings: &Settings,
    tool: Tool,
    button: PointerButton,
) -> Option<Live> {
    if !dynamic(tool) {
        return None;
    }
    let mut d = settings
        .dynamics(tool)
        .to_paint(|name| crate::tools::toolbox::dither_named(editor, toolbox, name));
    if !colors(tool) {
        d.color = Sensor::Off;
    }
    if !turns(tool) {
        d.angle = Sensor::Off;
    }
    let fg = paint_color(editor, button);
    let bg = if button == PointerButton::Secondary {
        editor.fg
    } else {
        editor.bg
    };
    // A custom brush keeps its own size, which the size dynamics scale.
    let size = editor.brush(editor.options.size).size;
    Live::start(d, size, fg, bg)
}

/// Feeds a stroke's pointer event to its dynamics; when the brush or the
/// color changes, `apply` gets the new brush and, when the color follows
/// an input, the new ink.
pub fn follow(
    editor: &Editor,
    live: &mut Live,
    (at, data): (Vec2, &PointerData),
    apply: impl FnOnce(Brush, Option<Ink>),
) {
    if let Some(dab) = live.feed(at, data) {
        let brush = live.brush(&dab, |s| editor.brush(s).resized(s));
        let ink = live.ink(&dab);
        apply(brush, ink);
    }
}

/// The popup's state: open or not, and what its controls show.
#[derive(Resource, Default)]
pub struct DynamicsPopup {
    open: bool,
    /// The tool and the dynamics the controls were last written for.
    shown: Option<(Tool, ToolDynamics)>,
    /// Where the popup was put, and whether it shows.
    placed: Option<(bool, i32, i32)>,
    /// The curves as last drawn, by canvas id.
    drawn: HashMap<&'static str, (u32, u32, u64)>,
}

impl DynamicsPopup {
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// What the popup reads besides its controls.
#[derive(SystemParam)]
pub struct PopupInput<'w, 's> {
    clicks: MessageReader<'w, 's, ClickEvent>,
    ids: ElementIds<'w, 's>,
    keys: Res<'w, KeyCommands>,
    home: Res<'w, Home>,
    pointer: PointerInput<'w, 's>,
    place: crate::tools::Place<'w, 's>,
}

fn percent_text(v: u32) -> String {
    format!("{v}%")
}

fn curve_text(v: f64) -> String {
    format!("{v:.1}")
}

/// Writes every control of the popup from `d`.
fn write_controls(signals: &mut Signals, tool: Tool, d: &ToolDynamics) {
    signals.set("dyn_title", format!("{} dynamics", tool.label()));
    signals.set("dyn_show_angle", turns(tool));
    signals.set("dyn_show_color", colors(tool));
    signals.set("dyn_size", d.size.label());
    signals.set("dyn_size_min", d.min_size.to_string());
    signals.set("dyn_size_max", d.max_size.to_string());
    signals.set("dyn_angle", d.angle.label());
    signals.set("dyn_angle_min", d.min_angle.to_string());
    signals.set("dyn_angle_max", d.max_angle.to_string());
    signals.set("dyn_color", d.color.label());
    signals.set("dyn_dither", d.dither.clone());
    for (key, r) in [("p", &d.pressure), ("v", &d.velocity)] {
        signals.set(&format!("dyn_{key}_curve"), r.curve);
        signals.set(&format!("dyn_{key}_curve_text"), curve_text(r.curve));
        signals.set(&format!("dyn_{key}_min"), f64::from(r.min));
        signals.set(&format!("dyn_{key}_min_text"), percent_text(r.min));
        signals.set(&format!("dyn_{key}_max"), f64::from(r.max));
        signals.set(&format!("dyn_{key}_max_text"), percent_text(r.max));
    }
}

/// The dynamics the controls say, starting from `last`, the dynamics they
/// were written from: a control that no longer shows its value of `last`
/// was moved.
fn read_controls(signals: &Signals, last: &ToolDynamics) -> ToolDynamics {
    let mut d = last.clone();
    let text = |name: &str| signals.get::<String>(name);
    let follow =
        |name: &str, was: Follow| text(name).and_then(|t| Follow::named(&t)).unwrap_or(was);
    d.size = follow("dyn_size", last.size);
    d.angle = follow("dyn_angle", last.angle);
    d.color = follow("dyn_color", last.color);
    if let Some(t) = text("dyn_dither")
        && DITHERS.contains(&t.as_str())
    {
        d.dither = t;
    }
    let parsed = |name: &str, was: i64| {
        text(name)
            .filter(|t| *t != was.to_string())
            .and_then(|t| t.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .map_or(was, |v| v.round() as i64)
    };
    d.min_size = parsed("dyn_size_min", i64::from(last.min_size)).clamp(0, 400) as u32;
    d.max_size = parsed("dyn_size_max", i64::from(last.max_size)).clamp(0, 400) as u32;
    d.min_angle = parsed("dyn_angle_min", i64::from(last.min_angle)).clamp(-360, 360) as i32;
    d.max_angle = parsed("dyn_angle_max", i64::from(last.max_angle)).clamp(-360, 360) as i32;
    for (key, r, was) in [
        ("p", &mut d.pressure, &last.pressure),
        ("v", &mut d.velocity, &last.velocity),
    ] {
        let num = |name: String| signals.get::<f64>(&name).filter(|v| v.is_finite());
        if let Some(v) = num(format!("dyn_{key}_curve"))
            && (v - was.curve).abs() > 1e-9
        {
            r.curve = ((v * 10.0).round() / 10.0).clamp(0.1, 10.0);
        }
        if let Some(v) = num(format!("dyn_{key}_min")) {
            let v = v.round().clamp(0.0, 100.0) as u32;
            if v != was.min {
                r.min = v;
            }
        }
        if let Some(v) = num(format!("dyn_{key}_max")) {
            let v = v.round().clamp(0.0, 100.0) as u32;
            if v != was.max {
                r.max = v;
            }
        }
    }
    d
}

/// Opens and closes the popup, and keeps its controls and the active
/// tool's dynamics in step.
pub fn dynamics_popup(
    editor: Res<Editor>,
    mut settings: ResMut<Settings>,
    mut popup: ResMut<DynamicsPopup>,
    mut input: PopupInput,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let tool = editor.tool;
    let was_open = popup.open;
    let clicked: Vec<Entity> = input.clicks.read().map(|c| c.entity).collect();
    for entity in clicked {
        match input.ids.id_of(entity).as_deref() {
            Some("opt-dynamics") => popup.open = !was_open,
            Some("dyn-close") => popup.open = false,
            _ => {}
        }
    }
    // A press on the canvas, Escape, the start screen or a tool without
    // dynamics closes it.
    let over_stage = input.place.over_stage();
    let pressed = input
        .pointer
        .events()
        .iter()
        .any(|e| matches!(e, Pointer::Down { .. }));
    if (pressed && over_stage)
        || input.keys.ran("cancel", false)
        || input.home.open
        || !dynamic(tool)
    {
        popup.open = false;
    }
    if dynamic(tool) {
        let current = settings.dynamics(tool);
        let mut next = current.clone();
        if let Some((t, last)) = &popup.shown
            && *t == tool
        {
            next = read_controls(&signals, last);
            if next != *last && next != current {
                settings.set_dynamics(tool, next.clone());
            } else {
                next = current;
            }
        }
        if popup.shown.as_ref() != Some(&(tool, next.clone())) {
            write_controls(&mut signals, tool, &next);
            popup.shown = Some((tool, next));
            crate::repaint_soon(&wake);
        }
    }
    // The popup opens under its button.
    let at = if popup.open {
        dom::get_by_id("opt-dynamics")
            .and_then(|b| b.rect())
            .map_or((200, 64), |r| {
                (
                    r.client_x.round() as i32,
                    (r.client_y + r.height).round() as i32 + 4,
                )
            })
    } else {
        (0, 0)
    };
    let placed = (popup.open, at.0, at.1);
    if popup.placed != Some(placed)
        && let Some(node) = dom::get_by_id("dynamics-popup")
    {
        if popup.open {
            node.set_style("inset", &format!("{} auto auto {}", at.1, at.0));
        }
        crate::set_class(node, "open", popup.open);
        popup.placed = Some(placed);
        crate::repaint_soon(&wake);
    }
}

fn canvas_color(c: Rgba) -> lumenui_canvas::Rgba {
    lumenui_canvas::Rgba::new(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        f64::from(c.a) / 255.0,
    )
}

/// Draws each input's curve in the popup: what the brush takes (up) from
/// each reading (across), in the theme's colors.
pub fn draw_curves(
    mut popup: ResMut<DynamicsPopup>,
    editor: Res<Editor>,
    settings: Res<Settings>,
    canvases: Query<(
        &lumenui_canvas::Canvas,
        &lumenui::lumen_core::components::Transform,
    )>,
) {
    if !popup.open || !dynamic(editor.tool) {
        return;
    }
    let d = settings.dynamics(editor.tool);
    for (id, r) in [
        ("dyn-pressure-curve", d.pressure.response()),
        ("dyn-velocity-curve", d.velocity.response()),
    ] {
        let Some((_, t)) = canvases.iter().find(|(c, _)| c.id == id) else {
            continue;
        };
        let (w, h) = (t.size.x.round() as u32, t.size.y.round() as u32);
        if w < 8 || h < 8 {
            continue;
        }
        let key = (
            w,
            h,
            u64::from(r.min.to_bits()) << 32
                ^ u64::from(r.max.to_bits())
                ^ u64::from(r.curve.to_bits()) << 16,
        );
        if popup.drawn.get(id) == Some(&key) {
            continue;
        }
        let probe = |id: &str| {
            dom::get_by_id(id)?
                .computed_style_of("bg")
                .as_deref()
                .and_then(parse_hex)
        };
        let (Some(line), Some(grid)) = (probe("probe-curve"), probe("probe-curve-grid")) else {
            return;
        };
        let ctx = Context2d::new(id);
        ctx.resize(w as f32, h as f32);
        ctx.clear();
        let (fw, fh) = (f64::from(w), f64::from(h));
        let pad = 4.0;
        let y_of = |amount: f64| fh - pad - amount * (fh - 2.0 * pad);
        ctx.set_fill_color(canvas_color(grid));
        ctx.fill_rect(pad, y_of(1.0).round(), fw - 2.0 * pad, 1.0);
        ctx.fill_rect(pad, fh - pad, fw - 2.0 * pad, 1.0);
        ctx.set_stroke_color(canvas_color(line));
        ctx.set_line_width(2.0);
        ctx.begin_path();
        let steps = 48;
        for i in 0..=steps {
            let input = i as f32 / steps as f32;
            let x = pad + f64::from(input) * (fw - 2.0 * pad);
            let y = y_of(f64::from(r.apply(input)));
            if i == 0 {
                ctx.move_to(x, y);
            } else {
                ctx.line_to(x, y);
            }
        }
        ctx.stroke();
        popup.drawn.insert(id, key);
    }
}

/// Adds the popup's resource, signals and systems.
pub fn install(app: App) -> App {
    use lumenui::lumen_core::input::ClickSet;
    use lumenui::lumen_core::signals;
    use lumenui_canvas::CanvasSet;
    let d = ToolDynamics::for_tool(Tool::Pencil);
    let mut app = app
        .insert_resource(DynamicsPopup::default())
        .insert_signal("show_dynamics", true)
        .insert_signal("dyn_title", "Pencil dynamics")
        .insert_signal("dyn_show_angle", true)
        .insert_signal("dyn_show_color", true)
        .insert_signal("dyn_size", d.size.label())
        .insert_signal("dyn_size_min", d.min_size.to_string())
        .insert_signal("dyn_size_max", d.max_size.to_string())
        .insert_signal("dyn_angle", d.angle.label())
        .insert_signal("dyn_angle_min", d.min_angle.to_string())
        .insert_signal("dyn_angle_max", d.max_angle.to_string())
        .insert_signal("dyn_color", d.color.label())
        .insert_signal("dyn_dither", d.dither.clone());
    for key in ["p", "v"] {
        let r = InputResponse::default();
        app = app
            .insert_signal(format!("dyn_{key}_curve"), r.curve)
            .insert_signal(format!("dyn_{key}_curve_text"), curve_text(r.curve))
            .insert_signal(format!("dyn_{key}_min"), f64::from(r.min))
            .insert_signal(format!("dyn_{key}_min_text"), percent_text(r.min))
            .insert_signal(format!("dyn_{key}_max"), f64::from(r.max))
            .insert_signal(format!("dyn_{key}_max_text"), percent_text(r.max));
    }
    app.add_systems(
        TickStage::Systems,
        crate::perf::timed("dynamics::dynamics_popup", dynamics_popup)
            .after(crate::tools::pick_tool)
            .before(crate::tools::use_tools)
            .after(ClickSet::Producers)
            .after(signals::push_slider_to_signal)
            .after(signals::push_textinput_to_signal)
            .before(signals::apply_text_bindings)
            .before(signals::apply_value_bindings),
    )
    .add_systems(
        TickStage::Systems,
        crate::perf::timed("dynamics::draw_curves", draw_curves)
            .after(dynamics_popup)
            .before(CanvasSet::Encode),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pencil_and_the_eraser_start_sized_by_pressure() {
        assert_eq!(ToolDynamics::for_tool(Tool::Pencil).size, Follow::Pressure);
        assert_eq!(ToolDynamics::for_tool(Tool::Eraser).size, Follow::Pressure);
        assert_eq!(ToolDynamics::for_tool(Tool::Brush), ToolDynamics::default());
        assert!(dynamic(Tool::Smudge) && !dynamic(Tool::Spray) && !dynamic(Tool::Line));
        assert!(colors(Tool::Pencil) && !colors(Tool::Eraser));
        assert!(turns(Tool::Shade) && !turns(Tool::Brush));
    }

    #[test]
    fn old_pen_settings_become_pressure_dynamics() {
        let pen =
            serde_json::json!({"pressure": true, "curve": 2.0, "min_size": 50, "max_size": 150});
        let d = ToolDynamics::from_pen(&pen).unwrap();
        assert_eq!(d.size, Follow::Pressure);
        assert_eq!((d.min_size, d.max_size), (50, 150));
        assert_eq!(d.pressure.curve, 2.0);
        let off = ToolDynamics::from_pen(&serde_json::json!({"pressure": false})).unwrap();
        assert_eq!(off.size, Follow::Off);
        assert!(ToolDynamics::from_pen(&serde_json::json!("nonsense")).is_none());
        // The paint side sizes the brush the way the pen settings did.
        let p = d.to_paint(|_| DitherPattern::None);
        let half = p.dab(
            8,
            Reading {
                pressure: 0.5,
                velocity: 0.0,
            },
        );
        assert_eq!(half.size, 6, "50% + 100% * 0.5^2 of 8");
    }

    #[test]
    fn a_blend_has_no_dither_and_a_dither_name_asks_for_its_pattern() {
        let blend = ToolDynamics::default().to_paint(|_| unreachable!());
        assert_eq!(blend.dither, None);
        let d = ToolDynamics {
            dither: "Blue noise".into(),
            ..ToolDynamics::default()
        };
        let mut asked = String::new();
        let p = d.to_paint(|n| {
            asked = n.to_string();
            DitherPattern::BlueNoise
        });
        assert_eq!(asked, "Blue noise");
        assert_eq!(p.dither, Some(DitherPattern::BlueNoise));
    }

    #[test]
    fn values_out_of_range_are_pulled_in() {
        let mut d = ToolDynamics {
            min_size: 9000,
            pressure: InputResponse {
                min: 300,
                max: 100,
                curve: f64::NAN,
            },
            dither: "Plaid".into(),
            ..ToolDynamics::default()
        };
        d.clamp();
        assert_eq!(d.min_size, 400);
        assert_eq!(d.pressure.min, 100);
        assert_eq!(d.pressure.curve, 1.0);
        assert_eq!(d.dither, "Blend");
    }

    #[test]
    fn the_summary_names_what_follows_an_input() {
        assert_eq!(ToolDynamics::default().summary(), None);
        let d = ToolDynamics {
            size: Follow::Pressure,
            color: Follow::Velocity,
            ..ToolDynamics::default()
        };
        assert_eq!(
            d.summary().as_deref(),
            Some("size by pressure, color by velocity")
        );
    }
}
