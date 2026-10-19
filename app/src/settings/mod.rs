//! The user's settings: how the view navigates, how the pen paints, how
//! the canvas and the interface look, how files are kept, how the editor
//! spends its time, and the keyboard shortcuts. They are kept as JSON in
//! pixl's folder of the user data directory and take effect as they change.
//!
//! The look (the dark or light token set, the pixel or plain UI font, the
//! UI scale and whether tooltips show) is a set of classes on the root
//! element; the stylesheet does the rest.

pub mod dialog;
pub mod keymap;
pub mod store;
pub mod themes;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use serde::{Deserialize, Serialize};

use crate::home::Home;
use crate::ids::ElementIds;
use crate::settings::store::Store;

/// The settings file in pixl's data folder.
pub const SETTINGS_FILE: &str = "settings.json";

/// How long a change waits before the file is written, so a dragged slider
/// writes once when it settles.
pub const SAVE_AFTER: Duration = Duration::from_millis(400);

/// What the wheel does over the canvas.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WheelMode {
    /// The wheel zooms; a sideways scroll pans.
    #[default]
    Zoom,
    /// The wheel and two-finger scrolls pan; Ctrl with the wheel (and a
    /// pinch, which arrives that way) zooms.
    Pan,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct NavigationSettings {
    /// Zoom steps per wheel notch.
    pub wheel_zoom_speed: f64,
    /// Zoom around the pointer; off zooms around the middle of the view.
    pub zoom_to_cursor: bool,
    pub wheel: WheelMode,
    /// How far a scroll pans, as a multiple of its distance.
    pub pan_speed: f64,
    /// How fast holding W, A, S or D moves the view, in screen pixels per
    /// second.
    pub key_pan_speed: f64,
    /// How much faster the keys move the view with Shift held.
    pub key_pan_fast: f64,
    /// Turn the wheel the other way.
    pub invert_wheel: bool,
}

