//! Keyboard shortcuts: every command a key can run, the chords that run
//! them under each preset (pixl's own, Aseprite's and Photoshop's), the
//! user's own bindings over a preset, and the system that turns this tick's
//! key presses into commands for the rest of the editor to carry out.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use lumenui::ecs::change_detection::DetectChanges;
use lumenui::ecs::system::SystemParam;
use lumenui::lumen_core::input::{
    Key, MenuClicked, Modifiers, ModifiersState, NamedKey, WindowFocused,
};
use lumenui::prelude::*;
use serde::{Deserialize, Serialize};

use crate::editor::Tool;
use crate::settings::Settings;
use crate::tutorial::lessons::LESSONS;

/// One key with the modifiers held for it. `ctrl` stands for Ctrl, or
/// Command on macOS.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The key's name: a lowercase character, or a name such as `F3`,
    /// `Space` or `ArrowUp`.
    pub key: String,
}

impl Chord {
    /// Reads `Ctrl+Shift+Z`, `F3` or `Space`. Modifier names are not case
    /// sensitive; a single-character key is lowercased.
    pub fn parse(text: &str) -> Option<Chord> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        // A chord ending in "+" names the plus key itself.
        let (mods, key) = match text.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None if text == "+" => ("", "+"),
            None => match text.rsplit_once('+') {
                Some((m, k)) => (m, k),
                None => ("", text),
            },
        };
        let mut chord = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            key: normal_key(key)?,
        };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" | "super" => chord.ctrl = true,
                "alt" | "option" => chord.alt = true,
                "shift" => chord.shift = true,
                _ => return None,
            }
        }
        Some(chord)
    }

    /// The chord a key press makes, or `None` for a key that is only a
    /// modifier.
    pub fn of_press(key: &Key, held: Modifiers) -> Option<Chord> {
        let name = key_name(key)?;
        Some(Chord {
            ctrl: held.ctrl || held.super_,
            alt: held.alt,
            shift: held.shift,
            key: name,
        })
    }

    /// Whether a press of this chord's key with `held` runs it. A letter
    /// needs Shift to match; a symbol already carries it (`+` is Shift and
    /// `=` on most layouts), so Shift is not asked of it.
    pub fn matches(&self, press: &Chord) -> bool {
        if self.key != press.key || self.ctrl != press.ctrl || self.alt != press.alt {
            return false;
        }
        let letter = self.key.chars().count() == 1
            && self.key.chars().next().is_some_and(char::is_alphanumeric);
        let named = self.key.chars().count() > 1;
        if letter || named {
            self.shift == press.shift
        } else {
            !self.shift || press.shift
        }
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.key.chars().count() == 1 {
            write!(f, "{}", self.key.to_uppercase())
        } else {
            f.write_str(&self.key)
        }
    }
}

/// A key's name as chords write it.
fn normal_key(key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if key.chars().count() == 1 {
        return Some(key.to_lowercase());
    }
    // Names are matched as the window writes them; accept any case here.
    let known = [
        "Tab",
        "Enter",
        "Escape",
        "Backspace",
        "Space",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "Home",
        "End",
        "Delete",
        "PageUp",
        "PageDown",
        "Insert",
    ];
    if let Some(k) = known.iter().find(|k| k.eq_ignore_ascii_case(key)) {
        return Some((*k).to_string());
    }
    let upper = key.to_ascii_uppercase();
    if let Some(n) = upper.strip_prefix('F')
        && n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n))
    {
        return Some(upper);
    }
    None
}

/// The name of a pressed key, or `None` for a modifier key alone.
pub fn key_name(key: &Key) -> Option<String> {
    match key {
        Key::Named(n) => Some(
            match n {
                NamedKey::Tab => "Tab",
                NamedKey::Enter => "Enter",
                NamedKey::Escape => "Escape",
                NamedKey::Backspace => "Backspace",
                NamedKey::Space => "Space",
                NamedKey::ArrowUp => "ArrowUp",
                NamedKey::ArrowDown => "ArrowDown",
                NamedKey::ArrowLeft => "ArrowLeft",
                NamedKey::ArrowRight => "ArrowRight",
                NamedKey::Home => "Home",
                NamedKey::End => "End",
                NamedKey::Delete => "Delete",
            }
            .to_string(),
        ),
        Key::Character(c) => {
            if matches!(
                c.as_str(),
                "Shift" | "Control" | "Alt" | "Meta" | "Super" | "AltGraph" | "CapsLock"
            ) {
                return None;
            }
            let mut chars = c.chars();
            match (chars.next(), chars.next()) {
                // With Ctrl held a platform may report the control
                // character instead of the letter.
                (Some(ch), None) if (ch as u32) >= 1 && (ch as u32) <= 26 => {
                    char::from_u32(ch as u32 + 0x60).map(String::from)
                }
                (Some(_), None) => Some(c.to_lowercase()),
                (Some(_), Some(_)) => normal_key(c).or_else(|| Some(c.clone())),
                (None, _) => None,
            }
        }
    }
}

