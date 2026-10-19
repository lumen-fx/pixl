//! Sprite sheets: File > Export sprite sheet lays the document's frames out
//! on one PNG with Aseprite-shaped JSON beside it, and File > Import sprite
//! sheet splits a PNG into the frames of a new document. Both dialogs show a
//! preview; the export's is drawn on a worker thread, so a large document
//! keeps the editor answering while it renders.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use lumenui::ecs::world::World;
use lumenui::lumen_core::components::Transform;
use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{Canvas, Context2d, PixelBuffer};
use pixl_document::native::write_atomic;
use pixl_document::{Document, Rgba};
use pixl_io::png::decode_png;
use pixl_io::sheet::{
    JsonFrames, SheetGrid, SheetLayout, SheetOptions, SpriteSheet, sheet_document, split_sheet,
    sprite_sheet,
};

use crate::editor::Editor;
use crate::files::{Files, Request};
use crate::ids::ElementIds;
use crate::panels::escape;
use crate::settings::keymap::KeyCommands;
use crate::view::ViewColors;

/// The longest side a preview is drawn at; a larger sheet is sampled down.
pub const PREVIEW_SIDE: u32 = 512;

/// What the Export sprite sheet dialog's fields ask for, beside the
/// document.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportChoice {
    pub options: SheetOptions,
    /// Write the JSON beside the image.
    pub json: bool,
}

/// A sheet image being split.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetImage {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<Rgba>>,
}

/// A finished preview, or why it could not be drawn.
type Drawn = (SheetOptions, std::result::Result<SpriteSheet, String>);

/// The two dialogs' work.
#[derive(Resource, Default)]
pub struct Sheets {
    /// The export options a preview was last asked for, and the document
    /// revision.
    asked: Option<(SheetOptions, u64)>,
    job: Option<Mutex<Receiver<Drawn>>>,
    /// The last finished preview.
    pub preview: Option<(SheetOptions, Arc<SpriteSheet>)>,
    preview_drawn: bool,
    /// The frames dropdown as last written.
    frames_markup: Option<String>,
    /// The image to split.
    pub image: Option<SheetImage>,
    import_drawn: Option<(SheetGrid, u32, u32)>,
    /// The image whose cell size the fields were given a first guess for.
    guessed: Option<PathBuf>,
}

/// The text a number field holds, as a number in `lo..=hi`.
fn field(signals: &Signals, name: &str, lo: u32, hi: u32, default: u32) -> u32 {
    signals
        .get::<String>(name)
        .and_then(|t| t.trim().parse::<u32>().ok())
        .filter(|n| (lo..=hi).contains(n))
        .unwrap_or(default)
}

fn flag(signals: &Signals, name: &str) -> bool {
    signals.get::<bool>(name).unwrap_or(false)
}

/// The document's name without its extension, for frame names and the
/// file the dialog suggests.
fn title(editor: &Editor) -> String {
    let name = editor.name();
    Path::new(&name)
        .file_stem()
        .map_or(name.clone(), |s| s.to_string_lossy().into_owned())
}

/// The export options the dialog's fields describe for `doc`.
pub fn export_choice(signals: &Signals, doc: &Document, title: &str) -> ExportChoice {
    let layout = match signals.get::<String>("sx_layout").as_deref() {
        Some("Vertical strip") => SheetLayout::Vertical,
        Some("Grid by rows") => SheetLayout::Grid {
            columns: field(signals, "sx_columns", 1, 1024, 8),
        },
        Some("Grid by columns") => SheetLayout::Columns {
            rows: field(signals, "sx_rows", 1, 1024, 8),
        },
        Some("Packed") => SheetLayout::Packed,
        _ => SheetLayout::Horizontal,
    };
    let n = doc.frame_count() as u32;
    let frames = match signals.get::<String>("sx_frames").as_deref() {
        Some("Frames...") => {
            let from = field(signals, "sx_from", 1, n, 1);
            let to = field(signals, "sx_to", 1, n, n).max(from);
            Some(from as usize - 1..to as usize)
        }
        Some(tag) => tag
            .strip_prefix("Tag: ")
            .and_then(|name| SheetOptions::tag_range(doc, name)),
        None => None,
    };
    let json = signals.get::<String>("sx_json");
    ExportChoice {
        options: SheetOptions {
            layout,
            frames,
            padding: field(signals, "sx_padding", 0, 256, 0),
            border: field(signals, "sx_border", 0, 256, 0),
            trim: flag(signals, "sx_trim"),
            by_layer: flag(signals, "sx_by_layer"),
            scale: field(signals, "sx_scale", 1, 16, 1),
            title: title.to_string(),
            image_name: format!("{title}.png"),
            json_frames: if json.as_deref() == Some("Array") {
                JsonFrames::Array
            } else {
                JsonFrames::Hash
            },
        },
        json: json.as_deref() != Some("None"),
    }
}

