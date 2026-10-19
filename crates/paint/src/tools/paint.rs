//! Painting tools: pencil, line, rectangle, ellipse, color replace.

use crate::tools::{Brush, Canvas, Ink, ToolContext};
use pixl_document::color::Color;
use pixl_document::document::Document;
use pixl_document::geom::{Point, Rect};
use pixl_document::shapes;

/// A freehand stroke fed one input point at a time, as a pointer moves.
///
/// Each new point extends the stroke with a line from the previous one. With
/// pixel-perfect on (one-pixel brushes only), the newest pixel stays pending
/// until the next point shows whether it is an L-shaped corner to drop;
/// [`PencilStroke::finish`] paints it. Radial symmetry rotates the input
/// points and rasterizes each copy as its own continuous line.
pub struct PencilStroke {
    brush: Brush,
    ink: Ink,
    pixel_perfect: bool,
    last: Option<Point>,
    copies: Vec<CopyPath>,
}

struct CopyPath {
    path: Vec<Point>,
    painted: usize,
}

impl PencilStroke {
    pub fn new(canvas: &Canvas, brush: Brush, ink: Ink, pixel_perfect: bool) -> Self {
        let pixel_perfect = pixel_perfect && brush.is_pixel();
        let copies = (0..canvas.symmetry().rotations())
            .map(|_| CopyPath {
                path: Vec::new(),
                painted: 0,
            })
            .collect();
        Self {
            brush,
            ink,
            pixel_perfect,
            last: None,
            copies,
        }
    }

    /// Extends the stroke to `p` and paints what is final.
    pub fn add(&mut self, canvas: &mut Canvas, p: Point) {
        let sym = canvas.symmetry();
        let from = self.last.unwrap_or(p);
        self.last = Some(p);
        for (k, copy) in self.copies.iter_mut().enumerate() {
            let seg = shapes::line(sym.rotate(from, k), sym.rotate(p, k));
            for q in seg {
                if self.pixel_perfect {
                    shapes::push_pixel_perfect(&mut copy.path, q);
                    // A pixel already painted is never removed: only the
                    // pending last pixel can be.
                    copy.painted = copy.painted.min(copy.path.len() - 1);
                } else if copy.path.last() != Some(&q) {
                    copy.path.push(q);
                }
            }
            let ready = if self.pixel_perfect {
                copy.path.len().saturating_sub(1)
            } else {
                copy.path.len()
            };
            for &q in &copy.path[copy.painted..ready] {
                canvas.stamp(q, &self.brush, &self.ink);
            }
            copy.painted = copy.painted.max(ready);
        }
    }

    /// Paints the pending pixels and ends the stroke.
    pub fn finish(self, canvas: &mut Canvas) {
        for copy in &self.copies {
            for &q in &copy.path[copy.painted..] {
                canvas.stamp(q, &self.brush, &self.ink);
            }
        }
    }
}

/// A whole freehand stroke through `points`.
pub fn pencil(
    canvas: &mut Canvas,
    points: &[Point],
    brush: &Brush,
    ink: &Ink,
    pixel_perfect: bool,
) {
    let mut stroke = PencilStroke::new(canvas, brush.clone(), ink.clone(), pixel_perfect);
    for &p in points {
        stroke.add(canvas, p);
    }
    stroke.finish(canvas);
}

/// A straight line (see [`shapes::line`]); pass the end through
/// [`shapes::snap_line`] for angle snapping.
pub fn line(canvas: &mut Canvas, a: Point, b: Point, brush: &Brush, ink: &Ink) {
    let sym = canvas.symmetry();
    for k in 0..sym.rotations() {
        for q in shapes::line(sym.rotate(a, k), sym.rotate(b, k)) {
            canvas.stamp(q, brush, ink);
        }
    }
}

/// A rectangle with corners `a` and `b`. The outline uses the brush; a fill
/// covers every pixel inside.
pub fn rectangle(canvas: &mut Canvas, a: Point, b: Point, filled: bool, brush: &Brush, ink: &Ink) {
    if filled {
        let r = pixl_document::geom::Rect::from_corners(a, b);
        let pixels: Vec<Point> = r.points().collect();
        canvas.stamp_all(&pixels, &Brush::PIXEL, ink);
    } else {
        canvas.stamp_all(&shapes::rect_outline(a, b), brush, ink);
    }
}

/// The ellipse inscribed in the box `a`..`b`, with a one-pixel outline that
/// has no doubled pixels (see [`shapes::ellipse_outline`]).
pub fn ellipse(canvas: &mut Canvas, a: Point, b: Point, filled: bool, brush: &Brush, ink: &Ink) {
    if filled {
        canvas.stamp_all(&shapes::ellipse_filled(a, b), &Brush::PIXEL, ink);
    } else {
        canvas.stamp_all(&shapes::ellipse_outline(a, b), brush, ink);
    }
}

