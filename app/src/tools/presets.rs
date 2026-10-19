//! Brushes and tool presets: the brush library (named brushes, each a
//! shape, a size, opacity, hardness, dithering, pixel-perfect, the
//! stabilizer and the dynamics; made from the current tool,
//! from the selection or from a PNG; shared as `.pixlbrushes` packs), named
//! presets of a tool's whole set of options, and the quick bar of the
//! colors and brushes painted with last. All of it is kept in pixl's data
//! folder: `brushes.json`, `tool-presets.json` and `quickbar.json`.

use std::path::Path;
use std::sync::Arc;

use lumenui::ecs::world::World;
use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_document::Rgba;
use pixl_document::native::write_atomic;
use pixl_io::png::decode_png;
use pixl_paint::Brush;
use pixl_paint::tools::BrushShape as CoreShape;
use serde::{Deserialize, Serialize};

use crate::editor::{BrushShape, Editor, Tool};
use crate::home::Home;
use crate::ids::ElementIds;
use crate::panels::escape;
use crate::settings::Settings;
use crate::settings::store::Store;
use crate::tools::dynamics::ToolDynamics;
use crate::tools::hex;
use crate::tools::toolbox::Toolbox;

/// The extension of a brush pack file.
pub const PACK_EXTENSION: &str = "pixlbrushes";

/// How many colors and brushes the quick bar keeps.
pub const QUICK_COLORS: usize = 10;
pub const QUICK_BRUSHES: usize = 5;

/// A brush's shape as files keep it: a custom mask is rows of `#` (set) and
/// `.` (clear).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Round,
    Square,
    Custom { rows: Vec<String> },
}

impl Shape {
    fn of(shape: &BrushShape) -> Shape {
        match shape {
            BrushShape::Round => Shape::Round,
            BrushShape::Square => Shape::Square,
            BrushShape::Custom(brush) => match &brush.shape {
                CoreShape::Custom {
                    width,
                    height: _,
                    mask,
                } => Shape::Custom {
                    rows: mask
                        .chunks(*width.max(&1) as usize)
                        .map(|row| row.iter().map(|&on| if on { '#' } else { '.' }).collect())
                        .collect(),
                },
                CoreShape::Square => Shape::Square,
                CoreShape::Circle => Shape::Round,
            },
        }
    }

    /// The editor's brush shape, or `None` for a mask with no pixel set.
    fn to_editor(&self) -> Option<BrushShape> {
        Some(match self {
            Shape::Round => BrushShape::Round,
            Shape::Square => BrushShape::Square,
            Shape::Custom { rows } => {
                let width = rows.iter().map(|r| r.chars().count()).max()? as u32;
                let height = rows.len() as u32;
                let mut mask = Vec::with_capacity((width * height) as usize);
                for row in rows {
                    let mut cells: Vec<bool> = row.chars().map(|c| c == '#').collect();
                    cells.resize(width as usize, false);
                    mask.extend(cells);
                }
                if !mask.iter().any(|&b| b) {
                    return None;
                }
                BrushShape::Custom(Brush {
                    shape: CoreShape::Custom {
                        width,
                        height,
                        mask: Arc::new(mask),
                    },
                    size: width.max(height),
                })
            }
        })
    }

    /// A custom shape from an image: pixels at least half opaque are in.
    pub fn from_image(width: u32, height: u32, pixels: &[Rgba]) -> Option<Shape> {
        if width == 0 || height == 0 || width > 256 || height > 256 {
            return None;
        }
        let rows: Vec<String> = pixels
            .chunks(width as usize)
            .map(|row| {
                row.iter()
                    .map(|p| if p.a >= 128 { '#' } else { '.' })
                    .collect()
            })
            .collect();
        rows.iter()
            .any(|r| r.contains('#'))
            .then_some(Shape::Custom { rows })
    }

    fn label(&self, size: u32) -> String {
        match self {
            Shape::Round => format!("round {size} px"),
            Shape::Square => format!("square {size} px"),
            Shape::Custom { rows } => format!(
                "custom {} x {}",
                rows.iter().map(|r| r.chars().count()).max().unwrap_or(0),
                rows.len()
            ),
        }
    }
}

