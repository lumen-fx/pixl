//! Tilemap layers in the editor: the Tileset panel, Layer > New tilemap
//! layer and the conversions to and from tilemaps, Pixels and Tiles mode
//! (Tab) with the draw mode of pixel edits, and File > Export tileset.
//!
//! The panel shows the active tilemap layer's tileset as a grid of tiles,
//! the empty tile 0 first. A click picks the tile the tile tools place and
//! switches to Tiles mode; Earlier and Later move the picked tile, Copy
//! duplicates it, Delete removes it (emptying the cells that showed it),
//! and Unused removes every tile no cell shows. Every change is one undo
//! step.

use lumenui::ecs::world::World;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{MenuClicked, MouseWheel, PointerButton, PointerPressed};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{Context2d, PixelBuffer};
use pixl_document::{
    ColorMode, Document, Error, NewLayer, Rgba, Scope, TileFlags, TileMode, TileRef, TilesetId,
};

use crate::editor::Editor;
use crate::home::Home;
use crate::ids::ElementIds;
use crate::settings::keymap::KeyCommands;
use crate::tools::set_if_changed;

/// The tileset canvas's element id.
const CANVAS: &str = "tileset-canvas";
/// The smallest and largest screen size of one tile in the panel.
const CELL_MIN: f64 = 8.0;
const CELL_MAX: f64 = 48.0;

/// What the New tilemap layer dialog makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Making {
    /// A new tilemap layer above the active layer.
    Layer,
    /// The active raster layer converted.
    Convert,
}

/// The Tileset panel and the tilemap dialog.
#[derive(Resource, Default)]
pub struct TilesetPanel {
    /// The first row of tiles the panel shows.
    scroll: u32,
    drawn: Option<Drawn>,
    /// The dialog's purpose while it is open, and the tileset it uses
    /// (`None` makes a new one).
    dialog: Option<(Making, Option<TilesetId>)>,
    /// The draw mode the tool options showed.
    shown_mode: Option<TileMode>,
    /// The tileset choices the dialog shows.
    chips: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Drawn {
    size: (f32, f32),
    revision: u64,
    tileset: Option<TilesetId>,
    picked: TileRef,
    scroll: u32,
}

fn mode_name(m: TileMode) -> &'static str {
    match m {
        TileMode::Manual => "Manual",
        TileMode::Auto => "Auto",
        TileMode::Stack => "Stack",
    }
}

fn mode_from(name: &str) -> Option<TileMode> {
    [TileMode::Manual, TileMode::Auto, TileMode::Stack]
        .into_iter()
        .find(|m| mode_name(*m) == name)
}

/// The panel's layout: the side of a tile on screen and the tiles a row.
fn layout(size: (u32, u32), panel: (f32, f32)) -> (f64, u32) {
    let side = f64::from(size.0.max(size.1));
    let cell = (side * 2.0).clamp(CELL_MIN, CELL_MAX);
    let cols = ((f64::from(panel.0) / (cell + 1.0)).floor() as u32).max(1);
    (cell, cols)
}

/// Where an element is laid out: its top left and size.
fn rect_of(id: &str) -> Option<(Vec2, Vec2)> {
    let r = dom::get_by_id(id)?.rect()?;
    Some((
        Vec2::new(r.client_x, r.client_y),
        Vec2::new(r.width, r.height),
    ))
}

impl Editor {
    /// Runs a tileset edit as one undo step over every image; a failure
    /// becomes the status line's message.
    fn tileset_edit<R>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut Document) -> std::result::Result<R, Error>,
    ) -> Option<R> {
        match self
            .history
            .transact(&mut self.doc, label, Scope::All, edit)
        {
            Ok(r) => {
                self.touch();
                Some(r)
            }
            Err(e) => {
                self.message = Some(e.to_string());
                None
            }
        }
    }

    /// Adds a tilemap layer of tileset `ts` (a new one of `tile` size when
    /// `None`) above the active layer and makes it active.
    pub fn add_tilemap_layer(&mut self, ts: Option<TilesetId>, tile: (u32, u32), name: String) {
        let at = self.placement_above(self.layer);
        let layer_name = format!("Tilemap {}", self.doc.walk().len() + 1);
        let made = self.tileset_edit("New tilemap layer", |doc| {
            let ts = match ts {
                Some(ts) => ts,
                None => doc.add_tileset(name, tile.0, tile.1)?,
            };
            doc.add_layer(NewLayer::Tilemap { tileset: ts }, layer_name, at)
        });
        if let Some(id) = made {
            self.layer = id;
            self.tiles_mode = true;
        }
    }

    /// Cuts the active layer into tiles of tileset `ts` (a new one of `tile`
    /// size when `None`).
    pub fn convert_to_tilemap(&mut self, ts: Option<TilesetId>, tile: (u32, u32), name: String) {
        let layer = self.layer;
        self.tileset_edit("Convert to tilemap", |doc| {
            let ts = match ts {
                Some(ts) => ts,
                None => doc.add_tileset(name, tile.0, tile.1)?,
            };
            doc.convert_to_tilemap(layer, ts)
        });
    }

    /// Makes the active tilemap layer a plain layer of the same pixels.
    pub fn convert_to_raster(&mut self) {
        let layer = self.layer;
        if self
            .tileset_edit("Convert to normal layer", |doc| {
                doc.convert_to_raster(layer)
            })
            .is_some()
        {
            self.tiles_mode = false;
        }
    }
}