impl Default for NavigationSettings {
    fn default() -> Self {
        Self {
            wheel_zoom_speed: 1.0,
            zoom_to_cursor: true,
            wheel: WheelMode::Zoom,
            pan_speed: 1.0,
            key_pan_speed: crate::view::input::KEY_PAN_SPEED,
            key_pan_fast: crate::view::input::KEY_PAN_FAST,
            invert_wheel: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct PenSettings {
    /// Pen pressure sets the brush size.
    pub pressure: bool,
    /// The pressure curve's exponent: below 1 a light touch already paints
    /// wide, above 1 it takes a firm press.
    pub curve: f64,
    /// The brush size at the lightest touch and at full pressure, as
    /// percentages of the tool's size.
    pub min_size: u32,
    pub max_size: u32,
}

impl Default for PenSettings {
    fn default() -> Self {
        Self {
            pressure: true,
            curve: 1.0,
            min_size: 0,
            max_size: 100,
        }
    }
}

impl PenSettings {
    /// The share of the tool's size a press of `pressure` paints with.
    pub fn scale(&self, pressure: f32) -> f64 {
        let p = f64::from(pressure.clamp(0.0, 1.0));
        let curved = p.powf(self.curve.clamp(0.1, 10.0));
        let (lo, hi) = (
            f64::from(self.min_size.min(400)) / 100.0,
            f64::from(self.max_size.min(400)) / 100.0,
        );
        lo + (hi - lo) * curved
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CanvasSettings {
    /// Screen pixels per checkerboard cell.
    pub checker_size: u32,
    /// The checkerboard's two colors as `#rrggbb`; `None` follows the theme.
    pub checker_a: Option<String>,
    pub checker_b: Option<String>,
    /// The pixel grid is on (View > Pixel grid). It shows from
    /// `grid_from_zoom` on.
    pub pixel_grid: bool,
    /// The zoom, in percent, from which the pixel grid shows.
    pub grid_from_zoom: f64,
    /// The pixel grid's color as `#rrggbbaa`; `None` follows the theme.
    pub grid_color: Option<String>,
    /// The cell size a new custom grid starts with, in document pixels.
    pub custom_grid: [u32; 2],
    /// The color around the document; `None` follows the theme.
    pub background: Option<String>,
}

impl Default for CanvasSettings {
    fn default() -> Self {
        Self {
            checker_size: 8,
            checker_a: None,
            checker_b: None,
            pixel_grid: false,
            grid_from_zoom: 800.0,
            grid_color: None,
            custom_grid: [16, 16],
            background: None,
        }
    }
}

/// The built-in token sets. A user theme starts from one of them and
/// overrides some of its tokens.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    Contrast,
    Retro,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Dark, Theme::Light, Theme::Contrast, Theme::Retro];

    /// The class on the root element that selects the set.
    pub fn class(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Contrast => "contrast",
            Theme::Retro => "retro",
        }
    }

    pub fn from_class(class: &str) -> Option<Theme> {
        Theme::ALL.into_iter().find(|t| t.class() == class)
    }

    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
            Theme::Contrast => "High contrast",
            Theme::Retro => "Retro",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UiFont {
    #[default]
    Pixel,
    Plain,
}

/// The UI scales the stylesheet has a token set for, in percent.
pub const UI_SCALES: [u32; 4] = [100, 125, 150, 200];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct InterfaceSettings {
    pub theme: Theme,
    /// The user theme over the built-in set, by name; see `themes`.
    pub user_theme: Option<String>,
    pub font: UiFont,
    /// One of [`UI_SCALES`].
    pub scale: u32,
    pub tooltips: bool,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            user_theme: None,
            font: UiFont::Pixel,
            scale: 100,
            tooltips: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FileSettings {
    /// How often a modified document writes its recovery copy, in seconds.
    pub autosave_seconds: u32,
    /// Keep recovery copies at all, and offer them on open.
    pub recovery: bool,
    /// How many documents the start screen lists.
    pub recent_count: usize,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            autosave_seconds: 30,
            recovery: true,
            recent_count: crate::files::recents::MAX_RECENTS,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct PerformanceSettings {
    /// Dirty tiles a frame redraws itself; a larger redraw goes to the
    /// workers and shows tile by tile.
    pub progressive_tiles: usize,
    /// Worker threads for large redraws; 0 picks one per core.
    pub worker_threads: usize,
    /// Show the performance overlay.
    pub show_fps: bool,
    /// The most layer buffer memory the view hands the renderer, in MiB.
    /// Past it the bottom layers are flattened into one buffer.
    pub buffer_budget_mib: u64,
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            progressive_tiles: 24,
            worker_threads: 0,
            show_fps: false,
            buffer_budget_mib: crate::view::DEFAULT_BUFFER_BUDGET_MIB,
        }
    }
}

/// How the color panel picks: a saturation and value square beside a hue
/// strip, or a hue wheel around the square.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Picker {
    #[default]
    Square,
    Wheel,
}

/// The color model of the color panel's sliders.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Channels {
    #[default]
    Rgb,
    Hsv,
    Hsl,
}

/// The color panel's modes, as last chosen in the panel.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ColorSettings {
    pub picker: Picker,
    pub channels: Channels,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ShortcutSettings {
    /// The shortcut set the overrides start from.
    pub preset: crate::settings::keymap::Preset,
    /// Per command id, the chords that replace the preset's, written as
    /// `Ctrl+Shift+Z`; an empty list leaves the command without a key.
    pub overrides: BTreeMap<String, Vec<String>>,
}

/// Everything the settings dialog sets.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub navigation: NavigationSettings,
    pub pen: PenSettings,
    pub canvas: CanvasSettings,
    pub interface: InterfaceSettings,
    pub files: FileSettings,
    pub performance: PerformanceSettings,
    pub shortcuts: ShortcutSettings,
    pub color: ColorSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            navigation: NavigationSettings::default(),
            pen: PenSettings::default(),
            canvas: CanvasSettings::default(),
            interface: InterfaceSettings::default(),
            files: FileSettings::default(),
            performance: PerformanceSettings::default(),
            shortcuts: ShortcutSettings::default(),
            color: ColorSettings::default(),
        }
    }
}