/// How pen pressure shaped a brush in the files of an older pixl. It
/// reads as the brush's dynamics.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pressure {
    pub on: bool,
    pub curve: f64,
    pub min_size: u32,
    pub max_size: u32,
}

impl From<&Pressure> for ToolDynamics {
    fn from(p: &Pressure) -> ToolDynamics {
        let pen = serde_json::json!({
            "pressure": p.on,
            "curve": p.curve,
            "min_size": p.min_size,
            "max_size": p.max_size,
        });
        ToolDynamics::from_pen(&pen).unwrap_or_default()
    }
}

/// One brush of the library.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct BrushPreset {
    pub name: String,
    pub shape: Shape,
    pub size: u32,
    /// Percent, for the soft brush.
    pub opacity: u8,
    /// Percent, for the soft brush.
    pub hardness: u8,
    pub dithered: bool,
    pub pixel_perfect: bool,
    /// Stabilizer string length in pixels; 0 is off.
    pub stabilizer: u32,
    /// What pressure and velocity do to the brush; `None` leaves the tool's
    /// own dynamics.
    pub dynamics: Option<ToolDynamics>,
    /// An older file's pressure mapping, read as `dynamics` when the brush
    /// has none, and never written.
    #[serde(skip_serializing)]
    pub pressure: Option<Pressure>,
}

impl Default for BrushPreset {
    fn default() -> Self {
        Self {
            name: "Pixel".to_string(),
            shape: Shape::Square,
            size: 1,
            opacity: 100,
            hardness: 50,
            dithered: false,
            pixel_perfect: true,
            stabilizer: 0,
            dynamics: None,
            pressure: None,
        }
    }
}

impl BrushPreset {
    /// The brushes a new user starts with.
    pub fn starters() -> Vec<BrushPreset> {
        let b = BrushPreset::default;
        vec![
            b(),
            BrushPreset {
                name: "Round 4".into(),
                shape: Shape::Round,
                size: 4,
                pixel_perfect: false,
                ..b()
            },
            BrushPreset {
                name: "Square 3".into(),
                size: 3,
                pixel_perfect: false,
                ..b()
            },
            BrushPreset {
                name: "Soft 8".into(),
                shape: Shape::Round,
                size: 8,
                opacity: 60,
                hardness: 20,
                pixel_perfect: false,
                ..b()
            },
            BrushPreset {
                name: "Dithered 6".into(),
                shape: Shape::Round,
                size: 6,
                opacity: 50,
                dithered: true,
                pixel_perfect: false,
                ..b()
            },
            BrushPreset {
                name: "Steady ink".into(),
                size: 1,
                stabilizer: 12,
                ..b()
            },
        ]
    }

    /// The brush the editor paints with now, named `name`.
    pub fn capture(name: &str, editor: &Editor, toolbox: &Toolbox, settings: &Settings) -> Self {
        let o = &toolbox.options;
        BrushPreset {
            name: name.to_string(),
            shape: Shape::of(&editor.brush_shape),
            size: editor.options.size,
            opacity: o.opacity,
            hardness: o.hardness,
            dithered: o.dithered,
            pixel_perfect: editor.options.pixel_perfect,
            stabilizer: o.stabilizer,
            dynamics: Some(settings.dynamics(editor.tool)),
            pressure: None,
        }
    }

    /// The brush as this pixl keeps it: an older file's pressure mapping
    /// becomes its dynamics.
    fn upgraded(mut self) -> Self {
        if let Some(p) = self.pressure.take()
            && self.dynamics.is_none()
        {
            self.dynamics = Some(ToolDynamics::from(&p));
        }
        self
    }

    /// Makes this the brush the editor paints with.
    pub fn apply(&self, editor: &mut Editor, toolbox: &mut Toolbox, settings: &mut Settings) {
        if let Some(shape) = self.shape.to_editor() {
            editor.brush_shape = shape;
        }
        editor.options.size = self.size.clamp(1, 32);
        editor.options.pixel_perfect = self.pixel_perfect;
        let o = &mut toolbox.options;
        o.opacity = self.opacity.clamp(1, 100);
        o.hardness = self.hardness.min(100);
        o.dithered = self.dithered;
        o.stabilizer = self.stabilizer.min(64);
        // The dynamics go to the tool in use, when it has dynamics.
        if let Some(d) = &self.dynamics
            && crate::tools::dynamics::dynamic(editor.tool)
        {
            settings.set_dynamics(editor.tool, d.clone());
        }
    }