/// Whether a command runs once per press or for as long as its key is down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Press,
    Hold,
}

/// A command a key can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// The command's id. A command that is also a menu item or a tool bar
    /// button shares that element's id.
    pub id: String,
    pub label: String,
    /// The section the shortcut list shows it under.
    pub group: &'static str,
    pub kind: Kind,
}

/// The commands that are not tools, with their group and kind.
const COMMANDS: &[(&str, &str, &str, Kind)] = &[
    ("file-new", "New project", "File", Kind::Press),
    ("file-open", "Open", "File", Kind::Press),
    ("file-save", "Save", "File", Kind::Press),
    ("file-save-as", "Save as", "File", Kind::Press),
    ("file-export-png", "Export PNG", "File", Kind::Press),
    ("file-export-gif", "Export GIF", "File", Kind::Press),
    (
        "file-export-sheet",
        "Export sprite sheet",
        "File",
        Kind::Press,
    ),
    (
        "file-import-sheet",
        "Import sprite sheet",
        "File",
        Kind::Press,
    ),
    ("file-export-presets", "Export presets", "File", Kind::Press),
    ("file-export-all", "Export all presets", "File", Kind::Press),
    ("file-export-tileset", "Export tileset", "File", Kind::Press),
    ("file-close", "Close", "File", Kind::Press),
    ("tab-next", "Next document", "File", Kind::Press),
    ("tab-prev", "Previous document", "File", Kind::Press),
    ("undo", "Undo", "Edit", Kind::Press),
    ("redo", "Redo", "Edit", Kind::Press),
    ("edit-cut", "Cut", "Edit", Kind::Press),
    ("edit-copy", "Copy", "Edit", Kind::Press),
    ("edit-copy-merged", "Copy merged", "Edit", Kind::Press),
    ("edit-paste", "Paste", "Edit", Kind::Press),
    (
        "edit-paste-layer",
        "Paste as new layer",
        "Edit",
        Kind::Press,
    ),
    (
        "edit-paste-document",
        "Paste as new document",
        "Edit",
        Kind::Press,
    ),
    ("swap-colors", "Swap colors", "Edit", Kind::Press),
    ("settings", "Settings", "Edit", Kind::Press),
    ("zoom-in", "Zoom in", "View", Kind::Press),
    ("zoom-out", "Zoom out", "View", Kind::Press),
    ("zoom-fit", "Fit to window", "View", Kind::Press),
    ("zoom-100", "Actual size", "View", Kind::Press),
    ("toggle-perf", "Performance overlay", "View", Kind::Press),
    ("command-palette", "Command palette", "Window", Kind::Press),
    (
        "window-workspace",
        "Panels and layouts",
        "Window",
        Kind::Press,
    ),
    ("window-layout-paint", "Paint layout", "Window", Kind::Press),
    (
        "window-layout-animate",
        "Animate layout",
        "Window",
        Kind::Press,
    ),
    ("window-layout-rig", "Rig layout", "Window", Kind::Press),
    ("window-layout-tiles", "Tiles layout", "Window", Kind::Press),
    ("window-layout-reset", "Reset layout", "Window", Kind::Press),
    ("window-show-all", "Show all panels", "Window", Kind::Press),
    ("window-themes", "Themes", "Window", Kind::Press),
    (
        "window-toolbar",
        "Customize the tool bar",
        "Window",
        Kind::Press,
    ),
    ("play", "Play or pause", "Animation", Kind::Press),
    ("frame-prev", "Previous frame", "Animation", Kind::Press),
    ("frame-next", "Next frame", "Animation", Kind::Press),
    ("frame-first", "First frame", "Animation", Kind::Press),
    ("frame-last", "Last frame", "Animation", Kind::Press),
    ("frame-new", "New frame", "Animation", Kind::Press),
    ("layer-above", "Layer above", "Animation", Kind::Press),
    ("layer-below", "Layer below", "Animation", Kind::Press),
    ("pan-drag", "Pan while dragging", "View", Kind::Hold),
    ("pan-up", "Move view up", "View", Kind::Hold),
    ("pan-left", "Move view left", "View", Kind::Hold),
    ("pan-down", "Move view down", "View", Kind::Hold),
    ("pan-right", "Move view right", "View", Kind::Hold),
    ("rig-mode-bones", "Bones mode", "Rig", Kind::Press),
    ("rig-mode-pose", "Pose mode", "Rig", Kind::Press),
    (
        "apply",
        "Finish the shape, or apply the transform",
        "Tools",
        Kind::Press,
    ),
    (
        "cancel",
        "Drop the shape or the transform, or leave the rig's mode",
        "Tools",
        Kind::Press,
    ),
    ("select-all", "Select all", "Select", Kind::Press),
    ("select-none", "Deselect", "Select", Kind::Press),
    ("select-invert", "Invert selection", "Select", Kind::Press),
    ("select-color", "Select by color", "Select", Kind::Press),
    (
        "select-layer",
        "Select layer content",
        "Select",
        Kind::Press,
    ),
    ("select-grow", "Grow selection", "Select", Kind::Press),
    ("select-shrink", "Shrink selection", "Select", Kind::Press),
    ("select-border", "Border selection", "Select", Kind::Press),
    ("select-save", "Save selection", "Select", Kind::Press),
    ("select-load", "Saved selections", "Select", Kind::Press),
    ("image-crop", "Crop to selection", "Image", Kind::Press),
    ("image-canvas-size", "Canvas size", "Image", Kind::Press),
    ("image-resize", "Image size", "Image", Kind::Press),
    (
        "image-flip-h",
        "Flip canvas horizontally",
        "Image",
        Kind::Press,
    ),
    (
        "image-flip-v",
        "Flip canvas vertically",
        "Image",
        Kind::Press,
    ),
    (
        "image-rotate-cw",
        "Rotate canvas clockwise",
        "Image",
        Kind::Press,
    ),
    (
        "image-rotate-ccw",
        "Rotate canvas counterclockwise",
        "Image",
        Kind::Press,
    ),
    ("image-indexed", "Indexed color", "Image", Kind::Press),
    ("image-rgba", "RGBA color", "Image", Kind::Press),
    ("image-replace-color", "Replace color", "Image", Kind::Press),
    ("palette-load", "Load palette", "Palette", Kind::Press),
    ("palette-save", "Save palette", "Palette", Kind::Press),
    ("palette-sort", "Sort palette", "Palette", Kind::Press),
    ("palette-ramp", "Ramp builder", "Palette", Kind::Press),
    (
        "palette-from-image",
        "Palette from image",
        "Palette",
        Kind::Press,
    ),
    (
        "palette-lock",
        "Lock colors to the palette",
        "Palette",
        Kind::Press,
    ),
    ("filter-hue", "Hue and saturation", "Filter", Kind::Press),
    (
        "filter-brightness",
        "Brightness and contrast",
        "Filter",
        Kind::Press,
    ),
    ("filter-levels", "Levels", "Filter", Kind::Press),
    ("filter-curves", "Curves", "Filter", Kind::Press),
    ("filter-balance", "Color balance", "Filter", Kind::Press),
    ("filter-posterize", "Posterize", "Filter", Kind::Press),
    ("filter-invert", "Invert colors", "Filter", Kind::Press),
    ("filter-desaturate", "Desaturate", "Filter", Kind::Press),
    ("filter-gradient-map", "Gradient map", "Filter", Kind::Press),
    ("filter-outline", "Outline", "Filter", Kind::Press),
    ("filter-shadow", "Drop shadow", "Filter", Kind::Press),
    ("filter-noise", "Noise", "Filter", Kind::Press),
    ("filter-pixelate", "Pixelate", "Filter", Kind::Press),
    ("view-rulers", "Rulers", "View", Kind::Press),
    ("view-guides", "Guides", "View", Kind::Press),
    ("view-clear-guides", "Clear guides", "View", Kind::Press),
    ("view-snap", "Snap", "View", Kind::Press),
    ("view-pixel-grid", "Pixel grid", "View", Kind::Press),
    ("view-grid", "Grid", "View", Kind::Press),
    ("view-grid-settings", "Grid settings", "View", Kind::Press),
    ("view-slices", "Show slices", "View", Kind::Press),
    ("view-tile-x", "Tile horizontally", "View", Kind::Press),
    ("view-tile-y", "Tile vertically", "View", Kind::Press),
    ("view-rotate-ccw", "Rotate view left", "View", Kind::Press),
    ("view-rotate-cw", "Rotate view right", "View", Kind::Press),
    (
        "view-rotate-reset",
        "Reset view rotation",
        "View",
        Kind::Press,
    ),
    ("view-flip", "Flip view", "View", Kind::Press),
    ("tiles-mode", "Pixels or tiles", "Tools", Kind::Press),
    ("brush-smaller", "Smaller brush", "Tools", Kind::Press),
    ("brush-larger", "Larger brush", "Tools", Kind::Press),
    ("help-rig-demo", "Open rig demo", "Help", Kind::Press),
];