/// The tile the panel shows picked: the tile tools' tile without its flips.
fn picked_index(editor: &Editor) -> u32 {
    editor.tile.index()
}

/// The tilemap dialog's tileset choices: New, then each tileset.
fn choices(doc: &Document, chosen: Option<TilesetId>) -> String {
    let mut out = String::new();
    let mut chip = |id: &str, text: &str, on: bool| {
        out.push_str(&format!(
            r#"<button id="{id}" class="ws-chip{}" text="{}"/>"#,
            if on { " active" } else { "" },
            crate::panels::escape(text)
        ));
    };
    chip("tm-set-new", "New tileset", chosen.is_none());
    for ts in doc.tilesets() {
        let (w, h) = ts.tile_size();
        chip(
            &format!("tm-set-{}", ts.id.0),
            &format!("{} ({w}x{h})", ts.name),
            chosen == Some(ts.id),
        );
    }
    out
}

fn open_dialog(panel: &mut TilesetPanel, editor: &Editor, making: Making, signals: &mut Signals) {
    // A new tileset unless one of the document's is picked.
    let chosen = None;
    panel.dialog = Some((making, chosen));
    signals.set(
        "tm_title",
        match making {
            Making::Layer => "New tilemap layer",
            Making::Convert => "Convert to tilemap",
        }
        .to_string(),
    );
    signals.set("tm_new_set", chosen.is_none());
    signals.set(
        "tm_name",
        format!("Tileset {}", editor.doc.tilesets().len() + 1),
    );
    signals.set("tm_open", true);
}