    /// What the library lists under the name.
    pub fn summary(&self) -> String {
        let mut parts = vec![self.shape.label(self.size)];
        if self.opacity < 100 {
            parts.push(format!("{}%", self.opacity));
        }
        if self.dithered {
            parts.push("dithered".into());
        }
        if self.pixel_perfect {
            parts.push("pixel-perfect".into());
        }
        if self.stabilizer > 0 {
            parts.push(format!("steady {}", self.stabilizer));
        }
        if let Some(d) = self.dynamics.as_ref().and_then(ToolDynamics::summary) {
            parts.push(d);
        }
        parts.join(", ")
    }
}

/// A named preset of one tool's options.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ToolPreset {
    pub name: String,
    /// The tool, by its button id.
    pub tool: String,
    pub brush: BrushPreset,
    pub filled: bool,
    pub tolerance: u8,
    pub contiguous: bool,
    pub spray_radius: u32,
    pub spray_density: u32,
    pub dither_amount: u8,
    pub strength: u8,
    pub keep_colors: bool,
    pub sharpen: bool,
    pub merged: bool,
    pub eight: bool,
}

impl Default for ToolPreset {
    fn default() -> Self {
        Self {
            name: String::new(),
            tool: Tool::Pencil.button_id().to_string(),
            brush: BrushPreset::default(),
            filled: false,
            tolerance: 0,
            contiguous: true,
            spray_radius: 8,
            spray_density: 12,
            dither_amount: 50,
            strength: 50,
            keep_colors: false,
            sharpen: false,
            merged: false,
            eight: false,
        }
    }
}

impl ToolPreset {
    pub fn capture(name: &str, editor: &Editor, toolbox: &Toolbox, settings: &Settings) -> Self {
        let o = &toolbox.options;
        ToolPreset {
            name: name.to_string(),
            tool: editor.tool.button_id().to_string(),
            brush: BrushPreset::capture(name, editor, toolbox, settings),
            filled: editor.options.filled,
            tolerance: editor.options.tolerance,
            contiguous: editor.options.contiguous,
            spray_radius: o.spray_radius,
            spray_density: o.spray_density,
            dither_amount: o.dither_amount,
            strength: o.strength,
            keep_colors: o.keep_colors,
            sharpen: o.sharpen,
            merged: o.merged,
            eight: o.eight,
        }
    }

    pub fn tool(&self) -> Option<Tool> {
        Tool::from_button_id(&self.tool)
    }

    /// Sets the options; the tool itself is picked first, on its own tick.
    fn apply(&self, editor: &mut Editor, toolbox: &mut Toolbox, settings: &mut Settings) {
        self.brush.apply(editor, toolbox, settings);
        editor.options.filled = self.filled;
        editor.options.tolerance = self.tolerance;
        editor.options.contiguous = self.contiguous;
        let o = &mut toolbox.options;
        o.spray_radius = self.spray_radius.clamp(1, 64);
        o.spray_density = self.spray_density.clamp(1, 100);
        o.dither_amount = self.dither_amount.min(100);
        o.strength = self.strength.min(100);
        o.keep_colors = self.keep_colors;
        o.sharpen = self.sharpen;
        o.merged = self.merged;
        o.eight = self.eight;
    }
}

/// A brush pack: brushes to share as one file.
#[derive(Serialize, Deserialize, Default)]
struct Pack {
    #[serde(rename = "pixl brushes")]
    version: u32,
    brushes: Vec<BrushPreset>,
}

/// Writes `brushes` as a pack.
pub fn pack(brushes: &[BrushPreset]) -> Vec<u8> {
    serde_json::to_vec_pretty(&Pack {
        version: 1,
        brushes: brushes.to_vec(),
    })
    .unwrap_or_default()
}