/// Every command a key can run: the tools, then the rest by group, with
/// the tutorials last in Help.
pub fn commands() -> Vec<Command> {
    let tools = Tool::ALL.into_iter().map(|t| Command {
        id: t.button_id().to_string(),
        label: t.label().to_string(),
        group: "Tools",
        kind: Kind::Press,
    });
    let lessons = LESSONS.into_iter().map(|l| Command {
        id: l.id.to_string(),
        label: l.title.to_string(),
        group: "Help",
        kind: Kind::Press,
    });
    tools
        .chain(COMMANDS.iter().map(|(id, label, group, kind)| Command {
            id: (*id).to_string(),
            label: (*label).to_string(),
            group,
            kind: *kind,
        }))
        .chain(lessons)
        .collect()
}

/// A set of default shortcuts.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    #[default]
    Pixl,
    Aseprite,
    Photoshop,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Pixl, Preset::Aseprite, Preset::Photoshop];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Pixl => "pixl",
            Preset::Aseprite => "Aseprite",
            Preset::Photoshop => "Photoshop",
        }
    }

    pub fn from_label(label: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.label() == label)
    }

    /// The preset's chords for `id`.
    pub fn chords(self, id: &str) -> Vec<Chord> {
        let pixl = pixl_default(id);
        let own: Option<&[&str]> = match self {
            Preset::Pixl => None,
            Preset::Aseprite => aseprite(id),
            Preset::Photoshop => photoshop(id),
        };
        own.unwrap_or(pixl)
            .iter()
            .filter_map(|c| Chord::parse(c))
            .collect()
    }
}

