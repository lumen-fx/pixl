//! The Filter menu's adjustments and filters, previewed live on the active
//! cel and applied as one undo step.
//!
//! A filter renders tile by tile from the cel as it was, on worker threads,
//! nearest the middle of the view first. Each finished tile is written into
//! the cel on the next tick, so the canvas fills in as tiles finish, the
//! editor stays responsive on a large document, and a change of a setting
//! (or Cancel) stops the work at the next tile.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_document::history::Transaction;
use pixl_document::{ImageId, LayerId, Point, Raster, Rect, Rgba, Scope};
use pixl_paint::filters::{Filter, FilterInput, write_values};
use pixl_paint::tools::fill::Connectivity;

use crate::editor::Editor;
use crate::menus::Chosen;
use crate::view::View;
use crate::view::jobs::{TileJob, WRITE_BUDGET, tiles_of};

/// The Filter menu's items: command id, dialog title, and whether the
/// filter has settings to show (the others apply at once).
pub const FILTERS: [(&str, &str, bool); 13] = [
    ("filter-hue", "Hue and saturation", true),
    ("filter-brightness", "Brightness and contrast", true),
    ("filter-levels", "Levels", true),
    ("filter-curves", "Curves", true),
    ("filter-balance", "Color balance", true),
    ("filter-posterize", "Posterize", true),
    ("filter-invert", "Invert", false),
    ("filter-desaturate", "Desaturate", false),
    ("filter-gradient-map", "Gradient map", true),
    ("filter-outline", "Outline", true),
    ("filter-shadow", "Drop shadow", true),
    ("filter-noise", "Noise", true),
    ("filter-pixelate", "Pixelate", true),
];

/// A filter dialog in progress, or a filter finishing after Apply.
struct Session {
    id: &'static str,
    title: &'static str,
    layer: LayerId,
    frame: usize,
    tx: Option<Transaction>,
    /// The cel as it was before the filter.
    original: Raster,
    /// The filter the job renders, and the job.
    job: Option<(Filter, TileJob)>,
    /// Apply was chosen: commit once the job is done.
    applying: bool,
}

/// The filter in progress, if any.
#[derive(Resource, Default)]
pub struct Adjust {
    session: Option<Session>,
}

impl Adjust {
    /// Whether a filter is open or still finishing.
    pub fn active(&self) -> bool {
        self.session.is_some()
    }

    /// Tiles still to render, for tests and the progress line.
    pub fn pending(&self) -> usize {
        self.session
            .as_ref()
            .and_then(|s| s.job.as_ref())
            .map_or(0, |(_, j)| j.total - j.done)
    }
}

fn num(signals: &Signals, name: &str, default: f64) -> f64 {
    signals.get::<f64>(name).unwrap_or(default)
}

/// The filter a dialog's settings describe.
fn filter_of(id: &str, editor: &Editor, signals: &Signals) -> Option<Filter> {
    let n = |name: &str, d: f64| num(signals, name, d);
    let flag = |name: &str, d: bool| signals.get::<bool>(name).unwrap_or(d);
    Some(match id {
        "filter-hue" => Filter::HueSaturation {
            hue: n("flt_hue", 0.0) as f32,
            saturation: n("flt_sat", 0.0) as f32,
            lightness: n("flt_light", 0.0) as f32,
        },
        "filter-brightness" => Filter::BrightnessContrast {
            brightness: n("flt_bright", 0.0) as f32,
            contrast: n("flt_contrast", 0.0) as f32,
        },
        "filter-levels" => Filter::Levels {
            input: (n("flt_in_lo", 0.0) as u8, n("flt_in_hi", 255.0) as u8),
            gamma: n("flt_gamma", 100.0) as f32 / 100.0,
            output: (n("flt_out_lo", 0.0) as u8, n("flt_out_hi", 255.0) as u8),
        },
        "filter-curves" => Filter::Curves {
            points: parse_points(&signals.get::<String>("flt_points").unwrap_or_default()),
        },
        "filter-balance" => {
            let tone = |t: &str| {
                [
                    n(&format!("flt_{t}_r"), 0.0) as f32,
                    n(&format!("flt_{t}_g"), 0.0) as f32,
                    n(&format!("flt_{t}_b"), 0.0) as f32,
                ]
            };
            Filter::ColorBalance {
                shadows: tone("sh"),
                midtones: tone("mid"),
                highlights: tone("hi"),
            }
        }
        "filter-posterize" => Filter::Posterize {
            levels: n("flt_levels", 4.0).clamp(2.0, 255.0) as u8,
        },
        "filter-invert" => Filter::Invert,
        "filter-desaturate" => Filter::Desaturate,
        "filter-gradient-map" => {
            let palette = signals.get::<String>("flt_map_colors").as_deref() == Some("Palette");
            Filter::GradientMap {
                stops: map_stops(editor, palette),
            }
        }
        "filter-outline" => Filter::Outline {
            color: editor.fg,
            outside: flag("flt_outside", true),
            connectivity: if flag("flt_diagonal", false) {
                Connectivity::Eight
            } else {
                Connectivity::Four
            },
        },
        "filter-shadow" => Filter::DropShadow {
            offset: Point::new(n("flt_dx", 1.0) as i32, n("flt_dy", 1.0) as i32),
            color: editor.fg,
        },
        "filter-noise" => Filter::Noise {
            amount: n("flt_amount", 16.0).clamp(0.0, 255.0) as u8,
            monochrome: flag("flt_mono", true),
            seed: n("flt_seed", 1.0) as u32,
        },
        "filter-pixelate" => Filter::Pixelate {
            size: n("flt_size", 4.0).clamp(1.0, 256.0) as u32,
        },
        _ => return None,
    })
}