impl Settings {
    /// Reads settings from JSON. A field that is missing or cannot be read
    /// keeps its default, so a file from another version still opens.
    pub fn from_json(bytes: &[u8]) -> Self {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return Self::default();
        };
        let mut s: Settings = serde_json::from_value(value.clone()).unwrap_or_else(|_| {
            // One bad section should not cost the others.
            let mut s = Settings::default();
            let section = |name: &str| value.get(name).cloned();
            macro_rules! read {
                ($field:ident) => {
                    if let Some(v) = section(stringify!($field))
                        && let Ok(parsed) = serde_json::from_value(v)
                    {
                        s.$field = parsed;
                    }
                };
            }
            read!(navigation);
            read!(pen);
            read!(canvas);
            read!(interface);
            read!(files);
            read!(performance);
            read!(shortcuts);
            read!(color);
            s
        });
        s.clamp();
        s
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }

    /// Pulls every value into the range the editor uses.
    pub fn clamp(&mut self) {
        let n = &mut self.navigation;
        n.wheel_zoom_speed = finite(n.wheel_zoom_speed, 1.0).clamp(0.1, 8.0);
        n.pan_speed = finite(n.pan_speed, 1.0).clamp(0.1, 8.0);
        n.key_pan_speed = finite(n.key_pan_speed, 900.0).clamp(50.0, 10000.0);
        n.key_pan_fast = finite(n.key_pan_fast, 3.0).clamp(1.0, 20.0);
        let p = &mut self.pen;
        p.curve = finite(p.curve, 1.0).clamp(0.1, 10.0);
        p.min_size = p.min_size.min(400);
        p.max_size = p.max_size.clamp(1, 400);
        let c = &mut self.canvas;
        c.checker_size = c.checker_size.clamp(1, 128);
        c.grid_from_zoom = finite(c.grid_from_zoom, 800.0).clamp(100.0, 6400.0);
        c.custom_grid = [
            c.custom_grid[0].clamp(1, 4096),
            c.custom_grid[1].clamp(1, 4096),
        ];
        for color in [
            &mut c.checker_a,
            &mut c.checker_b,
            &mut c.grid_color,
            &mut c.background,
        ] {
            if color
                .as_deref()
                .is_some_and(|h| crate::view::parse_hex(h).is_none())
            {
                *color = None;
            }
        }
        let i = &mut self.interface;
        if !UI_SCALES.contains(&i.scale) {
            let asked = i.scale;
            i.scale = UI_SCALES
                .into_iter()
                .min_by_key(|s| s.abs_diff(asked))
                .unwrap_or(100);
        }
        let f = &mut self.files;
        f.autosave_seconds = f.autosave_seconds.min(3600);
        f.recent_count = f.recent_count.clamp(1, 200);
        let perf = &mut self.performance;
        perf.progressive_tiles = perf.progressive_tiles.clamp(1, 4096);
        perf.worker_threads = perf.worker_threads.min(256);
        perf.buffer_budget_mib = perf.buffer_budget_mib.clamp(64, 1 << 20);
    }

    /// The look these settings give the window.
    pub fn look(&self) -> Look {
        Look {
            theme: self.interface.theme,
            plain_font: self.interface.font == UiFont::Plain,
            scale: self.interface.scale,
            tooltips: self.interface.tooltips,
        }
    }

    /// The autosave interval, or `None` when recovery copies are off.
    pub fn autosave_every(&self) -> Option<Duration> {
        (self.files.recovery && self.files.autosave_seconds > 0)
            .then(|| Duration::from_secs(u64::from(self.files.autosave_seconds)))
    }
}