/// The split the Import sprite sheet dialog's fields describe for an image
/// of `size`.
pub fn import_grid(signals: &Signals, size: (u32, u32)) -> SheetGrid {
    let offset = (
        field(signals, "si_ox", 0, 8192, 0),
        field(signals, "si_oy", 0, 8192, 0),
    );
    let padding = (
        field(signals, "si_px", 0, 1024, 0),
        field(signals, "si_py", 0, 1024, 0),
    );
    let mut grid = if signals.get::<String>("si_mode").as_deref() == Some("Count") {
        SheetGrid::count(
            size,
            field(signals, "si_cols", 1, 1024, 1),
            field(signals, "si_rows", 1, 1024, 1),
            offset,
            padding,
        )
    } else {
        SheetGrid {
            cell: (
                field(signals, "si_w", 1, 8192, 32),
                field(signals, "si_h", 1, 8192, 32),
            ),
            offset,
            padding,
            skip_empty: false,
        }
    };
    grid.skip_empty = flag(signals, "si_skip");
    grid
}

/// Writes a sheet as `path` (a PNG) and its JSON beside it.
pub fn write_sheet(
    doc: &Document,
    choice: &ExportChoice,
    path: &Path,
) -> std::result::Result<String, String> {
    let png_path = crate::files::with_extension(path, "png");
    let mut options = choice.options.clone();
    if let Some(name) = png_path.file_name() {
        options.image_name = name.to_string_lossy().into_owned();
    }
    let sheet = sprite_sheet(doc, &options).map_err(|e| e.to_string())?;
    let png = sheet.to_png().map_err(|e| e.to_string())?;
    write_atomic(&png_path, &png)
        .map_err(|e| format!("could not write {}: {e}", png_path.display()))?;
    let mut done = format!(
        "Exported {} ({} x {} px, {} images)",
        png_path.display(),
        sheet.width,
        sheet.height,
        sheet.cells.len()
    );
    if choice.json {
        let json_path = png_path.with_extension("json");
        write_atomic(&json_path, sheet.json.as_bytes())
            .map_err(|e| format!("could not write {}: {e}", json_path.display()))?;
        done.push_str(" with its JSON");
    }
    Ok(done)
}

/// Reads a PNG to split.
pub fn read_sheet_image(path: &Path) -> std::result::Result<SheetImage, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let decoded =
        decode_png(&bytes).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let (width, height) = decoded.size();
    Ok(SheetImage {
        path: path.to_path_buf(),
        width,
        height,
        pixels: Arc::new(decoded.to_rgba()),
    })
}

/// A new document of the frames `grid` splits `image` into, with the colors
/// and tool of `window` (the editor in front).
pub fn import(
    window: &Editor,
    path: &Path,
    grid: &SheetGrid,
    duration_ms: u32,
) -> std::result::Result<Editor, String> {
    let image = read_sheet_image(path)?;
    let cells = split_sheet(image.width, image.height, &image.pixels, grid);
    let doc = sheet_document(grid.cell, &cells, duration_ms).map_err(|e| e.to_string())?;
    let frames = doc.frame_count();
    let mut editor = Editor::opened(doc, None, window);
    editor.imported_from = Some(path.to_path_buf());
    editor.name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    editor.message = Some(format!(
        "Imported {frames} frames of {} x {} from {}",
        grid.cell.0,
        grid.cell.1,
        path.display()
    ));
    Ok(editor)
}

