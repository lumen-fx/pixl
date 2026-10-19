//! The settings dialog (Edit > Settings, Ctrl+, by default): one page per
//! part of the [`Settings`], in a split beside the page list. Every control
//! writes its setting as it changes, and a setting changed elsewhere (the
//! menu row's theme switch) moves its control.

use std::collections::HashMap;

use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;

use crate::ids::ElementIds;
use crate::panels::escape;
use crate::settings::keymap::{self, Chord, KeyCommands, Keymap, Kind, Preset};
use crate::settings::{Settings, Theme, UiFont, WheelMode};
use crate::view::{Layers, ViewColors, parse_hex};

/// The pages of the dialog, in the order the page list shows them.
pub const PAGES: [&str; 6] = [
    "navigation",
    "canvas",
    "interface",
    "files",
    "performance",
    "keyboard",
];

/// A control's value as its bound signal holds it.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Text(String),
    Bool(bool),
    Num(f64),
}

impl Val {
    fn read(signals: &Signals, name: &str, like: &Val) -> Option<Val> {
        Some(match like {
            Val::Text(_) => Val::Text(signals.get::<String>(name)?),
            Val::Bool(_) => Val::Bool(signals.get::<bool>(name)?),
            Val::Num(_) => Val::Num(signals.get::<f64>(name)?),
        })
    }

    fn write(&self, signals: &mut Signals, name: &str) {
        match self {
            Val::Text(t) => signals.set(name, t.clone()),
            Val::Bool(b) => signals.set(name, *b),
            Val::Num(n) => signals.set(name, *n),
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Val::Text(t) => Some(t),
            _ => None,
        }
    }

    /// A number typed in a field, if the field holds one.
    fn number(&self) -> Option<f64> {
        match self {
            Val::Text(t) => t.trim().parse().ok().filter(|n: &f64| n.is_finite()),
            Val::Num(n) => Some(*n),
            Val::Bool(_) => None,
        }
    }

    /// Whether a field showing `self` already shows `other`: the same
    /// number typed another way (`1.50` for 1.5) counts.
    fn shows(&self, other: &Val) -> bool {
        match (self, other) {
            (Val::Text(_), Val::Text(_)) => {
                self == other
                    || matches!((self.number(), other.number()), (Some(a), Some(b)) if a == b)
            }
            _ => self == other,
        }
    }
}

/// A number as a field shows it: no trailing zeros.
pub fn num_text(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// One control: its signal, how to show a setting in it, and how to take
/// what the user entered. `set` refuses a value out of range, which leaves
/// the field showing what was typed (and marked invalid by its own
/// `min`/`max`) until it holds one the setting takes.
struct Field {
    signal: &'static str,
    get: fn(&Settings, &Showing) -> Val,
    set: fn(&mut Settings, &Val, &Showing) -> bool,
}

/// The colors the canvas shows now, for a color setting that follows the
/// theme: its field shows the theme's color.
#[derive(Clone, Copy, Debug, Default)]
pub struct Showing {
    /// The checkerboard's two colors, the grid's and the stage's.
    pub colors: [Option<pixl_document::Rgba>; 4],
}

fn in_range(v: &Val, lo: f64, hi: f64) -> Option<f64> {
    v.number().filter(|n| (lo..=hi).contains(n))
}

macro_rules! number {
    ($signal:literal, $lo:expr, $hi:expr, |$s:ident| $field:expr, $ty:ty) => {
        Field {
            signal: $signal,
            get: |$s, _| Val::Text(num_text($field as f64)),
            set: |$s, v, _| match in_range(v, $lo as f64, $hi as f64) {
                Some(n) => {
                    $field = n as $ty;
                    true
                }
                None => false,
            },
        }
    };
}

macro_rules! flag {
    ($signal:literal, |$s:ident| $field:expr) => {
        Field {
            signal: $signal,
            get: |$s, _| Val::Bool($field),
            set: |$s, v, _| match v {
                Val::Bool(b) => {
                    $field = *b;
                    true
                }
                _ => false,
            },
        }
    };
}

/// A color setting that may follow the theme (`None`): its field, and the
/// "The theme's" switch beside it. Picking a color makes it the user's.
macro_rules! color {
    ($signal:literal, $theme:literal, $slot:expr, |$s:ident| $field:expr) => {
        [
            Field {
                signal: $signal,
                get: |$s, now| Val::Text(css_color(shown_color(&$field, now.colors[$slot]))),
                set: |$s, v, now| match v.text().and_then(parse_css_color) {
                    Some(c) => {
                        // A field may write back the color it was given:
                        // only a different one is a pick.
                        if $field.is_some() || Some(c) != now.colors[$slot] {
                            $field = Some(crate::tools::hex(c));
                        }
                        true
                    }
                    None => false,
                },
            },
            Field {
                signal: $theme,
                get: |$s, _| Val::Bool($field.is_none()),
                set: |$s, v, now| match v {
                    Val::Bool(true) => {
                        $field = None;
                        true
                    }
                    Val::Bool(false) => {
                        if $field.is_none() {
                            let c = shown_color(&$field, now.colors[$slot]);
                            $field = Some(crate::tools::hex(c));
                        }
                        true
                    }
                    _ => false,
                },
            },
        ]
    };
}

/// The color a setting shows: its own, or the theme's while it follows it.
fn shown_color(set: &Option<String>, theme: Option<pixl_document::Rgba>) -> pixl_document::Rgba {
    set.as_deref()
        .and_then(parse_hex)
        .or(theme)
        .unwrap_or(pixl_document::Rgba::opaque(0x80, 0x80, 0x80))
}

/// A color as a color field writes its value.
fn css_color(c: pixl_document::Rgba) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        let f = |v: u8| num_text(f64::from(v) / 255.0);
        format!("color(srgb {} {} {} / {})", f(c.r), f(c.g), f(c.b), f(c.a))
    }
}

