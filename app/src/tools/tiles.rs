//! The tools in Tiles mode, on a tilemap layer: the pencil and the brush
//! place the picked tile cell by cell along a drag, the eraser empties
//! cells, the bucket fills joined cells that hold the same tile, the
//! rectangle fills a block of cells, and the eyedropper picks the tile under
//! the pointer. The other painting tools wait for Pixels mode.

use lumenui::lumen_core::input::PointerButton;
use pixl_document::history::Transaction;
use pixl_document::{Point, Rect, TileRef, shapes};

use crate::editor::{Editor, Tool};
use crate::tools::refuse;
use crate::view::Preview;

/// A tile gesture in progress.
pub(crate) enum TileSession {
    /// Placing one tile along a drag, one undo step.
    Place {
        button: PointerButton,
        tile: TileRef,
        tx: Option<Box<Transaction>>,
        last: Option<Point>,
    },
    /// A block of cells from the press's cell to the pointer's.
    Rect {
        button: PointerButton,
        tile: TileRef,
        from: Point,
        to: Point,
    },
    /// Picking tiles while the button is down.
    Pick { button: PointerButton },
}

impl TileSession {
    pub(crate) fn button(&self) -> PointerButton {
        match self {
            TileSession::Place { button, .. }
            | TileSession::Rect { button, .. }
            | TileSession::Pick { button } => *button,
        }
    }
}

/// Whether a tool places tiles in Tiles mode.
pub fn places_tiles(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Pencil
            | Tool::Brush
            | Tool::Eraser
            | Tool::Bucket
            | Tool::Rectangle
            | Tool::Eyedropper
    )
}

/// The tile a button places: the picked one for the primary button, the
/// empty tile for the secondary button and the eraser.
fn tile_for(editor: &Editor, tool: Tool, button: PointerButton) -> TileRef {
    if tool == Tool::Eraser || button == PointerButton::Secondary {
        TileRef::EMPTY
    } else {
        editor.tile
    }
}

fn cell(editor: &Editor, p: Point) -> Option<Point> {
    editor
        .doc
        .tile_cell(editor.layer, editor.frame, p)
        .ok()
        .flatten()
}

/// Starts a tile gesture with `tool` at document pixel `p`; `None` when the
/// tool does not place tiles or is refused.
pub(crate) fn begin(
    editor: &mut Editor,
    preview: &mut Preview,
    tool: Tool,
    p: Point,
    button: PointerButton,
) -> Option<TileSession> {
    let tile = tile_for(editor, tool, button);
    match tool {
        Tool::Pencil | Tool::Brush | Tool::Eraser => {
            let label = if tile.is_empty() {
                "Erase tiles"
            } else {
                "Place tiles"
            };
            let (_, tx) = match editor.begin_paint(label) {
                Ok(started) => started,
                Err(e) => {
                    refuse(editor, e);
                    return None;
                }
            };
            let mut s = TileSession::Place {
                button,
                tile,
                tx: Some(Box::new(tx)),
                last: None,
            };
            extend(editor, preview, &mut s, p);
            Some(s)
        }
        Tool::Rectangle => {
            let c = cell(editor, p)?;
            let s = TileSession::Rect {
                button,
                tile,
                from: c,
                to: c,
            };
            show_rect(editor, preview, &s);
            Some(s)
        }
        Tool::Bucket => {
            let c = cell(editor, p)?;
            let (_, tx) = match editor.begin_paint("Fill tiles") {
                Ok(started) => started,
                Err(e) => {
                    refuse(editor, e);
                    return None;
                }
            };
            let contiguous = editor.options.contiguous;
            let (layer, frame) = (editor.layer, editor.frame);
            match editor.doc.fill_tiles(layer, frame, c, tile, contiguous) {
                Ok(_) => {
                    editor.history.commit(&mut editor.doc, tx);
                    editor.touch_pixels();
                }
                Err(e) => {
                    editor.history.rollback(&mut editor.doc, tx);
                    refuse(editor, e);
                }
            }
            None
        }
        Tool::Eyedropper => {
            pick(editor, p);
            Some(TileSession::Pick { button })
        }
        _ => None,
    }
}

pub(crate) fn extend(editor: &mut Editor, preview: &mut Preview, s: &mut TileSession, p: Point) {
    match s {
        TileSession::Place { tile, last, .. } => {
            let Some(c) = cell(editor, p) else {
                return;
            };
            if *last == Some(c) {
                return;
            }
            // Every cell between the last one and this, so a fast drag
            // leaves no gaps.
            let cells = match *last {
                Some(l) => shapes::line(l, c),
                None => vec![c],
            };
            *last = Some(c);
            let (layer, frame) = (editor.layer, editor.frame);
            match editor.doc.place_tiles(layer, frame, &cells, *tile) {
                Ok(dirty) if !dirty.is_empty() => editor.touch_pixels(),
                Ok(_) => {}
                Err(e) => refuse(editor, e),
            }
        }
        TileSession::Rect { to, .. } => {
            if let Some(c) = cell(editor, p)
                && *to != c
            {
                *to = c;
                show_rect(editor, preview, s);
            }
        }
        TileSession::Pick { .. } => pick(editor, p),
    }
}

pub(crate) fn finish(editor: &mut Editor, preview: &mut Preview, s: TileSession, p: Point) {
    match s {
        TileSession::Place { mut tx, .. } => {
            if let Some(tx) = tx.take() {
                editor.history.commit(&mut editor.doc, *tx);
                editor.touch_pixels();
            }
        }
        TileSession::Rect {
            tile, from, mut to, ..
        } => {
            preview.clear();
            if let Some(c) = cell(editor, p) {
                to = c;
            }
            let (_, tx) = match editor.begin_paint("Tile rectangle") {
                Ok(started) => started,
                Err(e) => {
                    refuse(editor, e);
                    return;
                }
            };
            let (layer, frame) = (editor.layer, editor.frame);
            let cells = Rect::from_corners(from, to);
            match editor.doc.fill_tile_rect(layer, frame, cells, tile) {
                Ok(_) => {
                    editor.history.commit(&mut editor.doc, tx);
                    editor.touch_pixels();
                }
                Err(e) => {
                    editor.history.rollback(&mut editor.doc, tx);
                    refuse(editor, e);
                }
            }
        }
        TileSession::Pick { .. } => {}
    }
}

/// Picks the tile under `p`, with its flips.
fn pick(editor: &mut Editor, p: Point) {
    if let Ok(Some((_, t))) = editor.doc.tile_at(editor.layer, editor.frame, p) {
        editor.tile = t;
    }
}

/// Outlines the block of cells a rectangle covers.
fn show_rect(editor: &Editor, preview: &mut Preview, s: &TileSession) {
    let TileSession::Rect { from, to, .. } = s else {
        return;
    };
    let Ok(ts) = editor.doc.layer_tileset(editor.layer) else {
        return;
    };
    let Some(size) = editor.doc.tileset(ts).map(|t| t.tile_size()) else {
        return;
    };
    let origin = editor
        .doc
        .tile_grid(editor.layer, editor.frame)
        .ok()
        .flatten()
        .map_or(Point::new(0, 0), |g| g.origin);
    let cells = Rect::from_corners(*from, *to);
    let (tw, th) = (size.0 as i32, size.1 as i32);
    let a = Point::new(origin.x + cells.x * tw, origin.y + cells.y * th);
    let b = Point::new(a.x + cells.w * tw - 1, a.y + cells.h * th - 1);
    preview.set(shapes::rect_outline(a, b), editor.fg);
}