/// Opens and answers the two dialogs.
#[allow(clippy::too_many_arguments)]
pub fn sheet_commands(
    mut editor: ResMut<Editor>,
    mut sheets: ResMut<Sheets>,
    mut files: ResMut<Files>,
    keys: Res<KeyCommands>,
    home: Res<crate::Home>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    mut commands: Commands,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    chosen.extend(clicks.read().filter_map(|c| ids.id_of(c.entity)));
    for id in ["file-export-sheet", "file-import-sheet"] {
        if keys.ran(id, false) {
            chosen.push(id.to_string());
        }
    }
    if home.open {
        chosen.retain(|id| id != "file-export-sheet");
    }
    let mut changed = false;
    for id in chosen {
        match id.as_str() {
            "file-export-sheet" => {
                signals.set("sheet_export_open", true);
                sheets.asked = None;
                sheets.frames_markup = None;
                changed = true;
            }
            "file-import-sheet" => {
                signals.set("sheet_import_open", true);
                changed = true;
            }
            "sx-cancel" => {
                signals.set("sheet_export_open", false);
                changed = true;
            }
            "si-cancel" => {
                signals.set("sheet_import_open", false);
                changed = true;
            }
            "sx-export" => {
                let title = title(&editor);
                let choice = export_choice(&signals, &editor.doc, &title);
                let generation = editor.generation;
                signals.set("sheet_export_open", false);
                changed = true;
                commands.queue(
                    FileDialog::save()
                        .filter("PNG images", &["png"])
                        .file_name(format!("{title}.png"))
                        .then(move |world: &mut World, paths| {
                            let Some(path) = paths.into_iter().next() else {
                                return;
                            };
                            crate::documents::edit_document(world, generation, |editor| {
                                let done = write_sheet(&editor.doc, &choice, &path);
                                editor.message = Some(done.unwrap_or_else(|e| e));
                            });
                        }),
                );
            }
            "si-choose" => commands.queue(FileDialog::open().filter("PNG images", &["png"]).then(
                |world: &mut World, paths| {
                    let Some(path) = paths.into_iter().next() else {
                        return;
                    };
                    match read_sheet_image(&path) {
                        Ok(image) => world.resource_mut::<Sheets>().set_image(image),
                        Err(e) => world.resource_mut::<Editor>().message = Some(e),
                    }
                },
            )),
            "si-import" => {
                if let Some(image) = sheets.image.clone() {
                    let grid = import_grid(&signals, (image.width, image.height));
                    let ms = field(&signals, "si_ms", 1, 60000, 100);
                    signals.set("sheet_import_open", false);
                    changed = true;
                    files.queued = Some(Request::Sheet(image.path, grid, ms));
                } else {
                    editor.message = Some("Choose a sheet image first".into());
                }
            }
            _ => {}
        }
    }
    if changed {
        crate::repaint_soon(&wake);
    }
}

impl Sheets {
    /// Takes a newly chosen image to split.
    pub fn set_image(&mut self, image: SheetImage) {
        self.image = Some(image);
        self.import_drawn = None;
    }
}

/// A first guess at a sheet's cells: square cells as tall as a strip, or
/// as wide as a column, else the whole image.
pub fn guess_cell(width: u32, height: u32) -> (u32, u32) {
    if width > height && width.is_multiple_of(height) {
        (height, height)
    } else if height > width && height.is_multiple_of(width) {
        (width, width)
    } else {
        (width, height)
    }
}