/// Reads `input:output` pairs, such as `64:48 192:208`.
pub fn parse_points(text: &str) -> Vec<(u8, u8)> {
    text.split([' ', ','])
        .filter_map(|pair| {
            let (a, b) = pair.split_once(':')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        })
        .collect()
}

/// The gradient map's stops: the foreground to the background, or the
/// palette's colors between them.
fn map_stops(editor: &Editor, palette: bool) -> Vec<Rgba> {
    let p = editor.doc.palette().colors();
    let at = |c: Rgba| p.iter().position(|x| *x == c && c.a > 0);
    match (palette, at(editor.fg), at(editor.bg)) {
        (true, Some(i), Some(j)) if i <= j => p[i..=j].to_vec(),
        (true, Some(i), Some(j)) => p[j..=i].iter().rev().copied().collect(),
        _ => vec![editor.fg, editor.bg],
    }
}

/// The area a filter can change: the selection, or the cel's content grown
/// by how far the filter reaches past it.
fn area(editor: &Editor, original: &Raster, filter: &Filter) -> Rect {
    let doc = editor.doc.bounds();
    if let Some(s) = &editor.selection {
        return s.bounds().intersect(&doc);
    }
    let reach = match filter {
        Filter::Outline { .. } => 1,
        Filter::DropShadow { offset, .. } => offset.x.abs().max(offset.y.abs()),
        Filter::Pixelate { size } => *size as i32,
        _ => 0,
    };
    let content = original.content_bounds();
    if content.is_empty() {
        return Rect::EMPTY;
    }
    content.inflate(reach).intersect(&doc)
}

/// Puts the cel back as it was before the filter.
fn restore(editor: &mut Editor, s: &Session) {
    if let Ok(t) = editor.doc.target(s.layer, s.frame) {
        *t.raster = s.original.clone();
    }
    editor.touch_pixels();
}

/// Starts (or restarts) the session's job for `filter`.
fn run(editor: &mut Editor, s: &mut Session, filter: Filter, focus: (f64, f64)) {
    s.job = None;
    restore(editor, s);
    let input = FilterInput {
        raster: s.original.clone(),
        palette: editor.doc.palette().clone(),
        transparent_index: editor.doc.transparent_index(),
        selection: editor.selection.clone(),
    };
    let tiles = tiles_of(area(editor, &s.original, &filter), focus);
    let prepared = Arc::new(filter.prepare());
    let input = Arc::new(input);
    let job = TileJob::start(tiles, move || {
        let (prepared, input) = (Arc::clone(&prepared), Arc::clone(&input));
        move |rect: Rect| prepared.render(&input, rect)
    });
    s.job = Some((filter, job));
}