fn finite(v: f64, fallback: f64) -> f64 {
    if v.is_finite() { v } else { fallback }
}

/// Where the settings are kept: `settings.json` in a [`Store`], which
/// keeps nothing for a store without a folder.
#[derive(Resource, Default)]
pub struct SettingsStore {
    store: Store,
    /// The settings as last written.
    saved: Option<Settings>,
    /// When a change was first seen unsaved.
    dirty_since: Option<Instant>,
}

impl SettingsStore {
    /// The user's settings, in pixl's data folder.
    pub fn user() -> (Settings, Self) {
        Self::load(Store::user())
    }

    /// The settings `store` holds, defaults when it holds none.
    pub fn load(store: Store) -> (Settings, Self) {
        let settings = store
            .path(SETTINGS_FILE)
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| Settings::from_json(&b))
            .unwrap_or_default();
        let kept = Self {
            store,
            saved: Some(settings.clone()),
            dirty_since: None,
        };
        (settings, kept)
    }

    /// The settings file, when the store has a folder.
    pub fn path(&self) -> Option<PathBuf> {
        self.store.path(SETTINGS_FILE)
    }

    /// Writes `settings` when they changed and have settled. Returns when
    /// to look again, if a write is waiting.
    pub fn save(&mut self, settings: &Settings, now: Instant) -> Option<Instant> {
        if self.saved.as_ref() == Some(settings) {
            self.dirty_since = None;
            return None;
        }
        let since = *self.dirty_since.get_or_insert(now);
        if now.saturating_duration_since(since) < SAVE_AFTER {
            return Some(since + SAVE_AFTER);
        }
        let _ = self.store.save(SETTINGS_FILE, settings);
        self.saved = Some(settings.clone());
        self.dirty_since = None;
        None
    }
}

/// Writes the settings file when the settings change.
pub fn save_settings(
    settings: Res<Settings>,
    mut store: ResMut<SettingsStore>,
    wake: Option<Res<WakeDeadline>>,
) {
    if let Some(at) = store.save(&settings, Instant::now())
        && let Some(wake) = wake
    {
        wake.request(at);
    }
}

/// Which token set, UI font and scale the window uses, and whether
/// tooltips show.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub theme: Theme,
    pub plain_font: bool,
    /// The UI scale in percent, one of [`UI_SCALES`].
    pub scale: u32,
    pub tooltips: bool,
}

impl Default for Look {
    fn default() -> Self {
        Settings::default().look()
    }
}

impl Look {
    /// The root element's classes for this look, with `home` when the
    /// start screen shows.
    pub fn classes(self, home: bool) -> String {
        let mut classes = String::from("pixl");
        classes.push(' ');
        classes.push_str(self.theme.class());
        if self.plain_font {
            classes.push_str(" font-plain");
        }
        if self.scale != 100 {
            classes.push_str(&format!(" scale-{}", self.scale));
        }
        if !self.tooltips {
            classes.push_str(" no-tooltips");
        }
        if home {
            classes.push_str(" home");
        }
        classes
    }

    /// Puts the look on the window, and labels the two switches with what a
    /// click on each turns to.
    pub fn apply(self, home: bool) {
        if let Some(root) = dom::get_by_id("app") {
            root.set_class(&self.classes(home));
        }
        if let Some(button) = dom::get_by_id("toggle-theme") {
            button.set_text(if self.theme == Theme::Light {
                "Dark"
            } else {
                "Light"
            });
        }
        if let Some(button) = dom::get_by_id("toggle-font") {
            button.set_text(if self.plain_font {
                "Pixel font"
            } else {
                "Plain font"
            });
        }
    }
}