/// pixl's own keys. A tool's key is the one its tooltip names.
fn pixl_default(id: &str) -> &'static [&'static str] {
    match id {
        "tool-pencil" => &["B"],
        "tool-eraser" => &["E"],
        "tool-line" => &["L"],
        "tool-rect" => &["U"],
        "tool-ellipse" => &["Shift+U"],
        "tool-bucket" => &["G"],
        "tool-eyedropper" => &["I"],
        "tool-select" => &["M"],
        "tool-move" => &["V"],
        "tool-brush" => &["Shift+B"],
        "tool-spray" => &["J"],
        "tool-shade" => &["O"],
        "tool-dither" => &["Shift+O"],
        "tool-clone" => &["K"],
        "tool-pattern" => &["Shift+K"],
        "tool-smudge" => &["R"],
        "tool-blur" => &["Shift+R"],
        "tool-replace" => &["Y"],
        "tool-curve" => &["Shift+L"],
        "tool-polygon" => &["H"],
        "tool-contour" => &["Shift+P"],
        "tool-gradient" => &["Shift+G"],
        "tool-ellipse-select" => &["Shift+M"],
        "tool-lasso" => &["Q"],
        "tool-polylasso" => &["Shift+Q"],
        "tool-wand" => &["N"],
        "tool-transform" => &["Ctrl+T"],
        "tool-crop" => &["C"],
        "tool-text" => &["T"],
        "tool-measure" => &["Shift+C"],
        "tool-slice" => &["F"],
        "file-new" => &["Ctrl+N"],
        "file-open" => &["Ctrl+O"],
        "file-save" => &["Ctrl+S"],
        "file-save-as" => &["Ctrl+Shift+S"],
        "file-export-png" => &["Ctrl+E"],
        "file-export-sheet" => &["Ctrl+Shift+E"],
        "file-import-sheet" => &["Ctrl+Shift+I"],
        "file-export-all" => &["Ctrl+Alt+E"],
        "file-close" => &["Ctrl+W"],
        "tab-next" => &["Ctrl+Tab"],
        "tab-prev" => &["Ctrl+Shift+Tab"],
        "undo" => &["Ctrl+Z"],
        "redo" => &["Ctrl+Shift+Z", "Ctrl+Y"],
        "edit-cut" => &["Ctrl+X"],
        "edit-copy" => &["Ctrl+C"],
        "edit-copy-merged" => &["Ctrl+Shift+C"],
        "edit-paste" => &["Ctrl+V"],
        "edit-paste-layer" => &["Ctrl+Shift+V"],
        "edit-paste-document" => &["Ctrl+Alt+V"],
        "swap-colors" => &["X"],
        "command-palette" => &["Ctrl+Shift+P"],
        "settings" => &["Ctrl+,"],
        "zoom-in" => &["+", "="],
        "zoom-out" => &["-"],
        "zoom-fit" => &["0"],
        "zoom-100" => &["1"],
        "toggle-perf" => &["F3"],
        "play" => &["P"],
        "frame-prev" => &[",", "Alt+ArrowLeft"],
        "frame-next" => &[".", "Alt+ArrowRight"],
        "frame-first" => &["Home"],
        "frame-last" => &["End"],
        "frame-new" => &["Alt+N"],
        "layer-above" => &["Alt+ArrowUp"],
        "layer-below" => &["Alt+ArrowDown"],
        "select-all" => &["Ctrl+A"],
        "select-none" => &["Ctrl+D", "Escape"],
        "select-invert" => &["Ctrl+Alt+I"],
        "filter-hue" => &["Ctrl+U"],
        "filter-levels" => &["Ctrl+L"],
        "filter-curves" => &["Ctrl+M"],
        "filter-balance" => &["Ctrl+B"],
        "filter-invert" => &["Ctrl+I"],
        "filter-desaturate" => &["Ctrl+Shift+U"],
        "view-rulers" => &["Ctrl+R"],
        "view-guides" => &["Ctrl+;"],
        "view-snap" => &["Ctrl+Shift+;"],
        // Ctrl+Shift+' as Aseprite has it: the window names the key a
        // shifted press makes, which is " on a US layout.
        "view-pixel-grid" => &["Ctrl+\""],
        "view-grid" => &["Ctrl+'"],
        "view-rotate-ccw" => &["Alt+["],
        "view-rotate-cw" => &["Alt+]"],
        "view-flip" => &["Alt+H"],
        "tiles-mode" => &["Tab"],
        "brush-smaller" => &["["],
        "brush-larger" => &["]"],
        "pan-drag" => &["Space"],
        "pan-up" => &["W"],
        "pan-left" => &["A"],
        "pan-down" => &["S"],
        "pan-right" => &["D"],
        "apply" => &["Enter"],
        "cancel" => &["Escape"],
        _ => &[],
    }
}