/// Reads a color field's value: `#rgb`, `#rrggbb`, `#rrggbbaa` or
/// `color(srgb r g b / a)`.
pub fn parse_css_color(text: &str) -> Option<pixl_document::Rgba> {
    let t = text.trim();
    if let Some(body) = t
        .strip_prefix("color(srgb")
        .and_then(|r| r.strip_suffix(')'))
    {
        let (rgb, alpha) = match body.split_once('/') {
            Some((rgb, a)) => (rgb, a.trim().parse::<f64>().ok()?),
            None => (body, 1.0),
        };
        let parts: Vec<f64> = rgb
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<std::result::Result<_, _>>()
            .ok()?;
        let [r, g, b] = parts[..] else {
            return None;
        };
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        return Some(pixl_document::Rgba::new(
            byte(r),
            byte(g),
            byte(b),
            byte(alpha),
        ));
    }
    if let Some(h) = t.strip_prefix('#')
        && h.len() == 3
    {
        let d: Vec<u8> = h
            .chars()
            .map(|c| c.to_digit(16).map(|v| v as u8 * 17))
            .collect::<Option<_>>()?;
        return Some(pixl_document::Rgba::opaque(d[0], d[1], d[2]));
    }
    parse_hex(t)
}

/// Every control of the dialog but the shortcut list.
fn fields() -> Vec<Field> {
    let mut f = vec![
        // Navigation.
        number!(
            "set_wheel_zoom",
            0.1,
            8,
            |s| s.navigation.wheel_zoom_speed,
            f64
        ),
        number!("set_pan_speed", 0.1, 8, |s| s.navigation.pan_speed, f64),
        number!(
            "set_key_pan_speed",
            50,
            10000,
            |s| s.navigation.key_pan_speed,
            f64
        ),
        number!(
            "set_key_pan_fast",
            1,
            20,
            |s| s.navigation.key_pan_fast,
            f64
        ),
        flag!("set_zoom_to_cursor", |s| s.navigation.zoom_to_cursor),
        flag!("set_invert_wheel", |s| s.navigation.invert_wheel),
        Field {
            signal: "set_wheel_mode",
            get: |s, _| {
                Val::Text(
                    match s.navigation.wheel {
                        WheelMode::Zoom => "Zooms",
                        WheelMode::Pan => "Pans",
                    }
                    .into(),
                )
            },
            set: |s, v, _| {
                s.navigation.wheel = match v.text() {
                    Some("Zooms") => WheelMode::Zoom,
                    Some("Pans") => WheelMode::Pan,
                    _ => return false,
                };
                true
            },
        },
        // Canvas.
        number!("set_checker_size", 1, 128, |s| s.canvas.checker_size, u32),
        flag!("set_pixel_grid", |s| s.canvas.pixel_grid),
        number!("set_grid_from", 100, 6400, |s| s.canvas.grid_from_zoom, f64),
        number!(
            "set_custom_grid_w",
            1,
            4096,
            |s| s.canvas.custom_grid[0],
            u32
        ),
        number!(
            "set_custom_grid_h",
            1,
            4096,
            |s| s.canvas.custom_grid[1],
            u32
        ),
        // Interface.
        Field {
            signal: "set_theme",
            get: |s, _| Val::Text(s.interface.theme.label().into()),
            set: |s, v, _| match Theme::ALL.into_iter().find(|t| Some(t.label()) == v.text()) {
                Some(t) => {
                    if t != s.interface.theme {
                        s.interface.theme = t;
                        s.interface.user_theme = None;
                    }
                    true
                }
                None => false,
            },
        },
        Field {
            signal: "set_font",
            get: |s, _| {
                Val::Text(
                    match s.interface.font {
                        UiFont::Pixel => "Pixel",
                        UiFont::Plain => "Plain",
                    }
                    .into(),
                )
            },
            set: |s, v, _| {
                s.interface.font = match v.text() {
                    Some("Pixel") => UiFont::Pixel,
                    Some("Plain") => UiFont::Plain,
                    _ => return false,
                };
                true
            },
        },
        Field {
            signal: "set_ui_scale",
            get: |s, _| Val::Text(format!("{}%", s.interface.scale)),
            set: |s, v, _| match v
                .text()
                .and_then(|t| t.trim_end_matches('%').parse::<u32>().ok())
                .filter(|n| crate::settings::UI_SCALES.contains(n))
            {
                Some(n) => {
                    s.interface.scale = n;
                    true
                }
                None => false,
            },
        },
        flag!("set_tooltips", |s| s.interface.tooltips),
        // Files.
        flag!("set_recovery", |s| s.files.recovery),
        number!("set_autosave", 0, 3600, |s| s.files.autosave_seconds, u32),
        number!("set_recent_count", 1, 200, |s| s.files.recent_count, usize),
        // Performance.
        flag!("set_show_perf", |s| s.performance.show_fps),
        number!(
            "set_progressive",
            1,
            4096,
            |s| s.performance.progressive_tiles,
            usize
        ),
        number!(
            "set_workers",
            0,
            256,
            |s| s.performance.worker_threads,
            usize
        ),
        number!(
            "set_buffer_budget",
            64,
            65536,
            |s| s.performance.buffer_budget_mib,
            u64
        ),
        // Keyboard.
        Field {
            signal: "set_keys_preset",
            get: |s, _| Val::Text(s.shortcuts.preset.label().into()),
            set: |s, v, _| match v.text().and_then(Preset::from_label) {
                Some(p) => {
                    s.shortcuts.preset = p;
                    true
                }
                None => false,
            },
        },
    ];
    // The two checker colors share one switch.
    let [checker_a, _] = color!("set_checker_a", "set_checker_theme", 0, |s| s
        .canvas
        .checker_a);
    let [checker_b, _] = color!("set_checker_b", "set_checker_theme", 1, |s| s
        .canvas
        .checker_b);
    f.push(checker_a);
    f.push(checker_b);
    f.extend(color!("set_grid_color", "set_grid_theme", 2, |s| s
        .canvas
        .grid_color));
    f.extend(color!("set_background", "set_background_theme", 3, |s| s
        .canvas
        .background));
    f.push(Field {
        signal: "set_checker_theme",
        get: |s, _| Val::Bool(s.canvas.checker_a.is_none() && s.canvas.checker_b.is_none()),
        set: |s, v, now| match v {
            Val::Bool(true) => {
                s.canvas.checker_a = None;
                s.canvas.checker_b = None;
                true
            }
            Val::Bool(false) => {
                for (slot, field) in [&mut s.canvas.checker_a, &mut s.canvas.checker_b]
                    .into_iter()
                    .enumerate()
                {
                    if field.is_none() {
                        *field = Some(crate::tools::hex(shown_color(field, now.colors[slot])));
                    }
                }
                true
            }
            _ => false,
        },
    });
    f
}