/// Reads a pack's brushes.
pub fn unpack(bytes: &[u8]) -> std::result::Result<Vec<BrushPreset>, String> {
    let pack: Pack = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if pack.version == 0 || pack.brushes.is_empty() {
        return Err("no pixl brushes in the file".into());
    }
    Ok(pack
        .brushes
        .into_iter()
        .map(BrushPreset::upgraded)
        .collect())
}

#[derive(Serialize, Deserialize, Default)]
struct Library {
    brushes: Vec<BrushPreset>,
}

#[derive(Serialize, Deserialize, Default)]
struct ToolPresets {
    presets: Vec<ToolPreset>,
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
struct Quick {
    colors: Vec<String>,
    brushes: Vec<BrushPreset>,
}

/// The library, the tool presets and the quick bar.
#[derive(Resource)]
pub struct Presets {
    store: Store,
    pub brushes: Vec<BrushPreset>,
    pub tools: Vec<ToolPreset>,
    pub recent_colors: Vec<Rgba>,
    pub recent_brushes: Vec<BrushPreset>,
    /// The brush the library marks as picked.
    pub selected: Option<usize>,
    /// A tool preset waiting for its tool to be picked first.
    pending: Option<(ToolPreset, bool)>,
    /// The editor revision the quick bar last looked at, and how many
    /// edits the history held then.
    seen_revision: u64,
    seen_edits: u64,
    shown: Option<(String, String, String)>,
}

impl Presets {
    pub fn load(store: Store) -> Self {
        let brushes = store
            .load::<Library>("brushes.json")
            .map(|l| {
                l.brushes
                    .into_iter()
                    .map(BrushPreset::upgraded)
                    .collect::<Vec<_>>()
            })
            .filter(|b| !b.is_empty())
            .unwrap_or_else(BrushPreset::starters);
        let tools = store
            .load::<ToolPresets>("tool-presets.json")
            .map(|t| {
                t.presets
                    .into_iter()
                    .map(|p| ToolPreset {
                        brush: p.brush.clone().upgraded(),
                        ..p
                    })
                    .collect()
            })
            .unwrap_or_default();
        let quick: Quick = store.load("quickbar.json").unwrap_or_default();
        Self {
            store,
            brushes,
            tools,
            recent_colors: quick
                .colors
                .iter()
                .filter_map(|c| crate::view::parse_hex(c))
                .collect(),
            recent_brushes: quick.brushes,
            selected: None,
            pending: None,
            seen_revision: 0,
            seen_edits: 0,
            shown: None,
        }
    }

    pub fn user() -> Self {
        Self::load(Store::user())
    }

    pub fn in_memory() -> Self {
        Self::load(Store::in_memory())
    }

    fn save_brushes(&self) {
        let _ = self.store.save(
            "brushes.json",
            &Library {
                brushes: self.brushes.clone(),
            },
        );
    }

    fn save_tools(&self) {
        let _ = self.store.save(
            "tool-presets.json",
            &ToolPresets {
                presets: self.tools.clone(),
            },
        );
    }

    fn save_quick(&self) {
        let _ = self.store.save(
            "quickbar.json",
            &Quick {
                colors: self.recent_colors.iter().map(|c| hex(*c)).collect(),
                brushes: self.recent_brushes.clone(),
            },
        );
    }

    /// Adds `brush`, replacing one of its name.
    pub fn add_brush(&mut self, brush: BrushPreset) {
        match self.brushes.iter_mut().find(|b| b.name == brush.name) {
            Some(b) => *b = brush,
            None => self.brushes.push(brush),
        }
        self.save_brushes();
    }

    /// Adds a tool preset, replacing one of its name for the same tool.
    pub fn add_tool_preset(&mut self, preset: ToolPreset) {
        match self
            .tools
            .iter_mut()
            .find(|p| p.name == preset.name && p.tool == preset.tool)
        {
            Some(p) => *p = preset,
            None => self.tools.push(preset),
        }
        self.save_tools();
    }

    /// Puts a tool preset in force: picks its tool now and sets its
    /// options on a later tick, once the tool's own options are loaded.
    pub fn use_tool_preset(&mut self, preset: ToolPreset, editor: &mut Editor) {
        if let Some(tool) = preset.tool() {
            editor.tool = tool;
            self.pending = Some((preset, true));
        }
    }