/// Opens a filter: its dialog for one with settings, straight to work for
/// one without.
fn open(editor: &mut Editor, id: &'static str, title: &'static str) -> Option<Session> {
    let (layer, frame) = (editor.layer, editor.frame);
    let locked = editor.doc.effective_locks(layer).map(|l| l.pixels);
    if locked != Ok(false) {
        editor.message = Some(match locked {
            Ok(true) => "The layer is locked".into(),
            _ => "Pick a raster layer to filter".into(),
        });
        return None;
    }
    // An empty frame has nothing to filter, and no cel is made for it.
    if matches!(editor.doc.cel(layer, frame), Ok(None)) {
        editor.message = Some("The frame is empty on this layer".into());
        return None;
    }
    let image: ImageId = match editor.doc.target_image(layer, frame) {
        Ok(i) => i,
        Err(e) => {
            editor.message = Some(e.to_string());
            return None;
        }
    };
    let original = editor.doc.image(image)?.clone();
    let mut tx = editor
        .history
        .begin(&editor.doc, title, Scope::Images(vec![image]));
    tx.watch(&editor.doc, image);
    Some(Session {
        id,
        title,
        layer,
        frame,
        tx: Some(tx),
        original,
        job: None,
        applying: false,
    })
}

/// Opens filters from the menu, keeps the preview in step with the
/// dialog's settings, writes finished tiles, and applies or cancels.
pub fn filter_commands(
    chosen: Res<Chosen>,
    mut editor: ResMut<Editor>,
    mut adjust: ResMut<Adjust>,
    view: Res<View>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let editor = &mut *editor;
    let focus = view.to_doc(view.middle());
    for id in &chosen.0 {
        if let Some((fid, title, settings)) = FILTERS.iter().find(|(f, _, _)| f == id) {
            if adjust.session.is_some() {
                continue;
            }
            let Some(mut s) = open(editor, fid, title) else {
                continue;
            };
            if *settings {
                signals.set("flt_title", *title);
                for (other, _, _) in FILTERS {
                    let key = format!(
                        "flt_show_{}",
                        other.trim_start_matches("filter-").replace('-', "_")
                    );
                    signals.set(&key, other == *fid);
                }
                signals.set("flt_progress", String::new());
                signals.set("filter_open", true);
            } else {
                s.applying = true;
                if let Some(f) = filter_of(fid, editor, &signals) {
                    run(editor, &mut s, f, focus);
                }
            }
            adjust.session = Some(s);
            continue;
        }
        match id.as_str() {
            "flt-ok" => {
                signals.set("filter_open", false);
                if let Some(s) = adjust.session.as_mut() {
                    s.applying = true;
                    // Apply what the settings say even when the preview was
                    // off.
                    if let Some(f) = filter_of(s.id, editor, &signals)
                        && s.job.as_ref().is_none_or(|(g, _)| *g != f)
                    {
                        run(editor, s, f, focus);
                    }
                }
            }
            "flt-cancel" => {
                signals.set("filter_open", false);
                if let Some(s) = adjust.session.take() {
                    if let Some((_, job)) = &s.job {
                        job.stop();
                    }
                    restore(editor, &s);
                }
            }
            _ => {}
        }
    }
    let Some(mut s) = adjust.session.take() else {
        return;
    };
    // The preview follows the settings.
    if !s.applying {
        let preview = signals.get::<bool>("flt_preview").unwrap_or(true);
        let want = filter_of(s.id, editor, &signals);
        match (preview, want) {
            (true, Some(f)) if s.job.as_ref().is_none_or(|(g, _)| *g != f) => {
                run(editor, &mut s, f, focus);
            }
            (false, _) if s.job.is_some() => {
                s.job = None;
                restore(editor, &s);
            }
            _ => {}
        }
    }
    // Write what the workers finished.
    let mut wrote = false;
    if let Some((_, job)) = s.job.as_mut() {
        let until = (!s.applying).then(|| Instant::now() + WRITE_BUDGET);
        let (layer, frame) = (s.layer, s.frame);
        wrote = job.drain(until, |rect, values| {
            if let Ok(t) = editor.doc.target(layer, frame) {
                write_values(t.raster, rect, values);
            }
        });
        crate::tools::set_if_changed(&mut signals, "flt_progress", job.progress());
    }
    if wrote {
        editor.touch_pixels();
    }
    let finished = s.job.as_ref().is_none_or(|(_, j)| j.complete());
    if s.applying && finished {
        if let Some(tx) = s.tx.take() {
            editor.history.commit(&mut editor.doc, tx);
        }
        editor.touch_pixels();
        let _ = s.title;
        return;
    }
    if !finished && let Some(wake) = &wake {
        wake.request(Instant::now() + Duration::from_millis(4));
    }
    adjust.session = Some(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_points_read() {
        assert_eq!(parse_points("64:48 192:208"), vec![(64, 48), (192, 208)]);
        assert_eq!(parse_points("0:0, 300:1 x"), vec![(0, 0)]);
    }
}