/// Replaces `from` with `to` on every selected pixel of the canvas.
pub fn replace_color(canvas: &mut Canvas, from: Color, to: Color, tolerance: u8) {
    let ink = Ink::Replace {
        from,
        to,
        tolerance,
    };
    let area = match canvas.selection() {
        Some(s) => s.bounds(),
        None => canvas.bounds(),
    };
    for p in area.points() {
        if let Some(v) = canvas.ink_value(&ink, p.x, p.y) {
            canvas.write(p.x, p.y, v);
        }
    }
}

/// Replaces `from` with `to` (within `tolerance`) in every cel of every
/// raster layer whose pixels are not locked, over the frames in `frames`.
/// Linked cels change once. Returns the changed area of the canvas. Run it
/// in a [`pixl_document::History`] transaction with [`pixl_document::Scope::All`] to make it
/// one undo step.
pub fn replace_color_everywhere(
    doc: &mut Document,
    from: Color,
    to: Color,
    tolerance: u8,
    frames: std::ops::Range<usize>,
) -> Rect {
    let layers: Vec<pixl_document::LayerId> = doc
        .walk()
        .into_iter()
        .filter(|(_, l)| l.is_raster())
        .map(|(_, l)| l.id)
        .collect();
    let ctx = ToolContext::default();
    let mut done: Vec<pixl_document::ImageId> = Vec::new();
    let mut dirty = Rect::EMPTY;
    for id in layers {
        if doc.effective_locks(id).is_ok_and(|l| l.pixels) {
            continue;
        }
        let count = doc.frame_count();
        for f in frames.clone().filter(|f| *f < count) {
            let Ok(Some(cel)) = doc.cel(id, f) else {
                continue;
            };
            if done.contains(&cel.image) {
                continue;
            }
            done.push(cel.image);
            let Ok(target) = doc.target(id, f) else {
                continue;
            };
            let mut canvas = Canvas::new(target, &ctx);
            replace_color(&mut canvas, from, to, tolerance);
            dirty = dirty.union(&canvas.dirty());
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Symmetry, SymmetryMode, ToolContext, edit_cel};
    use pixl_document::color::{ColorMode, Rgba};
    use pixl_document::document::Document;
    use pixl_document::geom::Rect;
    use pixl_document::history::History;
    use pixl_document::selection::Selection;

    const W: Rgba = Rgba::WHITE;

    fn run(size: u32, ctx: ToolContext, f: impl FnOnce(&mut Canvas)) -> (Vec<Vec<bool>>, Rect) {
        let mut d = Document::new(size, size, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let mut h = History::default();
        let (_, dirty) = edit_cel(&mut d, &mut h, "t", l, 0, &ctx, f).unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        let grid = (0..size as i32)
            .map(|y| (0..size as i32).map(|x| r.get_raw(x, y) != 0).collect())
            .collect();
        (grid, dirty)
    }

    fn count(g: &[Vec<bool>]) -> usize {
        g.iter().flatten().filter(|v| **v).count()
    }

    fn paint() -> Ink {
        Ink::Paint(W.into())
    }

    #[test]
    fn pixel_perfect_pencil_drops_corners() {
        let pts = [
            Point::new(0, 0),
            Point::new(1, 0),
            Point::new(1, 1),
            Point::new(2, 1),
            Point::new(2, 2),
        ];
        let (plain, _) = run(8, ToolContext::default(), |c| {
            pencil(c, &pts, &Brush::PIXEL, &paint(), false)
        });
        assert_eq!(count(&plain), 5);
        let (pp, dirty) = run(8, ToolContext::default(), |c| {
            pencil(c, &pts, &Brush::PIXEL, &paint(), true)
        });
        assert_eq!(count(&pp), 3);
        assert!(pp[0][0] && pp[1][1] && pp[2][2]);
        assert_eq!(dirty, Rect::new(0, 0, 3, 3));
    }

    #[test]
    fn incremental_stroke_matches_whole_stroke() {
        let pts: Vec<Point> = (0..40)
            .map(|i| Point::new(5 + (i * 7) % 50, 5 + (i * 13) % 50))
            .collect();
        let (whole, _) = run(64, ToolContext::default(), |c| {
            let filtered = shapes::pixel_perfect(&shapes::polyline(&pts));
            for p in filtered {
                c.plot(p, &paint());
            }
        });
        let (inc, _) = run(64, ToolContext::default(), |c| {
            pencil(c, &pts, &Brush::PIXEL, &paint(), true)
        });
        assert_eq!(whole, inc);
    }

    #[test]
    fn brush_size_widens_strokes() {
        let (g, dirty) = run(16, ToolContext::default(), |c| {
            line(
                c,
                Point::new(4, 8),
                Point::new(10, 8),
                &Brush::square(3),
                &paint(),
            )
        });
        assert_eq!(count(&g), 7 * 3 + 2 * 3);
        assert_eq!(dirty, Rect::new(3, 7, 9, 3));
    }

    #[test]
    fn mirror_symmetry_is_exact() {
        let ctx = ToolContext {
            symmetry: Symmetry::centered(SymmetryMode::Both, 16, 16),
            ..Default::default()
        };
        let (g, _) = run(16, ctx, |c| {
            line(
                c,
                Point::new(1, 2),
                Point::new(6, 5),
                &Brush::PIXEL,
                &paint(),
            )
        });
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(g[y][x], g[y][15 - x]);
                assert_eq!(g[y][x], g[15 - y][x]);
            }
        }
        assert_eq!(count(&g), 4 * 6);
    }

    #[test]
    fn radial_symmetry_keeps_lines_continuous() {
        let ctx = ToolContext {
            symmetry: Symmetry::centered(SymmetryMode::Radial(6), 64, 64),
            ..Default::default()
        };
        let (g, _) = run(64, ctx, |c| {
            line(
                c,
                Point::new(32, 32),
                Point::new(60, 32),
                &Brush::PIXEL,
                &paint(),
            )
        });
        // Six spokes of 29 pixels; they overlap near the center.
        let n = count(&g);
        assert!(n > 6 * 24, "{n}");
        let sym = Symmetry::centered(SymmetryMode::Radial(6), 64, 64);
        for k in 0..6 {
            let end = sym.rotate(Point::new(60, 32), k);
            assert!(g[end.y as usize][end.x as usize], "spoke {k}");
        }
        assert!(g[32][60]);
    }

    #[test]
    fn shapes_fill_and_outline() {
        let (rect, _) = run(16, ToolContext::default(), |c| {
            rectangle(
                c,
                Point::new(2, 2),
                Point::new(7, 5),
                false,
                &Brush::PIXEL,
                &paint(),
            )
        });
        assert_eq!(count(&rect), 6 * 2 + 2 * 2);
        let (filled, _) = run(16, ToolContext::default(), |c| {
            rectangle(
                c,
                Point::new(2, 2),
                Point::new(7, 5),
                true,
                &Brush::PIXEL,
                &paint(),
            )
        });
        assert_eq!(count(&filled), 24);
        let (e, _) = run(16, ToolContext::default(), |c| {
            ellipse(
                c,
                Point::new(0, 0),
                Point::new(9, 6),
                false,
                &Brush::PIXEL,
                &paint(),
            )
        });
        let (ef, _) = run(16, ToolContext::default(), |c| {
            ellipse(
                c,
                Point::new(0, 0),
                Point::new(9, 6),
                true,
                &Brush::PIXEL,
                &paint(),
            )
        });
        assert!(count(&ef) > count(&e));
        for y in 0..16 {
            for x in 0..16 {
                if e[y][x] {
                    assert!(ef[y][x], "the fill covers the outline");
                }
            }
        }
    }

    #[test]
    fn selection_limits_every_tool() {
        let ctx = ToolContext {
            selection: Some(Selection::rect(16, 16, Rect::new(0, 0, 8, 16))),
            ..Default::default()
        };
        let (g, dirty) = run(16, ctx, |c| {
            rectangle(
                c,
                Point::new(0, 0),
                Point::new(15, 15),
                true,
                &Brush::PIXEL,
                &paint(),
            );
            pencil(
                c,
                &[Point::new(0, 3), Point::new(15, 3)],
                &Brush::circle(3),
                &paint(),
                true,
            );
        });
        assert_eq!(count(&g), 8 * 16);
        assert_eq!(dirty, Rect::new(0, 0, 8, 16));
    }

    #[test]
    fn replace_color_everywhere() {
        let (g, _) = run(8, ToolContext::default(), |c| {
            c.plot(Point::new(1, 1), &paint());
            c.plot(
                Point::new(5, 5),
                &Ink::Paint(Rgba::new(250, 255, 255, 255).into()),
            );
            c.plot(Point::new(6, 6), &Ink::Paint(Rgba::opaque(9, 9, 9).into()));
            replace_color(c, W.into(), Rgba::TRANSPARENT.into(), 8);
        });
        assert_eq!(count(&g), 1);
        assert!(g[6][6]);
    }

    #[test]
    fn replacing_everywhere_skips_locked_layers_and_other_frames() {
        let mut d = Document::new(4, 4, ColorMode::Rgba).unwrap();
        let a = d.layers()[0].id;
        let b = d
            .add_layer(
                pixl_document::NewLayer::Raster,
                "b",
                pixl_document::Placement::default(),
            )
            .unwrap();
        d.insert_frame(1, 100).unwrap();
        let red = Rgba::opaque(255, 0, 0);
        for (l, f) in [(a, 0), (b, 0), (a, 1)] {
            d.target(l, f).unwrap().raster.set_raw(1, 1, red.to_u32());
        }
        d.update_layer(b, |p| p.locks.pixels = true).unwrap();
        let dirty =
            super::replace_color_everywhere(&mut d, red.into(), Rgba::WHITE.into(), 0, 0..1);
        assert_eq!(dirty, Rect::pixel(1, 1));
        let px = |d: &Document, l, f| d.cel_raster(l, f).unwrap().unwrap().get_raw(1, 1);
        assert_eq!(px(&d, a, 0), Rgba::WHITE.to_u32());
        assert_eq!(px(&d, b, 0), red.to_u32(), "locked");
        assert_eq!(px(&d, a, 1), red.to_u32(), "outside the range");
    }
}