    /// Notes a color painted with, newest first.
    fn note_color(&mut self, c: Rgba) {
        if self.recent_colors.first() == Some(&c) {
            return;
        }
        self.recent_colors.retain(|x| *x != c);
        self.recent_colors.insert(0, c);
        self.recent_colors.truncate(QUICK_COLORS);
        self.save_quick();
    }

    /// Notes a brush painted with, newest first.
    fn note_brush(&mut self, mut b: BrushPreset) {
        b.name = String::new();
        b.dynamics = None;
        b.pressure = None;
        if self.recent_brushes.first() == Some(&b) {
            return;
        }
        self.recent_brushes.retain(|x| *x != b);
        self.recent_brushes.insert(0, b);
        self.recent_brushes.truncate(QUICK_BRUSHES);
        self.save_quick();
    }
}

/// Reads a PNG into a brush named after the file.
pub fn brush_from_png(path: &Path) -> std::result::Result<BrushPreset, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let png = decode_png(&bytes).map_err(|e| e.to_string())?;
    let (w, h) = png.size();
    let shape = Shape::from_image(w, h, &png.to_rgba())
        .ok_or("the image has no opaque pixels, or is over 256 px")?;
    Ok(BrushPreset {
        name: path
            .file_stem()
            .map_or_else(|| "Imported".into(), |s| s.to_string_lossy().into_owned()),
        shape,
        size: w.max(h),
        pixel_perfect: false,
        ..BrushPreset::default()
    })
}