fn number(signals: &Signals, name: &str) -> Option<u32> {
    signals
        .get::<String>(name)
        .and_then(|t| t.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
}

/// What the panel, the dialog, the tool options and Tab do.
#[allow(clippy::too_many_arguments)]
pub fn tileset_input(
    mut editor: ResMut<Editor>,
    mut panel: ResMut<TilesetPanel>,
    keys: Res<KeyCommands>,
    home: Res<Home>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    mut pressed: MessageReader<PointerPressed>,
    mut wheel: MessageReader<MouseWheel>,
    hovered: Query<&LumenId, With<Hovered>>,
    ids: ElementIds,
    mut commands: Commands,
    mut signals: Signals,
) {
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    chosen.extend(clicks.read().filter_map(|c| ids.id_of(c.entity)));
    let presses: Vec<PointerPressed> = pressed.read().cloned().collect();
    let wheels: Vec<f32> = wheel.read().map(|w| w.delta.y).collect();
    if home.open {
        return;
    }
    let editor = &mut *editor;
    for key in ["tiles-mode", "file-export-tileset"] {
        if keys.ran(key, false) {
            chosen.push(if key == "tiles-mode" {
                "opt-tiles-mode".to_string()
            } else {
                key.to_string()
            });
        }
    }
    // The draw mode: a dropdown showing other than the editor's is the
    // user's choice.
    if let Some(shown) = panel.shown_mode
        && let Some(m) = signals
            .get::<String>("opt_tile_draw")
            .and_then(|n| mode_from(&n))
        && m != shown
    {
        editor.history.set_tile_mode(m);
    }
    let set = editor.active_tileset();
    for id in chosen {
        match id.as_str() {
            "opt-tiles-mode" => {
                if set.is_some() {
                    editor.tiles_mode = !editor.tiles_mode;
                } else {
                    editor.message = Some("Tiles mode needs a tilemap layer".to_string());
                }
            }
            "lm-tilemap-new" => open_dialog(&mut panel, editor, Making::Layer, &mut signals),
            "lm-tilemap-convert" => {
                let plain = editor
                    .doc
                    .layer(editor.layer)
                    .is_some_and(|l| l.is_raster() && !l.is_tilemap());
                if plain {
                    open_dialog(&mut panel, editor, Making::Convert, &mut signals);
                } else {
                    editor.message = Some("Pick a normal layer to convert".to_string());
                }
            }
            "lm-tilemap-raster" => {
                if set.is_some() {
                    editor.convert_to_raster();
                } else {
                    editor.message = Some("The layer is not a tilemap layer".to_string());
                }
            }
            "tm-cancel" => {
                panel.dialog = None;
                signals.set("tm_open", false);
            }
            "tm-ok" => {
                let Some((making, ts)) = panel.dialog.take() else {
                    continue;
                };
                signals.set("tm_open", false);
                let tile = (
                    number(&signals, "tm_width").unwrap_or(16),
                    number(&signals, "tm_height").unwrap_or(16),
                );
                let name = signals.get::<String>("tm_name").unwrap_or_default();
                let name = if name.trim().is_empty() {
                    "Tileset".to_string()
                } else {
                    name.trim().to_string()
                };
                match making {
                    Making::Layer => editor.add_tilemap_layer(ts, tile, name),
                    Making::Convert => editor.convert_to_tilemap(ts, tile, name),
                }
            }
            "file-export-tileset" => export_tileset(editor, &mut commands),
            other => {
                if let Some(rest) = other.strip_prefix("tm-set-")
                    && let Some((making, _)) = panel.dialog
                {
                    let ts = rest.parse::<u32>().ok().map(TilesetId);
                    panel.dialog = Some((making, ts));
                    signals.set("tm_new_set", ts.is_none());
                    continue;
                }
                let Some(ts) = set else { continue };
                let i = picked_index(editor);
                let picked = |editor: &mut Editor, i: u32| {
                    editor.tile = TileRef::new(i, TileFlags::NONE);
                };
                match other {
                    "ts-earlier" if i > 1 => {
                        if editor
                            .tileset_edit("Move tile", |d| d.move_tile(ts, i, i - 1))
                            .is_some()
                        {
                            picked(editor, i - 1);
                        }
                    }
                    "ts-later" => {
                        let len = editor.doc.tileset(ts).map_or(0, |t| t.len()) as u32;
                        if i >= 1
                            && i + 1 < len
                            && editor
                                .tileset_edit("Move tile", |d| d.move_tile(ts, i, i + 1))
                                .is_some()
                        {
                            picked(editor, i + 1);
                        }
                    }
                    "ts-copy" => {
                        if let Some(n) =
                            editor.tileset_edit("Duplicate tile", |d| d.duplicate_tile(ts, i))
                        {
                            picked(editor, n);
                        }
                    }
                    "ts-delete" => {
                        if editor
                            .tileset_edit("Delete tile", |d| d.remove_tile(ts, i))
                            .is_some()
                        {
                            picked(editor, i.saturating_sub(1).max(1));
                        }
                    }
                    "ts-unused" => {
                        if let Some(n) = editor
                            .tileset_edit("Delete unused tiles", |d| d.remove_unused_tiles(ts))
                        {
                            editor.message = Some(match n {
                                1 => "1 unused tile deleted".to_string(),
                                n => format!("{n} unused tiles deleted"),
                            });
                            let len = editor.doc.tileset(ts).map_or(1, |t| t.len()) as u32;
                            if i >= len {
                                picked(editor, len.saturating_sub(1));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // A click on a tile picks it for the tile tools, in Tiles mode.
    let over = hovered
        .iter()
        .any(|h| h.0 == CANVAS || h.0 == "tileset-stage");
    let Some(ts) = set.and_then(|id| editor.doc.tileset(id)) else {
        return;
    };
    let Some((at, size)) = rect_of(CANVAS) else {
        return;
    };
    let (cell, cols) = layout(ts.tile_size(), (size.x, size.y));
    let rows = (ts.len() as u32).div_ceil(cols);
    for dy in wheels {
        if over {
            let shown = ((f64::from(size.y) / (cell + 1.0)).floor() as u32).max(1);
            let most = rows.saturating_sub(shown);
            panel.scroll = if dy > 0.0 {
                panel.scroll.saturating_sub(1)
            } else {
                (panel.scroll + 1).min(most)
            };
        }
    }
    for p in presses {
        if p.button != PointerButton::Primary || !over {
            continue;
        }
        let local = p.position - at;
        if local.x < 0.0 || local.y < 0.0 {
            continue;
        }
        let col = (f64::from(local.x) / (cell + 1.0)).floor() as u32;
        let row = (f64::from(local.y) / (cell + 1.0)).floor() as u32 + panel.scroll;
        let index = row * cols + col;
        if col < cols && (index as usize) < ts.len() {
            editor.tile = TileRef::new(index, TileFlags::NONE);
            editor.tiles_mode = true;
        }
    }
}

/// File > Export tileset: the active tilemap layer's tileset (or the
/// document's first) as a PNG sheet with JSON beside it.
fn export_tileset(editor: &mut Editor, commands: &mut Commands) {
    let Some(ts) = editor
        .active_tileset()
        .or_else(|| editor.doc.tilesets().first().map(|t| t.id))
    else {
        editor.message = Some("The document has no tileset".to_string());
        return;
    };
    let name = editor
        .doc
        .tileset(ts)
        .map_or("tileset".to_string(), |t| t.name.clone());
    let generation = editor.generation;
    commands.queue(
        FileDialog::save()
            .filter("PNG images", &["png"])
            .file_name(format!("{name}.png"))
            .then(move |world: &mut World, paths| {
                if let Some(path) = paths.into_iter().next() {
                    crate::documents::edit_document(world, generation, |editor| {
                        editor.message = Some(match write_tileset(&editor.doc, ts, &path) {
                            Ok(json) => {
                                format!("Wrote {} and {}", path.display(), json.display())
                            }
                            Err(e) => e,
                        });
                    });
                }
            }),
    );
}

/// Writes a tileset sheet to `path` and its JSON beside it; returns the
/// JSON's path.
pub fn write_tileset(
    doc: &Document,
    ts: TilesetId,
    path: &std::path::Path,
) -> std::result::Result<std::path::PathBuf, String> {
    let image = path.file_name().map_or("tileset.png".to_string(), |n| {
        n.to_string_lossy().into_owned()
    });
    let sheet = pixl_io::sheet::tileset_sheet(doc, ts, &image).map_err(|e| e.to_string())?;
    let png = sheet.to_png().map_err(|e| e.to_string())?;
    pixl_document::native::write_atomic(path, &png).map_err(|e| e.to_string())?;
    let json = path.with_extension("json");
    pixl_document::native::write_atomic(&json, sheet.json.as_bytes()).map_err(|e| e.to_string())?;
    Ok(json)
}

/// The tile pixels of a tileset as RGBA.
fn tile_rgba(doc: &Document, px: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 4);
    for &v in px {
        let c = match doc.mode() {
            ColorMode::Rgba => Rgba::from_u32(v),
            ColorMode::Indexed if v as u8 == doc.transparent_index() => Rgba::TRANSPARENT,
            ColorMode::Indexed => doc.palette().get(v as u8),
        };
        out.extend_from_slice(&[c.r, c.g, c.b, c.a]);
    }
    out
}

/// The tool options' tilemap controls, the panel's title and its tiles.
pub fn show_tileset(
    editor: Res<Editor>,
    mut panel: ResMut<TilesetPanel>,
    colors: Res<crate::view::ViewColors>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let set = editor.active_tileset();
    let on_map = set.is_some();
    if signals.get::<bool>("show_tiles") != Some(on_map) {
        signals.set("show_tiles", on_map);
    }
    set_if_changed(
        &mut signals,
        "tiles_mode_label",
        if editor.tiles_mode { "Tiles" } else { "Pixels" }.to_string(),
    );
    let mode = editor.history.tile_mode();
    set_if_changed(&mut signals, "opt_tile_draw", mode_name(mode).to_string());
    panel.shown_mode = Some(mode);
    // The dialog's choices, once its markup is there.
    if let Some((_, chosen)) = panel.dialog {
        let markup = choices(&editor.doc, chosen);
        if panel.chips.as_ref() != Some(&markup)
            && let Some(node) = dom::get_by_id("tm-sets")
        {
            node.set_inner_markup(&markup);
            panel.chips = Some(markup);
            crate::repaint_soon(&wake);
        }
    } else {
        panel.chips = None;
    }
    let ts = set.and_then(|id| editor.doc.tileset(id));
    let (title, info) = match ts {
        Some(ts) => {
            let (w, h) = ts.tile_size();
            let i = editor.tile.index();
            let picked = if (i as usize) < ts.len() {
                format!("tile {}", ts.display_index(i))
            } else {
                "no tile".to_string()
            };
            (
                ts.name.clone(),
                format!("{} tiles of {w}x{h}, {picked} picked", ts.len() - 1),
            )
        }
        None => (
            String::new(),
            "Pick a tilemap layer to see its tiles".to_string(),
        ),
    };
    set_if_changed(&mut signals, "ts_name", title);
    set_if_changed(&mut signals, "ts_info", info);

    let Some((_, size)) = rect_of(CANVAS) else {
        return;
    };
    let now = Drawn {
        size: (size.x, size.y),
        revision: editor.revision,
        tileset: set,
        picked: editor.tile,
        scroll: panel.scroll,
    };
    if panel.drawn.as_ref() == Some(&now) || size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let ctx = Context2d::new(CANVAS);
    if panel.drawn.as_ref().map(|d| d.size) != Some(now.size) {
        ctx.resize(size.x, size.y);
    }
    ctx.clear();
    panel.drawn = Some(now);
    let Some(ts) = ts else {
        crate::repaint_soon(&wake);
        return;
    };
    let (tw, th) = ts.tile_size();
    let (cell, cols) = layout((tw, th), (size.x, size.y));
    let fit = (cell / f64::from(tw.max(th))).max(0.01);
    let (dw, dh) = (f64::from(tw) * fit, f64::from(th) * fit);
    let color = |c: Rgba| {
        lumenui_canvas::Rgba::new(
            f64::from(c.r) / 255.0,
            f64::from(c.g) / 255.0,
            f64::from(c.b) / 255.0,
            f64::from(c.a) / 255.0,
        )
    };
    ctx.set_image_smoothing(false);
    let shown_rows = ((f64::from(size.y) / (cell + 1.0)).ceil() as u32).max(1);
    let first = panel.scroll * cols;
    let last = ((panel.scroll + shown_rows) * cols).min(ts.len() as u32);
    for i in first..last {
        let k = i - first;
        let (x, y) = (
            f64::from(k % cols) * (cell + 1.0),
            f64::from(k / cols) * (cell + 1.0),
        );
        if let Some(c) = colors.checker_a {
            ctx.set_fill_color(color(c));
            ctx.fill_rect(x, y, dw, dh);
        }
        if i > 0
            && let Some(px) = ts.tile(i)
            && let Ok(buf) = PixelBuffer::new(tw, th)
        {
            let _ = buf.write_rgba8(0, 0, tw, th, &tile_rgba(&editor.doc, px));
            ctx.draw_buffer_scaled(&buf, x, y, dw, dh);
        }
        // The picked tile: the selection's two colors, so the outline shows
        // over light and dark tiles alike.
        if i == editor.tile.index()
            && let (Some(a), Some(b)) = (colors.marquee_a, colors.marquee_b)
        {
            ctx.set_line_width(1.0);
            ctx.set_stroke_color(color(b));
            ctx.stroke_rect(x + 0.5, y + 0.5, dw - 1.0, dh - 1.0);
            ctx.set_stroke_color(color(a));
            ctx.stroke_rect(x + 1.5, y + 1.5, dw - 3.0, dh - 3.0);
        }
    }
    crate::repaint_soon(&wake);
}

/// Adds the panel's resources, signals and systems.
pub fn install(app: App) -> App {
    use lumenui::lumen_core::input::ClickSet;
    use lumenui::lumen_core::signals;
    use lumenui_canvas::CanvasSet;
    app.insert_resource(TilesetPanel::default())
        .insert_signal("show_tiles", false)
        .insert_signal("tiles_mode_label", "Pixels")
        .insert_signal("opt_tile_draw", "Auto")
        .insert_signal("ts_name", "")
        .insert_signal("ts_info", "")
        .insert_signal("tm_open", false)
        .insert_signal("tm_title", "New tilemap layer")
        .insert_signal("tm_new_set", true)
        .insert_signal("tm_width", "16")
        .insert_signal("tm_height", "16")
        .insert_signal("tm_name", "Tileset 1")
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("tileset::tileset_input", tileset_input)
                .after(crate::panels::layers::layer_input)
                .before(crate::tools::use_tools)
                .after(ClickSet::Producers)
                .after(signals::push_textinput_to_signal)
                .before(signals::apply_text_bindings)
                .before(signals::apply_value_bindings),
        )
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("tileset::show_tileset", show_tileset)
                .after(crate::view::draw_stage)
                .before(signals::apply_text_bindings)
                .before(signals::apply_checked_bindings)
                .before(signals::apply_value_bindings)
                .before(CanvasSet::Encode),
        )
}