/// Flips the theme or the font when one of the menu row's switches is
/// clicked.
pub fn look_switches(
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    mut settings: ResMut<Settings>,
) {
    for click in clicks.read() {
        let i = &mut settings.interface;
        match ids.of(click.entity).map(|id| id.0.as_str()) {
            Ok("toggle-theme") => {
                i.theme = match i.theme {
                    Theme::Light => Theme::Dark,
                    _ => Theme::Light,
                };
                i.user_theme = None;
            }
            Ok("toggle-font") => {
                i.font = match i.font {
                    UiFont::Pixel => UiFont::Plain,
                    UiFont::Plain => UiFont::Pixel,
                }
            }
            _ => {}
        }
    }
}

/// Puts the settings' [`Look`] on the window until the root element shows
/// it. DOM edits land on a later tick, so this checks rather than assumes.
pub fn show_look(
    settings: Res<Settings>,
    mut look: ResMut<Look>,
    home: Res<Home>,
    wake: Option<Res<WakeDeadline>>,
) {
    let wanted_look = settings.look();
    if *look != wanted_look {
        *look = wanted_look;
    }
    let Some(root) = dom::get_by_id("app") else {
        return;
    };
    let wanted = look.classes(home.open);
    let mut shown = root.classes();
    shown.sort();
    let mut expected: Vec<String> = wanted.split(' ').map(str::to_string).collect();
    expected.sort();
    if shown != expected {
        look.apply(home.open);
        crate::repaint_soon(&wake);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_json() {
        let mut s = Settings::default();
        s.navigation.invert_wheel = true;
        s.canvas.checker_a = Some("#ff0000".into());
        s.interface.scale = 150;
        s.shortcuts
            .overrides
            .insert("undo".into(), vec!["Ctrl+U".into()]);
        assert_eq!(Settings::from_json(&s.to_json()), s);
    }

    #[test]
    fn a_partial_or_broken_file_keeps_the_defaults_it_lacks() {
        let s = Settings::from_json(br#"{"pen": {"curve": 2.0}, "canvas": "nonsense"}"#);
        assert_eq!(s.pen.curve, 2.0);
        assert_eq!(s.canvas, CanvasSettings::default());
        assert_eq!(Settings::from_json(b"not json"), Settings::default());
    }

    #[test]
    fn values_out_of_range_are_pulled_in() {
        let s = Settings::from_json(
            br#"{"interface": {"scale": 130}, "canvas": {"checker_size": 0, "grid_color": "red"}}"#,
        );
        assert_eq!(s.interface.scale, 125);
        assert_eq!(s.canvas.checker_size, 1);
        assert_eq!(s.canvas.grid_color, None);
    }

    #[test]
    fn the_pressure_curve_maps_between_the_sizes() {
        let mut pen = PenSettings::default();
        assert_eq!(pen.scale(1.0), 1.0);
        assert_eq!(pen.scale(0.0), 0.0);
        pen.curve = 2.0;
        assert!((pen.scale(0.5) - 0.25).abs() < 1e-9);
        pen.min_size = 50;
        assert!((pen.scale(0.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_change_is_written_once_it_settles() {
        let dir = std::env::temp_dir().join(format!("pixl-settings-{}", std::process::id()));
        let path = dir.join(SETTINGS_FILE);
        let (mut s, mut store) = SettingsStore::load(Store::at(&dir));
        let t0 = Instant::now();
        assert_eq!(store.save(&s, t0), None);
        s.pen.pressure = false;
        assert!(store.save(&s, t0).is_some());
        assert!(!path.exists());
        assert_eq!(store.save(&s, t0 + SAVE_AFTER), None);
        let (back, _) = SettingsStore::load(Store::at(&dir));
        assert!(!back.pen.pressure);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_look_names_its_classes() {
        let look = Look {
            theme: Theme::Light,
            plain_font: true,
            scale: 150,
            tooltips: false,
        };
        assert_eq!(
            look.classes(false),
            "pixl light font-plain scale-150 no-tooltips"
        );
        assert_eq!(Look::default().classes(true), "pixl dark home");
    }
}