/// The brushes panel's buttons, the tool presets dialog, the quick bar and
/// the menu items for packs.
#[allow(clippy::too_many_arguments)]
pub fn presets_input(
    mut presets: ResMut<Presets>,
    mut editor: ResMut<Editor>,
    mut toolbox: ResMut<Toolbox>,
    mut settings: ResMut<Settings>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    home: Res<Home>,
    mut commands: Commands,
    mut signals: Signals,
) {
    let editor = &mut *editor;
    if let Some((preset, fresh)) = presets.pending.take() {
        if fresh {
            presets.pending = Some((preset, false));
        } else if preset.tool() == Some(editor.tool) {
            preset.apply(editor, &mut toolbox, &mut settings);
        }
    }
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    chosen.extend(clicks.read().filter_map(|c| ids.id_of(c.entity)));
    if home.open {
        return;
    }
    for id in chosen {
        match id.as_str() {
            "brush-save" => {
                let name = signals.get::<String>("brush_name").unwrap_or_default();
                let name = name.trim();
                if name.is_empty() {
                    editor.message = Some("Name the brush first".into());
                } else {
                    let b = BrushPreset::capture(name, editor, &toolbox, &settings);
                    presets.add_brush(b);
                    editor.message = Some(format!("Saved the brush {name}"));
                }
            }
            "brush-from-selection" => match &editor.selection {
                Some(sel) if !sel.is_empty() => {
                    let shape = Shape::of(&BrushShape::Custom(Brush::from_selection(sel)));
                    let name = signals
                        .get::<String>("brush_name")
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| format!("Selection {}", presets.brushes.len() + 1));
                    let b = BrushPreset {
                        name: name.clone(),
                        shape,
                        pixel_perfect: false,
                        ..BrushPreset::capture(&name, editor, &toolbox, &settings)
                    };
                    b.apply(editor, &mut toolbox, &mut settings);
                    presets.add_brush(b);
                    editor.message = Some(format!("Made the brush {name} from the selection"));
                }
                _ => editor.message = Some("Select the pixels to make a brush of".into()),
            },
            "brush-delete" => {
                if let Some(i) = presets.selected.filter(|&i| i < presets.brushes.len()) {
                    let gone = presets.brushes.remove(i);
                    presets.selected = None;
                    presets.save_brushes();
                    editor.message = Some(format!("Deleted the brush {}", gone.name));
                } else {
                    editor.message = Some("Pick a brush to delete".into());
                }
            }
            "brush-import-png" => {
                commands.queue(FileDialog::open().filter("PNG image", &["png"]).then(
                    |world: &mut World, paths| {
                        if let Some(path) = paths.into_iter().next() {
                            let note = match brush_from_png(&path) {
                                Ok(b) => {
                                    let name = b.name.clone();
                                    world.resource_mut::<Presets>().add_brush(b);
                                    format!("Imported the brush {name}")
                                }
                                Err(e) => {
                                    format!("Could not make a brush of {}: {e}", path.display())
                                }
                            };
                            world.resource_mut::<Editor>().message = Some(note);
                        }
                    },
                ))
            }
            "brush-import-pack" => commands.queue(
                FileDialog::open()
                    .filter("pixl brush pack", &[PACK_EXTENSION])
                    .then(|world: &mut World, paths| {
                        if let Some(path) = paths.into_iter().next() {
                            let note = match std::fs::read(&path)
                                .map_err(|e| e.to_string())
                                .and_then(|b| unpack(&b))
                            {
                                Ok(brushes) => {
                                    let n = brushes.len();
                                    let mut p = world.resource_mut::<Presets>();
                                    for b in brushes {
                                        p.add_brush(b);
                                    }
                                    format!("Imported {n} brushes")
                                }
                                Err(e) => format!("Could not import {}: {e}", path.display()),
                            };
                            world.resource_mut::<Editor>().message = Some(note);
                        }
                    }),
            ),
            "brush-export-pack" => {
                let bytes = pack(&presets.brushes);
                commands.queue(
                    FileDialog::save()
                        .filter("pixl brush pack", &[PACK_EXTENSION])
                        .file_name(format!("brushes.{PACK_EXTENSION}"))
                        .then(move |world: &mut World, paths| {
                            if let Some(path) = paths.into_iter().next() {
                                let note = match write_atomic(&path, &bytes) {
                                    Ok(()) => format!("Exported the brushes to {}", path.display()),
                                    Err(e) => format!("Could not export: {e}"),
                                };
                                world.resource_mut::<Editor>().message = Some(note);
                            }
                        }),
                );
            }
            "tp-open" => {
                signals.set("tp_title", format!("{} presets", editor.tool.label()));
                signals.set("tp_message", String::new());
                signals.set("tp_open", true);
            }
            "tp-close" => signals.set("tp_open", false),
            "tp-save" => {
                let name = signals.get::<String>("tp_name").unwrap_or_default();
                let name = name.trim();
                if name.is_empty() {
                    signals.set("tp_message", "Name the preset first".to_string());
                } else {
                    let p = ToolPreset::capture(name, editor, &toolbox, &settings);
                    presets.add_tool_preset(p);
                    signals.set("tp_message", format!("Saved {name}"));
                }
            }
            other => {
                if let Some(i) = other
                    .strip_prefix("brush-pick-")
                    .and_then(|i| i.parse::<usize>().ok())
                {
                    if let Some(b) = presets.brushes.get(i).cloned() {
                        b.apply(editor, &mut toolbox, &mut settings);
                        presets.selected = Some(i);
                    }
                } else if let Some(i) = other
                    .strip_prefix("qb-color-")
                    .and_then(|i| i.parse::<usize>().ok())
                {
                    if let Some(c) = presets.recent_colors.get(i) {
                        editor.fg = *c;
                    }
                } else if let Some(i) = other
                    .strip_prefix("qb-brush-")
                    .and_then(|i| i.parse::<usize>().ok())
                {
                    if let Some(b) = presets.recent_brushes.get(i).cloned() {
                        b.apply(editor, &mut toolbox, &mut settings);
                    }
                } else if let Some(i) = other
                    .strip_prefix("tp-use-")
                    .and_then(|i| i.parse::<usize>().ok())
                {
                    if let Some(p) = presets.tools.get(i).cloned() {
                        presets.use_tool_preset(p, editor);
                        signals.set("tp_open", false);
                    }
                } else if let Some(i) = other
                    .strip_prefix("tp-delete-")
                    .and_then(|i| i.parse::<usize>().ok())
                    && i < presets.tools.len()
                {
                    presets.tools.remove(i);
                    presets.save_tools();
                }
            }
        }
    }
}