/// The frames dropdown: all frames, a range, or each tag.
fn frames_markup(doc: &Document) -> String {
    let mut out = String::from(
        r#"<dropdown id="sx-frames" class="setting-choice wide" bind-value="sx_frames"><option value="All frames"/><option value="Frames..."/>"#,
    );
    for tag in doc.tags() {
        out.push_str(&format!(r#"<option value="Tag: {}"/>"#, escape(&tag.name)));
    }
    out.push_str("</dropdown>");
    out
}

/// Keeps the dialogs' fields, previews and lines in step with what they
/// ask for.
#[allow(clippy::too_many_arguments)]
pub fn sheet_forms(
    editor: Res<Editor>,
    mut sheets: ResMut<Sheets>,
    colors: Res<ViewColors>,
    canvases: Query<(&Canvas, &Transform)>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let size_of = |id: &str| {
        canvases
            .iter()
            .find(|(c, _)| c.id == id)
            .map(|(_, t)| (t.size.x, t.size.y))
    };
    if flag(&signals, "sheet_export_open") {
        export_form(
            &editor,
            &mut sheets,
            &mut signals,
            size_of("sx-preview"),
            &wake,
        );
    }
    if flag(&signals, "sheet_import_open") {
        import_form(
            &mut sheets,
            &colors,
            &mut signals,
            size_of("si-preview"),
            &wake,
        );
    }
}

fn set_flag(signals: &mut Signals, name: &str, on: bool) {
    if signals.get::<bool>(name) != Some(on) {
        signals.set(name, on);
    }
}

fn export_form(
    editor: &Editor,
    sheets: &mut Sheets,
    signals: &mut Signals,
    preview: Option<(f32, f32)>,
    wake: &Option<Res<WakeDeadline>>,
) {
    let layout = signals.get::<String>("sx_layout").unwrap_or_default();
    set_flag(signals, "sx_rows_mode", layout == "Grid by rows");
    set_flag(signals, "sx_columns_mode", layout == "Grid by columns");
    set_flag(
        signals,
        "sx_range_mode",
        signals.get::<String>("sx_frames").as_deref() == Some("Frames..."),
    );
    let markup = frames_markup(&editor.doc);
    if sheets.frames_markup.as_deref() != Some(markup.as_str())
        && let Some(node) = dom::get_by_id("sx-frames-box")
    {
        node.set_inner_markup(&markup);
        sheets.frames_markup = Some(markup);
        if !signals.contains("sx_to") || signals.get::<String>("sx_to").as_deref() == Some("") {
            signals.set("sx_to", editor.doc.frame_count().to_string());
        }
        crate::repaint_soon(wake);
    }
    // Ask for a preview of what the fields say, on a worker.
    let choice = export_choice(signals, &editor.doc, &title(editor));
    let ask = (choice.options.clone(), editor.revision);
    if sheets.asked.as_ref() != Some(&ask) {
        sheets.asked = Some(ask);
        let (tx, rx) = channel();
        let doc = editor.doc.clone();
        let options = choice.options;
        std::thread::spawn(move || {
            let sheet = sprite_sheet(&doc, &options).map_err(|e| e.to_string());
            let _ = tx.send((options, sheet));
        });
        sheets.job = Some(Mutex::new(rx));
        signals.set("sx_info", "Drawing the preview...".to_string());
    }
    if let Some(rx) = &sheets.job {
        let got = rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_recv();
        match got {
            Ok((options, Ok(sheet))) => {
                let info = format!(
                    "{} x {} px, {} images",
                    sheet.width,
                    sheet.height,
                    sheet.cells.len()
                );
                signals.set("sx_info", info);
                sheets.preview = Some((options, Arc::new(sheet)));
                sheets.preview_drawn = false;
                sheets.job = None;
            }
            Ok((_, Err(e))) => {
                signals.set("sx_info", e);
                sheets.job = None;
            }
            Err(_) => {
                // Look again soon.
                if let Some(wake) = wake {
                    wake.request(std::time::Instant::now() + std::time::Duration::from_millis(30));
                }
            }
        }
    }
    if !sheets.preview_drawn
        && let (Some((_, sheet)), Some(size)) = (&sheets.preview, preview)
    {
        draw_preview(
            "sx-preview",
            size,
            sheet.width,
            sheet.height,
            &sheet.pixels,
            &[],
            None,
        );
        sheets.preview_drawn = true;
        crate::repaint_soon(wake);
    }
}

fn import_form(
    sheets: &mut Sheets,
    colors: &ViewColors,
    signals: &mut Signals,
    preview: Option<(f32, f32)>,
    wake: &Option<Res<WakeDeadline>>,
) {
    let count = signals.get::<String>("si_mode").as_deref() == Some("Count");
    set_flag(signals, "si_size_mode", !count);
    set_flag(signals, "si_count_mode", count);
    let Some(image) = sheets.image.clone() else {
        crate::tools::set_if_changed(signals, "si_file", "none".into());
        crate::tools::set_if_changed(signals, "si_info", "Choose a PNG to split.".into());
        return;
    };
    if sheets.guessed.as_ref() != Some(&image.path) {
        let (w, h) = guess_cell(image.width, image.height);
        signals.set("si_w", w.to_string());
        signals.set("si_h", h.to_string());
        sheets.guessed = Some(image.path.clone());
    }
    let name = image
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    crate::tools::set_if_changed(
        signals,
        "si_file",
        format!("{name} ({} x {})", image.width, image.height),
    );
    let grid = import_grid(signals, (image.width, image.height));
    let cells = split_sheet(image.width, image.height, &image.pixels, &grid).len();
    let info = if cells == 0 {
        "No whole cell fits the image.".to_string()
    } else {
        format!("{cells} frames of {} x {}", grid.cell.0, grid.cell.1)
    };
    crate::tools::set_if_changed(signals, "si_info", info);
    let Some(size) = preview else {
        return;
    };
    let key = (grid, size.0 as u32, size.1 as u32);
    if sheets.import_drawn == Some(key) {
        return;
    }
    let rects: Vec<pixl_document::Rect> = grid.rects((image.width, image.height));
    draw_preview(
        "si-preview",
        size,
        image.width,
        image.height,
        &image.pixels,
        &rects,
        colors.preview,
    );
    sheets.import_drawn = Some(key);
    crate::repaint_soon(wake);
}

/// Every `step`-th pixel of an image, so a preview of any size fits a
/// buffer.
fn sample(width: u32, height: u32, pixels: &[Rgba], step: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (width.div_ceil(step), height.div_ceil(step));
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = pixels[((y * step) * width + x * step) as usize];
            out.extend_from_slice(&[p.r, p.g, p.b, p.a]);
        }
    }
    (w, h, out)
}