/// Where Aseprite's default keys differ from pixl's.
fn aseprite(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        "file-export-png" => &["Ctrl+Alt+Shift+S"],
        "file-export-sheet" => &["Ctrl+E"],
        "file-import-sheet" => &["Ctrl+I"],
        "redo" => &["Ctrl+Y", "Ctrl+Shift+Z"],
        // Aseprite's Paste Special > Paste as New Layer; its New Sprite
        // from the clipboard has no key, so pixl's stays.
        "edit-paste-layer" => &["Ctrl+Shift+V"],
        "settings" => &["Ctrl+K"],
        "zoom-fit" => &["Ctrl+0"],
        "play" => &["Enter"],
        // Aseprite's Ctrl+I imports a sprite sheet.
        "filter-invert" => &[],
        _ => return None,
    })
}

/// Where Photoshop's default keys differ from pixl's.
fn photoshop(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        // Photoshop keeps its line with the shapes under U.
        "tool-line" => &[],
        "file-export-png" => &["Ctrl+Alt+Shift+W"],
        "file-export-sheet" => &[],
        "file-import-sheet" => &[],
        "redo" => &["Ctrl+Shift+Z"],
        // Photoshop's Paste in Place, which pastes onto a new layer where
        // the pixels came from; its Paste Into (Ctrl+Alt+Shift+V) keeps
        // Ctrl+Alt+V free for a new document.
        "edit-paste-layer" => &["Ctrl+Shift+V"],
        "settings" => &["Ctrl+K"],
        "zoom-in" => &["Ctrl+=", "Ctrl++"],
        "zoom-out" => &["Ctrl+-"],
        "zoom-fit" => &["Ctrl+0"],
        "zoom-100" => &["Ctrl+1"],
        _ => return None,
    })
}

/// The chords in force: a preset with the user's changes over it.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    pub preset: Preset,
    /// Per command id, the chords that run it.
    pub bindings: BTreeMap<String, Vec<Chord>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::from_settings(&Settings::default())
    }
}

impl Keymap {
    /// The keymap the shortcut settings describe.
    pub fn from_settings(settings: &Settings) -> Self {
        let s = &settings.shortcuts;
        let bindings = commands()
            .into_iter()
            .map(|c| {
                let chords = match s.overrides.get(&c.id) {
                    Some(list) => list.iter().filter_map(|t| Chord::parse(t)).collect(),
                    None => s.preset.chords(&c.id),
                };
                (c.id, chords)
            })
            .collect();
        Self {
            preset: s.preset,
            bindings,
        }
    }