/// What the dialog is doing.
#[derive(Resource, Default)]
pub struct SettingsDialog {
    pub page: usize,
    /// The command whose chord is being caught.
    pub capturing: Option<String>,
    /// A caught chord another command has, waiting for the user to take it.
    pub conflict: Option<(String, Chord)>,
    /// Per control, the value it was last given and the value the setting
    /// had then.
    shown: HashMap<&'static str, (Val, Val)>,
    /// The shortcut list as last written, and the keymap and catching
    /// command it was written for.
    keys_markup: Option<String>,
    keys_for: Option<(Keymap, Option<String>)>,
    /// What the Performance page says of the renderer.
    renderer: Option<String>,
}

/// The page a page button opens.
fn page_of(id: &str) -> Option<usize> {
    PAGES
        .iter()
        .position(|p| id.strip_prefix("settings-page-") == Some(p))
}

/// Opens and closes the dialog, switches its pages, resets a page, and
/// opens the Edit menu.
#[allow(clippy::too_many_arguments)]
pub fn settings_commands(
    mut dialog: ResMut<SettingsDialog>,
    mut settings: ResMut<Settings>,
    mut keys: ResMut<KeyCommands>,
    mut clicks: MessageReader<ClickEvent>,
    mut menu: MessageReader<MenuClicked>,
    ids: ElementIds,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    let clicked: Vec<Entity> = clicks.read().map(|c| c.entity).collect();
    chosen.extend(clicked.into_iter().filter_map(|e| ids.id_of(e)));
    if keys.ran("settings", false) {
        chosen.push("settings".into());
    }
    let open = signals.get::<bool>("settings_open").unwrap_or(false);
    let mut changed = false;
    for id in chosen {
        match id.as_str() {
            "menu-edit" => {
                signals.toggle("__menu_open:edit-menu");
            }
            "settings" | "edit-settings" => {
                signals.set("settings_open", !open);
                changed = true;
            }
            "settings-done" => {
                signals.set("settings_open", false);
                dialog.capturing = None;
                keys.capture = false;
                changed = true;
            }
            "settings-reset-page" => {
                reset_page(&mut settings, PAGES[dialog.page]);
                changed = true;
            }
            other => {
                if let Some(page) = page_of(other) {
                    dialog.page = page;
                    changed = true;
                }
            }
        }
    }
    // Show the page that is picked, and light its button.
    for (i, page) in PAGES.iter().enumerate() {
        let on = i == dialog.page;
        let name = format!("settings_on_{page}");
        if signals.get::<bool>(&name) != Some(on) {
            signals.set(&name, on);
            changed = true;
        }
        if let Some(b) = dom::get_by_id(&format!("settings-page-{page}")) {
            crate::set_class(b, "active", on);
        }
    }
    if changed {
        crate::repaint_soon(&wake);
    }
}

