//! The Image menu: crop, canvas size and image size, flips and turns of the
//! whole canvas, the color mode (indexed with a choice of palette and
//! dithering, or RGBA), replacing a color in every cel, and the palette:
//! load and save it as .gpl, .pal, .hex or .ase, sort it, build ramps, make
//! one from the image, and lock painting to it. Every change is one undo
//! step.

use std::path::Path;

use lumenui::ecs::world::World;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_document::palettes::{
    self, Dither, PaletteFormat, RampOptions, SortKey, palette_from_pixels, read_palette,
    write_palette,
};
use pixl_document::{
    Bayer, CanvasTransform, ColorMode, CompositeOptions, Document, Palette, Point, Rect, Resample,
    Rgba, Scope, composite_frame, crop, resize_canvas, transform_canvas,
};
use pixl_paint::tools::paint::replace_color_everywhere;

use crate::editor::Editor;
use crate::menus::Chosen;
use crate::tools::hex;
use crate::view::jobs::Task;

/// Crops the canvas to `rect` as one undo step.
pub fn crop_to(editor: &mut Editor, rect: Rect) {
    let rect = rect.intersect(&editor.doc.bounds());
    if rect.is_empty() {
        return;
    }
    let done = editor
        .history
        .transact(&mut editor.doc, "Crop", Scope::All, |d| crop(d, rect));
    match done {
        Ok(_) => after_resize(editor),
        Err(e) => editor.message = Some(e.to_string()),
    }
}

/// What every change of the canvas size leaves behind: no selection, the
/// symmetry centered on the new canvas.
pub fn after_resize(editor: &mut Editor) {
    editor.selection = None;
    let (w, h) = (editor.doc.width(), editor.doc.height());
    editor.symmetry = pixl_paint::Symmetry::centered(editor.symmetry.mode, w, h);
    editor.touch();
}

/// A transform of the whole canvas as one undo step.
pub fn transform(editor: &mut Editor, label: &str, op: CanvasTransform) {
    let done = editor
        .history
        .transact(&mut editor.doc, label, Scope::All, |d| {
            transform_canvas(d, op)
        });
    match done {
        Ok(_) => after_resize(editor),
        Err(e) => editor.message = Some(e.to_string()),
    }
}

/// Where the old canvas sits in a new one of `w x h`, for an anchor named
/// as the canvas size dialog names it.
pub fn anchor_offset(anchor: &str, old: (u32, u32), new: (u32, u32)) -> Point {
    let dx = new.0 as i32 - old.0 as i32;
    let dy = new.1 as i32 - old.1 as i32;
    let lower = anchor.to_ascii_lowercase();
    let x = if lower.ends_with("left") {
        0
    } else if lower.ends_with("right") {
        dx
    } else {
        dx / 2
    };
    let y = if lower.starts_with("top") {
        0
    } else if lower.starts_with("bottom") {
        dy
    } else {
        dy / 2
    };
    Point::new(x, y)
}

/// Reads a color as a color field writes it: `#rrggbb`, `#rrggbbaa` or
/// `color(srgb r g b / a)`.
pub fn parse_color(text: &str) -> Option<Rgba> {
    let t = text.trim();
    if let Some(c) = crate::view::parse_hex(t) {
        return Some(c);
    }
    let inner = t.strip_prefix("color(srgb")?.strip_suffix(')')?;
    let (rgb, a) = match inner.split_once('/') {
        Some((rgb, a)) => (rgb, a.trim().parse::<f64>().ok()?),
        None => (inner, 1.0),
    };
    let v: Vec<f64> = rgb
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() != 3 {
        return None;
    }
    let b = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(Rgba::new(b(v[0]), b(v[1]), b(v[2]), b(a)))
}

/// Puts the document on `palette`: an RGBA document takes it as its
/// palette; an indexed one has its pixels remapped to it by color.
pub fn use_palette(editor: &mut Editor, label: &str, palette: Palette) {
    let indexed = editor.doc.mode() == ColorMode::Indexed;
    let done = editor
        .history
        .transact(&mut editor.doc, label, Scope::All, |d| {
            if indexed {
                palettes::reduce(d, palette, Dither::None).map(|_| ())
            } else {
                d.set_palette(palette);
                Ok(())
            }
        });
    match done {
        Ok(()) => editor.touch(),
        Err(e) => editor.message = Some(e.to_string()),
    }
}