/// The library list, the quick bar and the presets dialog's list for the
/// active tool; and the quick bar's memory of what was painted with.
pub fn show_presets(
    mut presets: ResMut<Presets>,
    editor: Res<Editor>,
    toolbox: Res<Toolbox>,
    settings: Res<Settings>,
    signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    // A new edit made with a painting tool: note its color and brush.
    if editor.revision != presets.seen_revision {
        let edits = editor.history.undo_labels().count() as u64;
        let first = presets.seen_revision == 0;
        presets.seen_revision = editor.revision;
        let grew = edits > presets.seen_edits;
        presets.seen_edits = edits;
        if !first && grew && crate::tools::toolbox::paints(editor.tool) {
            presets.note_color(editor.fg);
            presets.note_brush(BrushPreset::capture("", &editor, &toolbox, &settings));
        }
    }
    let mut library = String::new();
    for (i, b) in presets.brushes.iter().enumerate() {
        let current = if presets.selected == Some(i) {
            " current"
        } else {
            ""
        };
        library.push_str(&format!(
            r#"<row id="brush-pick-{i}" class="brush-row{current}"><column class="brush-text"><label class="brush-name" text="{}"/><label class="brush-meta" text="{}"/></column></row>"#,
            escape(&b.name),
            escape(&b.summary())
        ));
    }
    let mut quick = String::new();
    for (i, c) in presets.recent_colors.iter().enumerate() {
        quick.push_str(&format!(
            r#"<tile id="qb-color-{i}" class="swatch qb-color" bg="{}"/>"#,
            hex(*c)
        ));
    }
    for (i, b) in presets.recent_brushes.iter().enumerate() {
        let size = match &b.shape {
            Shape::Custom { .. } => "*".to_string(),
            _ => b.size.to_string(),
        };
        let kind = match &b.shape {
            Shape::Round => "round",
            Shape::Square => "square",
            Shape::Custom { .. } => "custom",
        };
        quick.push_str(&format!(
            r#"<tooltip text="{}"><button id="qb-brush-{i}" class="qb-brush {kind}" text="{size}"/></tooltip>"#,
            escape(&b.summary())
        ));
    }
    let mut tool_list = String::new();
    if signals.get::<bool>("tp_open") == Some(true) {
        // The active tool's presets first, then the others'.
        let tool = editor.tool.button_id();
        let mut order: Vec<(usize, &ToolPreset)> = presets.tools.iter().enumerate().collect();
        order.sort_by_key(|(_, p)| p.tool != tool);
        for (i, p) in order {
            let of = p.tool().map_or("", |t| t.label());
            tool_list.push_str(&format!(
                r#"<row class="tp-row"><button id="tp-use-{i}" class="tp-use" text="{}"/><label class="tp-meta" text="{}"/><spacer/><button id="tp-delete-{i}" class="small" text="Delete"/></row>"#,
                escape(&p.name),
                escape(&format!("{of}, {}", p.brush.summary()))
            ));
        }
        if tool_list.is_empty() {
            tool_list =
                r#"<label class="tp-empty" text="No presets yet. Name the current options and save them."/>"#
                    .to_string();
        }
    }
    let now = (library, quick, tool_list);
    if presets.shown.as_ref() == Some(&now) {
        return;
    }
    let was = presets.shown.take().unwrap_or_default();
    for (id, new, old) in [
        ("brush-list", &now.0, &was.0),
        ("quickbar", &now.1, &was.1),
        ("tp-list", &now.2, &was.2),
    ] {
        if new != old
            && let Some(node) = dom::get_by_id(id)
        {
            node.set_inner_markup(new);
        }
    }
    presets.shown = Some(now);
    crate::repaint_soon(&wake);
}