/// Puts one page's settings back to their defaults.
pub fn reset_page(settings: &mut Settings, page: &str) {
    let d = Settings::default();
    match page {
        "navigation" => settings.navigation = d.navigation,
        "canvas" => settings.canvas = d.canvas,
        "interface" => settings.interface = d.interface,
        "files" => settings.files = d.files,
        "performance" => settings.performance = d.performance,
        "keyboard" => settings.shortcuts = d.shortcuts,
        _ => {}
    }
}

/// Keeps every control and its setting in step: a control the user moved
/// writes its setting, and a setting that changed moves its control.
pub fn settings_form(
    mut dialog: ResMut<SettingsDialog>,
    mut settings: ResMut<Settings>,
    layers: Res<Layers>,
    colors: Res<ViewColors>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    // The controls are brought up to date while the dialog shows; what
    // changed while it was closed reaches them when it opens.
    if signals.get::<bool>("settings_open") != Some(true) {
        return;
    }
    let fields = fields();
    let stage = dom::get_by_id("stage")
        .and_then(|n| n.computed_style_of("bg"))
        .as_deref()
        .and_then(parse_hex);
    let now = Showing {
        colors: [colors.checker_a, colors.checker_b, colors.grid, stage],
    };
    let mut edited = settings.clone();
    let mut any = false;
    // What the user changed since the controls were last written.
    for f in &fields {
        let Some((given, _)) = dialog.shown.get(f.signal).cloned() else {
            continue;
        };
        let Some(value) = Val::read(&signals, f.signal, &given) else {
            continue;
        };
        if value != given {
            if (f.set)(&mut edited, &value, &now) {
                any = true;
            }
            if let Some(entry) = dialog.shown.get_mut(f.signal) {
                entry.0 = value;
            }
        }
    }
    if any && edited != *settings {
        *settings = edited;
    }
    // What the settings say that the controls do not show yet.
    let mut wrote = false;
    for f in &fields {
        let want = (f.get)(&settings, &now);
        match dialog.shown.get(f.signal).cloned() {
            Some((_, had)) if had == want => {}
            Some((given, _)) if given.shows(&want) => {
                dialog.shown.insert(f.signal, (given, want));
            }
            _ => {
                want.write(&mut signals, f.signal);
                dialog.shown.insert(f.signal, (want.clone(), want));
                wrote = true;
            }
        }
    }
    let renderer = dialog.renderer.get_or_insert_with(renderer_text).clone();
    crate::tools::set_if_changed(&mut signals, "set_renderer", renderer);
    let workers = format!("0 uses one per core; {} running.", layers.workers());
    crate::tools::set_if_changed(&mut signals, "set_workers_hint", workers);
    if wrote {
        crate::repaint_soon(&wake);
    }
}