/// Draws an image fitted into a preview canvas of `size`, pixels kept
/// square and sharp, with `rects` outlined in `line`.
fn draw_preview(
    id: &str,
    size: (f32, f32),
    width: u32,
    height: u32,
    pixels: &[Rgba],
    rects: &[pixl_document::Rect],
    line: Option<Rgba>,
) {
    let ctx = Context2d::new(id);
    ctx.resize(size.0, size.1);
    ctx.clear();
    if width == 0 || height == 0 || size.0 < 2.0 || size.1 < 2.0 {
        return;
    }
    let step = width.max(height).div_ceil(PREVIEW_SIDE).max(1);
    let (bw, bh, rgba) = sample(width, height, pixels, step);
    let Ok(buffer) = PixelBuffer::new(bw, bh) else {
        return;
    };
    let _ = buffer.write_rgba8(0, 0, bw, bh, &rgba);
    let (sw, sh) = (f64::from(size.0), f64::from(size.1));
    let k = (sw / f64::from(width)).min(sh / f64::from(height));
    // Whole screen pixels per image pixel when the image is small.
    let k = if k >= 1.0 { k.floor() } else { k };
    let (dw, dh) = (f64::from(width) * k, f64::from(height) * k);
    let (x0, y0) = (((sw - dw) / 2.0).round(), ((sh - dh) / 2.0).round());
    ctx.set_image_smoothing(false);
    ctx.draw_buffer_scaled(&buffer, x0, y0, dw, dh);
    if let Some(c) = line {
        ctx.set_line_width(1.0);
        ctx.set_stroke_color(lumenui_canvas::Rgba::new(
            f64::from(c.r) / 255.0,
            f64::from(c.g) / 255.0,
            f64::from(c.b) / 255.0,
            f64::from(c.a) / 255.0,
        ));
        for r in rects.iter().take(4096) {
            ctx.stroke_rect(
                (x0 + f64::from(r.x) * k).round() + 0.5,
                (y0 + f64::from(r.y) * k).round() + 0.5,
                (f64::from(r.w) * k).round() - 1.0,
                (f64::from(r.h) * k).round() - 1.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strip_guesses_square_cells() {
        assert_eq!(guess_cell(128, 32), (32, 32));
        assert_eq!(guess_cell(16, 64), (16, 16));
        assert_eq!(guess_cell(100, 30), (100, 30));
    }

    #[test]
    fn sampling_keeps_every_nth_pixel() {
        let px: Vec<Rgba> = (0..16).map(|i| Rgba::new(i, 0, 0, 255)).collect();
        let (w, h, out) = sample(4, 4, &px, 2);
        assert_eq!((w, h), (2, 2));
        assert_eq!(
            out.iter().step_by(4).copied().collect::<Vec<u8>>(),
            [0, 2, 8, 10]
        );
    }
}