/// Adds the presets' resources and systems.
pub fn install(app: App, presets: Presets) -> App {
    use lumenui::lumen_core::input::ClickSet;
    use lumenui::lumen_core::signals;
    use lumenui_canvas::CanvasSet;
    app.insert_resource(presets)
        .insert_signal("brush_name", "")
        .insert_signal("tp_open", false)
        .insert_signal("tp_name", "")
        .insert_signal("tp_message", "")
        .insert_signal("tp_title", "Tool presets")
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("presets::presets_input", presets_input)
                .after(crate::tools::toolbox::toolbox_options)
                .before(crate::tools::toolbox::use_toolbox)
                .after(ClickSet::Producers)
                .after(signals::push_textinput_to_signal),
        )
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("presets::show_presets", show_presets)
                .after(crate::tools::toolbox::use_toolbox)
                .after(crate::tools::use_tools)
                .before(signals::apply_text_bindings)
                .before(CanvasSet::Encode),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_mask_reads_back_from_its_rows() {
        let shape = Shape::Custom {
            rows: vec![".#.".into(), "###".into(), ".#".into()],
        };
        let editor_shape = shape.to_editor().unwrap();
        let BrushShape::Custom(brush) = &editor_shape else {
            panic!("a custom brush")
        };
        assert_eq!(brush.offsets().len(), 5);
        assert_eq!(
            Shape::of(&editor_shape),
            Shape::Custom {
                rows: vec![".#.".into(), "###".into(), ".#.".into()]
            }
        );
        assert!(
            Shape::Custom {
                rows: vec!["...".into()]
            }
            .to_editor()
            .is_none()
        );
    }

    #[test]
    fn an_image_makes_a_mask_of_its_opaque_pixels() {
        let (o, c) = (Rgba::BLACK, Rgba::TRANSPARENT);
        let shape = Shape::from_image(2, 2, &[o, c, c, o]).unwrap();
        assert_eq!(
            shape,
            Shape::Custom {
                rows: vec!["#.".into(), ".#".into()]
            }
        );
        assert!(Shape::from_image(1, 1, &[c]).is_none());
    }

    #[test]
    fn a_pack_reads_back_and_a_stranger_does_not() {
        let brushes = BrushPreset::starters();
        assert_eq!(unpack(&pack(&brushes)).unwrap(), brushes);
        assert!(unpack(b"{\"brushes\": []}").is_err());
        assert!(unpack(b"not json").is_err());
    }

    #[test]
    fn an_older_brush_reads_its_pressure_mapping_as_dynamics() {
        let old = br#"{"pixl brushes": 1, "brushes": [{"name": "Old", "shape": "round", "size": 6,
            "pressure": {"on": true, "curve": 2.0, "min_size": 25, "max_size": 100}}]}"#;
        let brushes = unpack(old).unwrap();
        let d = brushes[0].dynamics.clone().unwrap();
        assert_eq!(brushes[0].pressure, None);
        assert_eq!(d.size, crate::tools::dynamics::Follow::Pressure);
        assert_eq!((d.min_size, d.pressure.curve), (25, 2.0));
        assert!(brushes[0].summary().contains("size by pressure"));
        // Written again, it keeps them as dynamics.
        let again = unpack(&pack(&brushes)).unwrap();
        assert_eq!(again[0].dynamics, Some(d));
        assert_eq!(again[0].pressure, None);
        // A brush from before dynamics or pressure brings none.
        let plain = unpack(br#"{"pixl brushes": 1, "brushes": [{"name": "P"}]}"#).unwrap();
        assert_eq!(plain[0].dynamics, None);
    }

    #[test]
    fn the_library_and_presets_keep() {
        let dir = std::env::temp_dir().join(format!("pixl-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = Presets::load(Store::at(&dir));
        assert_eq!(p.brushes, BrushPreset::starters());
        p.add_brush(BrushPreset {
            name: "Mine".into(),
            size: 7,
            ..BrushPreset::default()
        });
        p.add_tool_preset(ToolPreset {
            name: "Fat eraser".into(),
            tool: "tool-eraser".into(),
            ..ToolPreset::default()
        });
        p.note_color(Rgba::opaque(1, 2, 3));
        p.note_color(Rgba::opaque(4, 5, 6));
        p.note_color(Rgba::opaque(1, 2, 3));
        let again = Presets::load(Store::at(&dir));
        assert_eq!(again.brushes.last().unwrap().size, 7);
        assert_eq!(again.tools[0].tool(), Some(Tool::Eraser));
        assert_eq!(
            again.recent_colors,
            [Rgba::opaque(1, 2, 3), Rgba::opaque(4, 5, 6)]
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