/// What the Performance page says of the renderer: what `lumen.toml` asks
/// for. Under `auto` Lumen does not tell the app which renderer started.
pub fn renderer_text() -> String {
    let toml =
        std::fs::read_to_string(lumenui::lumen_core::app_paths::app_dir().join("lumen.toml"))
            .unwrap_or_default();
    match configured_backend(&toml).as_deref() {
        Some("gpu") => "GPU (wgpu)".into(),
        Some("cpu") => "CPU".into(),
        _ => "Automatic: the GPU, or the CPU when no GPU starts".into(),
    }
}

/// The `[render] backend` of a `lumen.toml`.
pub fn configured_backend(toml: &str) -> Option<String> {
    let mut in_render = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_render = line == "[render]";
            continue;
        }
        if in_render
            && let Some(v) = line.strip_prefix("backend")
            && let Some(v) = v.trim_start().strip_prefix('=')
        {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// The shortcut list: one row per command with its chords, a button that
/// catches a new one, and buttons that clear it or put the preset's back.
fn keys_markup(map: &Keymap, capturing: Option<&str>) -> String {
    let clashes = map.clashes();
    let mut out = String::new();
    let mut group = "";
    for c in keymap::commands() {
        if c.group != group {
            group = c.group;
            out.push_str(&format!(
                r#"<label class="keys-group" text="{}"/>"#,
                escape(group)
            ));
        }
        let chords = map.chords(&c.id);
        let clash = chords.iter().any(|ch| clashes.contains_key(ch));
        let text = if capturing == Some(c.id.as_str()) {
            "Press a key...".to_string()
        } else {
            map.describe(&c.id)
        };
        let mut class = String::from("key-row");
        if clash {
            class.push_str(" clash");
        }
        if capturing == Some(c.id.as_str()) {
            class.push_str(" capturing");
        }
        let hold = if c.kind == Kind::Hold { " (hold)" } else { "" };
        out.push_str(&format!(
            r#"<row id="key-row-{id}" class="{class}"><label class="key-label" text="{label}{hold}"/><spacer/><button id="key-bind-{id}" class="key-chord small" text="{text}"/><button id="key-clear-{id}" class="flat small" text="Clear"/><button id="key-reset-{id}" class="flat small" text="Reset"/></row>"#,
            id = c.id,
            label = escape(&c.label),
            text = escape(&text),
        ));
    }
    out
}

/// The label of a command.
fn label_of(id: &str) -> String {
    keymap::commands()
        .into_iter()
        .find(|c| c.id == id)
        .map_or_else(|| id.to_string(), |c| c.label)
}

/// The Keyboard page: catches a chord for a command, says when it is taken,
/// and keeps the list showing the keymap.
#[allow(clippy::too_many_arguments)]
pub fn keys_page(
    mut dialog: ResMut<SettingsDialog>,
    mut settings: ResMut<Settings>,
    mut keys: ResMut<KeyCommands>,
    map: Res<Keymap>,
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let mut message: Option<String> = None;
    for click in clicks.read() {
        let Ok(id) = ids.of(click.entity) else {
            continue;
        };
        let id = id.0.as_str();
        if let Some(cmd) = id.strip_prefix("key-bind-") {
            dialog.capturing = Some(cmd.to_string());
            dialog.conflict = None;
            keys.capture = true;
            keys.captured = None;
            message = Some(format!(
                "Press the keys for {}, or Escape to keep what it has.",
                label_of(cmd)
            ));
        } else if let Some(cmd) = id.strip_prefix("key-clear-") {
            keymap::unbind(&mut settings, cmd);
            message = Some(format!("{} has no key now.", label_of(cmd)));
        } else if let Some(cmd) = id.strip_prefix("key-reset-") {
            keymap::reset_one(&mut settings, cmd);
        } else if id == "keys-reset-all" {
            settings.shortcuts.overrides.clear();
            message = Some(format!(
                "Every shortcut is the {} preset's again.",
                settings.shortcuts.preset.label()
            ));
        } else if id == "keys-take" {
            if let Some((cmd, chord)) = dialog.conflict.take() {
                let taken = keymap::rebind(&mut settings, &cmd, chord.clone());
                let from: Vec<String> = taken.iter().map(|t| label_of(t)).collect();
                message = Some(format!(
                    "{chord} runs {} now; {} has no key for it.",
                    label_of(&cmd),
                    from.join(", ")
                ));
            }
        } else if id == "keys-keep" {
            dialog.conflict = None;
            message = Some(String::new());
        }
    }
    if let Some(chord) = keys.captured.take()
        && let Some(cmd) = dialog.capturing.take()
    {
        if chord.key == "Escape" && !chord.ctrl && !chord.alt && !chord.shift {
            message = Some(String::new());
        } else {
            let taken = map.conflicts(&cmd, &chord);
            if taken.is_empty() {
                keymap::rebind(&mut settings, &cmd, chord.clone());
                message = Some(format!("{chord} runs {} now.", label_of(&cmd)));
            } else {
                let names: Vec<String> = taken.iter().map(|t| label_of(t)).collect();
                message = Some(format!(
                    "{chord} already runs {}. Use it for {} instead?",
                    names.join(", "),
                    label_of(&cmd)
                ));
                dialog.conflict = Some((cmd, chord));
            }
        }
    }
    if !signals.get::<bool>("settings_open").unwrap_or(false) && dialog.capturing.is_some() {
        dialog.capturing = None;
        keys.capture = false;
    }
    let conflict = dialog.conflict.is_some();
    if signals.get::<bool>("keys_conflict") != Some(conflict) {
        signals.set("keys_conflict", conflict);
    }
    if let Some(m) = message {
        signals.set("keys_message", m);
        crate::repaint_soon(&wake);
    }
    // The list is written again when the keymap or the command catching
    // a key changes.
    let current = dialog
        .keys_for
        .as_ref()
        .is_some_and(|(m, c)| *m == *map && *c == dialog.capturing);
    if !current && let Some(list) = dom::get_by_id("keys-list") {
        let markup = keys_markup(&map, dialog.capturing.as_deref());
        if dialog.keys_markup.as_deref() != Some(markup.as_str()) {
            list.set_inner_markup(&markup);
            dialog.keys_markup = Some(markup);
            crate::repaint_soon(&wake);
        }
        dialog.keys_for = Some((map.clone(), dialog.capturing.clone()));
    }
}

/// The menu items that name a command, by their text before the shortcut.
const MENU_COMMANDS: &[(&str, &str)] = &[
    ("New...", "file-new"),
    ("Open...", "file-open"),
    ("Save", "file-save"),
    ("Save as...", "file-save-as"),
    ("Export PNG...", "file-export-png"),
    ("Export GIF...", "file-export-gif"),
    ("Export sprite sheet...", "file-export-sheet"),
    ("Import sprite sheet...", "file-import-sheet"),
    ("Close", "file-close"),
    ("Undo", "undo"),
    ("Redo", "redo"),
    ("Cut", "edit-cut"),
    ("Copy", "edit-copy"),
    ("Copy merged", "edit-copy-merged"),
    ("Paste", "edit-paste"),
    ("Paste as new layer", "edit-paste-layer"),
    ("Paste as new document", "edit-paste-document"),
    ("Settings...", "settings"),
];

/// Writes each menu item's shortcut after its name, from the keymap, so a
/// rebound key shows in the menus. It looks at the menus once per keymap:
/// the bar in the window holds its menus' items from the start, numbered
/// `menubar-0` up, and a bar the platform draws holds none of them.
pub fn menu_labels(map: Res<Keymap>, mut shown: Local<Option<Keymap>>) {
    if shown.as_ref() == Some(&*map) {
        return;
    }
    let menus = (0..).map_while(|i| dom::get_by_id(&format!("menubar-{i}")));
    for item in menus.flat_map(|m| m.children()) {
        let Some(text) = item.text() else {
            continue;
        };
        let name = text.split(" (").next().unwrap_or(&text).to_string();
        let Some((_, id)) = MENU_COMMANDS.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        let want = match map.chords(id).first() {
            Some(chord) => format!("{name} ({chord})"),
            None => name,
        };
        if want != text {
            item.set_text(&want);
        }
    }
    *shown = Some(map.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_fields_read_both_shapes() {
        assert_eq!(
            parse_css_color("#3c3c3c"),
            Some(pixl_document::Rgba::opaque(0x3c, 0x3c, 0x3c))
        );
        assert_eq!(
            parse_css_color("color(srgb 0 1 0 / 0.5)"),
            Some(pixl_document::Rgba::new(0, 255, 0, 128))
        );
        assert_eq!(parse_css_color("#fff"), Some(pixl_document::Rgba::WHITE));
        assert_eq!(parse_css_color("rgb(1,2,3)"), None);
        assert_eq!(
            css_color(pixl_document::Rgba::new(0, 0, 0, 0x59)),
            "color(srgb 0 0 0 / 0.349)"
        );
        assert_eq!(
            css_color(pixl_document::Rgba::opaque(255, 128, 0)),
            "#ff8000"
        );
    }

    #[test]
    fn a_number_typed_another_way_counts_as_shown() {
        let typed = Val::Text("1.50".into());
        assert!(typed.shows(&Val::Text("1.5".into())));
        assert!(!typed.shows(&Val::Text("2".into())));
        assert_eq!(num_text(900.0), "900");
        assert_eq!(num_text(0.25), "0.25");
    }

    #[test]
    fn a_field_refuses_a_value_out_of_range() {
        let fields = fields();
        let f = fields.iter().find(|f| f.signal == "set_pan_speed").unwrap();
        let mut s = Settings::default();
        let now = Showing::default();
        assert!(!(f.set)(&mut s, &Val::Text("0".into()), &now));
        assert!(!(f.set)(&mut s, &Val::Text("fast".into()), &now));
        assert!((f.set)(&mut s, &Val::Text("2.5".into()), &now));
        assert_eq!(s.navigation.pan_speed, 2.5);
    }

    #[test]
    fn the_renderer_line_reads_lumen_toml() {
        let toml = "[app]\nid = \"pixl\"\n\n[render]\nbackend = \"cpu\"\n";
        assert_eq!(configured_backend(toml).as_deref(), Some("cpu"));
        assert_eq!(configured_backend("[app]\nbackend = \"gpu\"\n"), None);
    }

    #[test]
    fn every_page_resets_to_its_defaults() {
        let mut s = Settings::default();
        s.navigation.pan_speed = 3.0;
        s.interface.scale = 200;
        reset_page(&mut s, "navigation");
        assert_eq!(s.navigation, Settings::default().navigation);
        assert_eq!(s.interface.scale, 200);
    }
}