    pub fn chords(&self, id: &str) -> &[Chord] {
        self.bindings.get(id).map_or(&[], Vec::as_slice)
    }

    /// The commands a press runs.
    pub fn commands_for(&self, press: &Chord) -> Vec<&str> {
        self.bindings
            .iter()
            .filter(|(_, chords)| chords.iter().any(|c| c.matches(press)))
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// The other commands `chord` already runs.
    pub fn conflicts(&self, id: &str, chord: &Chord) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|(other, chords)| {
                other.as_str() != id && chords.contains(chord) && !shares(id, other)
            })
            .map(|(other, _)| other.clone())
            .collect()
    }

    /// Every chord two or more commands share.
    pub fn clashes(&self) -> BTreeMap<Chord, Vec<String>> {
        let mut by_chord: BTreeMap<Chord, Vec<String>> = BTreeMap::new();
        for (id, chords) in &self.bindings {
            for c in chords {
                by_chord.entry(c.clone()).or_default().push(id.clone());
            }
        }
        // A tool's key shares its chord with one command outside the tool.
        by_chord.retain(|_, ids| {
            ids.iter()
                .enumerate()
                .any(|(i, a)| ids[i + 1..].iter().any(|b| !shares(a, b)))
        });
        by_chord
    }

    /// The text a command's chords show as: `Ctrl+Shift+Z, Ctrl+Y`.
    pub fn describe(&self, id: &str) -> String {
        let chords = self.chords(id);
        if chords.is_empty() {
            return "none".to_string();
        }
        chords
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Binds `chord` to `id` alone, taking it from any command that had it.
/// Writes the change into the settings and returns the commands it took it
/// from.
pub fn rebind(settings: &mut Settings, id: &str, chord: Chord) -> Vec<String> {
    let map = Keymap::from_settings(settings);
    let taken = map.conflicts(id, &chord);
    for other in &taken {
        let left: Vec<String> = map
            .chords(other)
            .iter()
            .filter(|c| **c != chord)
            .map(ToString::to_string)
            .collect();
        settings.shortcuts.overrides.insert(other.clone(), left);
    }
    settings
        .shortcuts
        .overrides
        .insert(id.to_string(), vec![chord.to_string()]);
    tidy(settings);
    taken
}

/// Leaves `id` without a key.
pub fn unbind(settings: &mut Settings, id: &str) {
    settings
        .shortcuts
        .overrides
        .insert(id.to_string(), Vec::new());
    tidy(settings);
}

/// Puts `id` back on its preset's keys.
pub fn reset_one(settings: &mut Settings, id: &str) {
    settings.shortcuts.overrides.remove(id);
}

/// Drops overrides that say what the preset already says.
fn tidy(settings: &mut Settings) {
    let preset = settings.shortcuts.preset;
    settings.shortcuts.overrides.retain(|id, list| {
        let chords: Vec<Chord> = list.iter().filter_map(|t| Chord::parse(t)).collect();
        chords != preset.chords(id)
    });
}

/// The commands a tool in use answers: while a shape, a transform or a rig
/// mode is under way its key runs only them, and otherwise never them, so
/// they share their chord with another command (Enter plays the animation
/// under the Aseprite keys, and finishes a polygon while one is drawn).
pub const TOOL_KEYS: &[&str] = &["apply", "cancel"];

/// Whether two commands may share a chord: one answers a tool in use and
/// the other does not.
fn shares(a: &str, b: &str) -> bool {
    TOOL_KEYS.contains(&a) != TOOL_KEYS.contains(&b)
}

/// The commands this tick's keys ran, and the hold commands whose key is
/// down.
#[derive(Resource, Default, Debug)]
pub struct KeyCommands {
    /// Press commands, in order, each with whether it came from a key
    /// repeat.
    pub pressed: Vec<(String, bool)>,
    /// Hold commands whose key is down.
    pub held: BTreeSet<String>,
    /// While set, the next key press is a chord for the shortcut list and
    /// runs nothing.
    pub capture: bool,
    /// The chord caught while capturing.
    pub captured: Option<Chord>,
}

impl KeyCommands {
    /// Whether `id` was pressed this tick (repeats included when `repeat`).
    pub fn ran(&self, id: &str, repeat: bool) -> bool {
        self.pressed.iter().any(|(c, r)| c == id && (repeat || !r))
    }

    pub fn holds(&self, id: &str) -> bool {
        self.held.contains(id)
    }
}

/// The key messages and what decides whether keys run commands.
#[derive(SystemParam)]
pub struct KeySources<'w, 's> {
    menu: MessageReader<'w, 's, MenuClicked>,
    pressed: MessageReader<'w, 's, KeyPressed>,
    released: MessageReader<'w, 's, KeyReleased>,
    focus: MessageReader<'w, 's, WindowFocused>,
    typing: Query<'w, 's, (), (With<Focused>, With<TextInput>)>,
    modifiers: Res<'w, ModifiersState>,
    toolbox: Option<Res<'w, crate::tools::toolbox::Toolbox>>,
    transform: Option<Res<'w, crate::tools::transform::FreeTransform>>,
    rig: Option<Res<'w, crate::rig::RigUi>>,
}

impl KeySources<'_, '_> {
    /// Whether a tool is in the middle of something its keys answer.
    fn tool_busy(&self) -> bool {
        self.toolbox.as_ref().is_some_and(|t| t.active())
            || self.transform.as_ref().is_some_and(|t| t.active())
            || self.rig.as_ref().is_some_and(|r| r.mode.is_some())
    }
}

/// The commands their menu item runs through the keymap's queue.
const MENU_RUNS: &[&str] = &["undo", "redo"];

/// The dialogs that keep the editor's keys while open.
const MODAL_SIGNALS: &[&str] = &[
    "settings_open",
    "new_open",
    "discard_open",
    "recover_open",
    "sheet_export_open",
    "sheet_import_open",
    "ws_open",
    "themes_open",
    "cp_open",
    "ex_open",
    "tp_open",
    "tb_open",
    "filter_open",
    "amount_open",
    "canvas_open",
    "resize_open",
    "indexed_open",
    "replace_open",
    "ramp_open",
    "sort_open",
    "quantize_open",
    "grid_open",
    "slice_open",
    "selections_open",
];

/// Turns this tick's key presses into commands, through the keymap the
/// settings describe.
pub fn read_keys(
    settings: Res<Settings>,
    mut keymap: ResMut<Keymap>,
    mut out: ResMut<KeyCommands>,
    mut keys: KeySources,
    signals: Signals,
) {
    if settings.is_changed() {
        let next = Keymap::from_settings(&settings);
        if *keymap != next {
            *keymap = next;
        }
    }
    out.pressed.clear();
    // A menu item named for a command no other system reads the menu for
    // runs it as its key would.
    for m in keys.menu.read() {
        if MENU_RUNS.contains(&m.id.as_str()) {
            out.pressed.push((m.id.clone(), false));
        }
    }
    let typing = !keys.typing.is_empty();
    let modal = MODAL_SIGNALS
        .iter()
        .any(|s| signals.get::<bool>(s).unwrap_or(false));
    if typing || keys.focus.read().any(|f| !f.focused) {
        // Keys that go to a text field or another window no longer hold:
        // their release may never arrive here.
        out.held.clear();
    }
    let held_now = keys.modifiers.0;
    let presses: Vec<(Key, Modifiers, bool)> = keys
        .pressed
        .read()
        .map(|k| (k.key.clone(), k.modifiers, k.repeat))
        .collect();
    for (key, m, repeat) in presses {
        let mods = Modifiers {
            shift: m.shift || held_now.shift,
            ctrl: m.ctrl || held_now.ctrl,
            alt: m.alt || held_now.alt,
            super_: m.super_ || held_now.super_,
        };
        let Some(press) = Chord::of_press(&key, mods) else {
            continue;
        };
        if out.capture {
            if !repeat {
                out.captured = Some(press);
                out.capture = false;
            }
            continue;
        }
        if typing {
            continue;
        }
        let mut ids: Vec<String> = keymap
            .commands_for(&press)
            .into_iter()
            .map(str::to_string)
            .collect();
        // A key a tool answers goes to the tool while it is busy, and to
        // the other command of its chord the rest of the time.
        if ids.iter().any(|id| TOOL_KEYS.contains(&id.as_str())) {
            let busy = keys.tool_busy();
            ids.retain(|id| TOOL_KEYS.contains(&id.as_str()) == busy);
        }
        for id in ids.into_iter().filter(|id| !is_hold(id)) {
            // The settings shortcut also closes the dialog it opens.
            if !modal || id == "settings" {
                out.pressed.push((id, repeat));
            }
        }
        // A hold key holds with Shift down (W, A, S and D move faster then)
        // or up, never with Ctrl or Alt, which make it a shortcut.
        if !press.ctrl && !press.alt {
            for (id, chords) in &keymap.bindings {
                let hold = chords
                    .iter()
                    .any(|c| c.key == press.key && !c.ctrl && !c.alt);
                if hold && !modal && is_hold(id) {
                    out.held.insert(id.clone());
                }
            }
        }
    }
    for k in keys.released.read() {
        let Some(name) = key_name(&k.key) else {
            continue;
        };
        let let_go: Vec<String> = out
            .held
            .iter()
            .filter(|id| keymap.chords(id).iter().any(|c| c.key == name))
            .cloned()
            .collect();
        for id in let_go {
            out.held.remove(&id);
        }
    }
}

fn is_hold(id: &str) -> bool {
    COMMANDS
        .iter()
        .any(|(c, _, _, kind)| *c == id && *kind == Kind::Hold)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(t: &str) -> Chord {
        Chord::parse(t).unwrap()
    }

    #[test]
    fn chords_read_and_write_back() {
        for t in ["Ctrl+Shift+Z", "F3", "Space", "X", "Ctrl+,", "+", "Ctrl++"] {
            assert_eq!(chord(t).to_string(), t);
        }
        assert_eq!(chord("ctrl+shift+z"), chord("Ctrl+Shift+Z"));
        assert_eq!(Chord::parse("Hyper+Q"), None);
        assert_eq!(Chord::parse(""), None);
    }

    #[test]
    fn a_letter_needs_its_shift_and_a_symbol_does_not() {
        let press = |key: &str, shift: bool| {
            Chord::of_press(
                &Key::from(key),
                Modifiers {
                    shift,
                    ..Modifiers::default()
                },
            )
            .unwrap()
        };
        assert!(chord("U").matches(&press("u", false)));
        assert!(!chord("U").matches(&press("u", true)));
        assert!(chord("Shift+U").matches(&press("U", true)));
        assert!(chord("+").matches(&press("+", true)));
        assert!(chord("F3").matches(&press("F3", false)));
        // A control character is the letter it stands for.
        let ctrl_z = Chord::of_press(
            &Key::Character("\x1a".into()),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        )
        .unwrap();
        assert!(chord("Ctrl+Z").matches(&ctrl_z));
        assert_eq!(
            Chord::of_press(&Key::from("Shift"), Modifiers::default()),
            None
        );
    }

    #[test]
    fn every_command_has_a_label_and_the_presets_have_no_clashes() {
        let ids: Vec<String> = commands().into_iter().map(|c| c.id).collect();
        let unique: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
        for preset in Preset::ALL {
            let mut s = Settings::default();
            s.shortcuts.preset = preset;
            let map = Keymap::from_settings(&s);
            assert!(map.clashes().is_empty(), "{preset:?}: {:?}", map.clashes());
        }
    }

    #[test]
    fn the_presets_differ_where_the_apps_do() {
        let mut s = Settings::default();
        assert_eq!(
            Keymap::from_settings(&s).describe("redo"),
            "Ctrl+Shift+Z, Ctrl+Y"
        );
        s.shortcuts.preset = Preset::Aseprite;
        let map = Keymap::from_settings(&s);
        assert_eq!(map.describe("file-export-sheet"), "Ctrl+E");
        assert_eq!(map.describe("settings"), "Ctrl+K");
        s.shortcuts.preset = Preset::Photoshop;
        let map = Keymap::from_settings(&s);
        assert_eq!(map.describe("zoom-100"), "Ctrl+1");
        assert_eq!(map.describe("tool-line"), "none");
    }

    #[test]
    fn rebinding_takes_the_chord_from_the_command_that_had_it() {
        let mut s = Settings::default();
        let taken = rebind(&mut s, "file-export-gif", chord("Ctrl+E"));
        assert_eq!(taken, vec!["file-export-png".to_string()]);
        let map = Keymap::from_settings(&s);
        assert_eq!(map.describe("file-export-gif"), "Ctrl+E");
        assert_eq!(map.describe("file-export-png"), "none");
        assert!(map.clashes().is_empty());
        reset_one(&mut s, "file-export-png");
        let map = Keymap::from_settings(&s);
        assert_eq!(map.clashes().len(), 1, "a reset can clash, and shows it");
        unbind(&mut s, "file-export-gif");
        assert!(Keymap::from_settings(&s).clashes().is_empty());
    }

    #[test]
    fn an_override_equal_to_the_preset_is_dropped() {
        let mut s = Settings::default();
        rebind(&mut s, "undo", chord("Ctrl+Z"));
        assert!(s.shortcuts.overrides.is_empty());
    }
}