fn number(signals: &Signals, name: &str, default: i64) -> i64 {
    signals
        .get::<String>(name)
        .and_then(|t| t.trim().parse::<f64>().ok())
        .map_or(default, |v| v.round() as i64)
}

/// The palette lock and the dialogs' state.
#[derive(Resource, Default)]
pub struct ImageMenu {
    /// Painting colors snap to the nearest palette color.
    pub lock: bool,
    ramp_shown: Option<Vec<Rgba>>,
    /// An edit of the whole document running on a thread of its own, its
    /// dialog open (and the document out of reach) until it lands.
    task: Option<Running>,
}

/// A whole-document edit off the UI thread: its undo label, the signal that
/// opens its dialog, and the document it makes (`None` once stopped).
struct Running {
    label: &'static str,
    dialog: &'static str,
    /// The edit changes the canvas size.
    resizes: bool,
    task: Task<Option<std::result::Result<Document, pixl_document::Error>>>,
}

impl ImageMenu {
    /// Whether a whole-document edit is still running.
    pub fn busy(&self) -> bool {
        self.task.is_some()
    }
}

/// Puts `doc` on a palette in a [`Task`]: `palette` gives it (from the
/// document as it is), and an indexed document, or `index` with a dither,
/// has its images indexed to it.
fn start_palette_task(
    editor: &Editor,
    palette: impl FnOnce(&Document) -> Palette + Send + 'static,
    index: Option<Dither>,
) -> Task<Option<std::result::Result<Document, pixl_document::Error>>> {
    let doc = editor.doc.clone();
    let threads = crate::view::jobs::worker_threads();
    Task::spawn(move |progress| {
        let mut d = doc;
        let p = palette(&d);
        if progress.stopped() {
            return None;
        }
        let dither = match (index, d.mode()) {
            (Some(dither), _) => dither,
            (None, ColorMode::Indexed) => Dither::None,
            (None, ColorMode::Rgba) => {
                d.set_palette(p);
                return Some(Ok(d));
            }
        };
        match palettes::reduce_with(&mut d, p, dither, threads, progress) {
            Ok(Some(_)) => Some(Ok(d)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    })
}

/// The ramp the ramp builder's fields describe, around the foreground.
fn ramp_of(editor: &Editor, signals: &Signals) -> Vec<Rgba> {
    let o = RampOptions {
        steps: number(signals, "ramp_steps", 5).clamp(2, 16) as usize,
        hue_shift: number(signals, "ramp_hue", 40) as f32,
        lightness: (
            number(signals, "ramp_dark", 15).clamp(0, 100) as f32 / 100.0,
            number(signals, "ramp_light", 85).clamp(0, 100) as f32 / 100.0,
        ),
        saturation_shift: number(signals, "ramp_sat", -15) as f32 / 100.0,
    };
    palettes::ramp(editor.fg, &o)
}

/// Runs the Image menu's commands and dialogs.
pub fn image_commands(
    chosen: Res<Chosen>,
    mut editor: ResMut<Editor>,
    mut state: ResMut<ImageMenu>,
    mut signals: Signals,
    mut commands: Commands,
    wake: Option<Res<WakeDeadline>>,
) {
    let editor = &mut *editor;
    let size = (editor.doc.width(), editor.doc.height());
    for id in &chosen.0 {
        match id.as_str() {
            "image-crop" => match editor.selection.as_ref().map(|s| s.bounds()) {
                Some(b) if !b.is_empty() => crop_to(editor, b),
                _ => editor.message = Some("Select the area to keep".into()),
            },
            "image-flip-h" => transform(editor, "Flip canvas", CanvasTransform::FlipH),
            "image-flip-v" => transform(editor, "Flip canvas", CanvasTransform::FlipV),
            "image-rotate-cw" => transform(editor, "Rotate canvas", CanvasTransform::Rotate90(1)),
            "image-rotate-ccw" => transform(editor, "Rotate canvas", CanvasTransform::Rotate90(-1)),
            "image-canvas-size" => {
                signals.set("canvas_title", "Canvas size");
                signals.set("canvas_w", size.0.to_string());
                signals.set("canvas_h", size.1.to_string());
                signals.set("canvas_anchor", "Center");
                signals.set("canvas_open", true);
            }
            "canvas-ok" => {
                signals.set("canvas_open", false);
                let w = number(&signals, "canvas_w", size.0 as i64).clamp(1, 32768) as u32;
                let h = number(&signals, "canvas_h", size.1 as i64).clamp(1, 32768) as u32;
                let anchor = signals.get::<String>("canvas_anchor").unwrap_or_default();
                let at = anchor_offset(&anchor, size, (w, h));
                let done =
                    editor
                        .history
                        .transact(&mut editor.doc, "Canvas size", Scope::All, |d| {
                            resize_canvas(d, w, h, at)
                        });
                match done {
                    Ok(_) => after_resize(editor),
                    Err(e) => editor.message = Some(e.to_string()),
                }
            }
            "canvas-cancel" => signals.set("canvas_open", false),
            "image-resize" => {
                signals.set("resize_title", "Image size");
                signals.set("resize_w", size.0.to_string());
                signals.set("resize_h", size.1.to_string());
                signals.set("resize_open", true);
            }
            "resize-ok" if state.task.is_none() => {
                let w = number(&signals, "resize_w", size.0 as i64).clamp(1, 32768) as u32;
                let h = number(&signals, "resize_h", size.1 as i64).clamp(1, 32768) as u32;
                let resample = match signals.get::<String>("resize_resample").as_deref() {
                    Some("RotSprite") => Resample::RotSprite,
                    Some("Clean edge") => Resample::CleanEdge,
                    _ => Resample::Nearest,
                };
                // Every cel is resampled, on a thread of its own.
                let doc = editor.doc.clone();
                let task = Task::spawn(move |_| {
                    let mut d = doc;
                    Some(
                        transform_canvas(&mut d, CanvasTransform::Scale(w, h, resample)).map(|_| d),
                    )
                });
                state.task = Some(Running {
                    label: "Image size",
                    dialog: "resize_open",
                    resizes: true,
                    task,
                });
            }
            "resize-cancel" => {
                state.task = None;
                signals.set("resize_open", false);
            }
            "image-indexed" => {
                signals.set("indexed_title", "Indexed color");
                signals.set("indexed_open", true);
            }
            "indexed-ok" if state.task.is_none() => {
                let dither = match signals.get::<String>("indexed_dither").as_deref() {
                    Some("Bayer 2x2") => Dither::Ordered(Bayer::B2),
                    Some("Bayer 4x4") => Dither::Ordered(Bayer::B4),
                    Some("Bayer 8x8") => Dither::Ordered(Bayer::B8),
                    Some("Floyd-Steinberg") => Dither::FloydSteinberg,
                    _ => Dither::None,
                };
                let n = number(&signals, "indexed_colors", 16).clamp(2, 256) as usize;
                let new = signals.get::<String>("indexed_palette").as_deref()
                    == Some("New palette from the image");
                let frame = editor.frame;
                let task = start_palette_task(
                    editor,
                    move |d| {
                        if new {
                            palette_from_pixels(
                                composite_frame(d, frame, &CompositeOptions::EXPORT),
                                n,
                                true,
                            )
                        } else {
                            d.palette().clone()
                        }
                    },
                    Some(dither),
                );
                state.task = Some(Running {
                    label: "Indexed color",
                    dialog: "indexed_open",
                    resizes: false,
                    task,
                });
            }
            "indexed-cancel" => {
                state.task = None;
                signals.set("indexed_open", false);
            }
            "image-rgba" => {
                editor
                    .history
                    .transact(&mut editor.doc, "RGBA color", Scope::All, |d| {
                        d.set_color_mode(ColorMode::Rgba)
                    });
                editor.touch();
            }
            "image-replace-color" => {
                signals.set("replace_title", "Replace color");
                signals.set("replace_from", hex(editor.fg));
                signals.set("replace_to", hex(editor.bg));
                signals.set("replace_open", true);
            }
            "replace-ok" => {
                signals.set("replace_open", false);
                let read = |name: &str| signals.get::<String>(name).and_then(|t| parse_color(&t));
                let (Some(from), Some(to)) = (read("replace_from"), read("replace_to")) else {
                    editor.message = Some("Pick both colors".into());
                    continue;
                };
                let tolerance = number(&signals, "replace_tolerance", 0).clamp(0, 255) as u8;
                let frames =
                    if signals.get::<String>("replace_frames").as_deref() == Some("Every frame") {
                        0..editor.doc.frame_count()
                    } else {
                        editor.frame..editor.frame + 1
                    };
                editor
                    .history
                    .transact(&mut editor.doc, "Replace color", Scope::All, |d| {
                        replace_color_everywhere(d, from.into(), to.into(), tolerance, frames)
                    });
                editor.touch();
            }
            "replace-cancel" => signals.set("replace_open", false),
            "palette-load" => load_palette(&mut commands),
            "palette-save" => save_palette(&mut commands, editor),
            "palette-sort" => {
                signals.set("sort_title", "Sort the palette");
                signals.set("sort_open", true);
            }
            "sort-ok" => {
                signals.set("sort_open", false);
                let key = match signals.get::<String>("sort_key").as_deref() {
                    Some("Saturation") => SortKey::Saturation,
                    Some("Lightness") => SortKey::Lightness,
                    Some("Brightness") => SortKey::Luma,
                    _ => SortKey::Hue,
                };
                editor
                    .history
                    .transact(&mut editor.doc, "Sort palette", Scope::All, |d| {
                        palettes::sort_palette(d, key)
                    });
                editor.touch();
            }
            "sort-cancel" => signals.set("sort_open", false),
            "palette-ramp" => {
                signals.set("ramp_title", "A ramp around the foreground color");
                signals.set("ramp_open", true);
                state.ramp_shown = None;
            }
            "ramp-ok" => {
                signals.set("ramp_open", false);
                let mut colors = editor.doc.palette().colors().to_vec();
                colors.extend(ramp_of(editor, &signals));
                match Palette::new(colors) {
                    Ok(p) => {
                        editor
                            .history
                            .transact(&mut editor.doc, "Add ramp", Scope::All, |d| {
                                d.set_palette(p)
                            });
                        editor.touch();
                    }
                    Err(e) => editor.message = Some(e.to_string()),
                }
            }
            "ramp-cancel" => signals.set("ramp_open", false),
            "palette-from-image" => {
                signals.set("quantize_title", "A palette from the image");
                signals.set("quantize_open", true);
            }
            "quantize-ok" if state.task.is_none() => {
                let n = number(&signals, "quantize_colors", 16).clamp(2, 256) as usize;
                let indexed = editor.doc.mode() == ColorMode::Indexed;
                let frame = editor.frame;
                let task = start_palette_task(
                    editor,
                    move |d| {
                        palette_from_pixels(
                            composite_frame(d, frame, &CompositeOptions::EXPORT),
                            n,
                            indexed,
                        )
                    },
                    None,
                );
                state.task = Some(Running {
                    label: "Palette from image",
                    dialog: "quantize_open",
                    resizes: false,
                    task,
                });
            }
            "quantize-cancel" => {
                state.task = None;
                signals.set("quantize_open", false);
            }
            "palette-lock" => {
                state.lock = !state.lock;
                editor.message = Some(if state.lock {
                    "Colors keep to the palette".into()
                } else {
                    "Colors are free of the palette".into()
                });
            }
            _ => {}
        }
    }
    // Locked colors snap to the palette.
    if state.lock {
        let p = editor.doc.palette();
        let ti = (editor.doc.mode() == ColorMode::Indexed).then(|| editor.doc.transparent_index());
        let snap = |c: Rgba| {
            if p.is_empty() || p.colors().contains(&c) {
                c
            } else {
                p.get(p.nearest(c, ti))
            }
        };
        let (fg, bg) = (snap(editor.fg), snap(editor.bg));
        if (fg, bg) != (editor.fg, editor.bg) {
            editor.fg = fg;
            editor.bg = bg;
        }
    }
    // A whole-document edit: how far it is, and the document it made.
    if let Some(run) = state.task.as_mut() {
        if signals.get::<bool>(run.dialog) != Some(true) {
            // Its dialog closed some other way (Escape): it stops.
            state.task = None;
        } else if let Some(result) = run.task.poll() {
            let (label, dialog, resizes) = (run.label, run.dialog, run.resizes);
            state.task = None;
            signals.set(dialog, false);
            match result {
                Some(Ok(doc)) => {
                    editor
                        .history
                        .transact(&mut editor.doc, label, Scope::All, |d| *d = doc);
                    if resizes {
                        after_resize(editor);
                    } else {
                        editor.touch();
                    }
                }
                Some(Err(e)) => editor.message = Some(e.to_string()),
                None => {}
            }
        } else {
            let text = match run.task.progress.get() {
                _ if run.resizes => "Resampling every cel...".to_string(),
                (_, 0) => "Reading the image...".to_string(),
                (done, total) => format!("Indexing images: {done} of {total}"),
            };
            crate::tools::set_if_changed(&mut signals, "img_progress", text);
            if let Some(wake) = &wake {
                wake.request(std::time::Instant::now() + std::time::Duration::from_millis(16));
            }
        }
    }
    let busy = state.task.is_some();
    if signals.get::<bool>("img_busy") != Some(busy) {
        signals.set("img_busy", busy);
        if !busy {
            signals.set("img_progress", String::new());
        }
    }
    // The ramp builder shows its ramp as it changes.
    if signals.get::<bool>("ramp_open").unwrap_or(false) {
        let ramp = ramp_of(editor, &signals);
        if state.ramp_shown.as_ref() != Some(&ramp)
            && let Some(row) = dom::get_by_id("ramp-preview")
        {
            let markup: String = ramp
                .iter()
                .map(|c| format!(r#"<tile class="swatch" bg="{}"/>"#, hex(*c)))
                .collect();
            row.set_inner_markup(&markup);
            state.ramp_shown = Some(ramp);
            crate::repaint_soon(&wake);
        }
    }
}

/// Asks for a palette file and puts the document on it.
pub fn load_palette(commands: &mut Commands) {
    commands.queue(
        FileDialog::open()
            .filter("Palettes", &["gpl", "pal", "hex", "ase", "txt"])
            .then(|world: &mut World, paths| {
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                let mut editor = world.resource_mut::<Editor>();
                match load_palette_file(&path) {
                    Ok(p) => use_palette(&mut editor, "Load palette", p),
                    Err(e) => editor.message = Some(e),
                }
            }),
    );
}

/// Reads a palette file, its format told by its content or extension.
pub fn load_palette_file(path: &Path) -> std::result::Result<Palette, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let hint = path
        .extension()
        .and_then(|e| PaletteFormat::from_extension(&e.to_string_lossy()));
    read_palette(&bytes, hint).map_err(|e| e.to_string())
}

/// Writes the palette to `path` in the format its extension names (GIMP's
/// .gpl when it names none).
pub fn save_palette_file(palette: &Palette, path: &Path) -> std::result::Result<(), String> {
    let format = path
        .extension()
        .and_then(|e| PaletteFormat::from_extension(&e.to_string_lossy()))
        .unwrap_or(PaletteFormat::Gpl);
    let name = path
        .file_stem()
        .map_or_else(|| "pixl".to_string(), |s| s.to_string_lossy().into_owned());
    std::fs::write(path, write_palette(palette, format, &name)).map_err(|e| e.to_string())
}

fn save_palette(commands: &mut Commands, editor: &Editor) {
    let base = editor.name();
    let stem = base.rsplit_once('.').map_or(base.as_str(), |(s, _)| s);
    let name = format!("{stem}.gpl");
    commands.queue(
        FileDialog::save()
            .filter("GIMP palette", &["gpl"])
            .filter("JASC palette", &["pal"])
            .filter("Hex list", &["hex"])
            .filter("Adobe swatches", &["ase"])
            .file_name(name)
            .then(|world: &mut World, paths| {
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                let mut editor = world.resource_mut::<Editor>();
                let palette = editor.doc.palette().clone();
                if let Err(e) = save_palette_file(&palette, &path) {
                    editor.message = Some(e);
                }
            }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_place_the_old_canvas() {
        assert_eq!(anchor_offset("Top left", (4, 4), (10, 8)), Point::new(0, 0));
        assert_eq!(anchor_offset("Center", (4, 4), (10, 8)), Point::new(3, 2));
        assert_eq!(
            anchor_offset("Bottom right", (4, 4), (10, 8)),
            Point::new(6, 4)
        );
        assert_eq!(anchor_offset("Left", (4, 4), (10, 8)), Point::new(0, 2));
        assert_eq!(anchor_offset("Top", (10, 8), (4, 4)), Point::new(-3, 0));
    }

    #[test]
    fn color_fields_read() {
        assert_eq!(parse_color("#ff0000"), Some(Rgba::opaque(255, 0, 0)));
        assert_eq!(
            parse_color("color(srgb 0 1 0 / 0.5)"),
            Some(Rgba::new(0, 255, 0, 128))
        );
        assert_eq!(parse_color("color(srgb 1 1 1)"), Some(Rgba::WHITE));
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn palette_files_round_trip_through_disk() {
        let dir = std::env::temp_dir().join(format!("pixl-pal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = Palette::new(vec![Rgba::BLACK, Rgba::opaque(10, 20, 30)]).unwrap();
        for ext in ["gpl", "pal", "hex", "ase"] {
            let path = dir.join(format!("p.{ext}"));
            save_palette_file(&p, &path).unwrap();
            assert_eq!(load_palette_file(&path).unwrap(), p, "{ext}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
