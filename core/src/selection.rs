//! Selection masks. A selection is a hard-edged set of pixels (pixel art has
//! no feathering by default); every tool limits its writes to it.

use crate::geom::{Point, Rect};
use crate::image::TiledImage;
use crate::tools::shapes;

/// How a new selection shape combines with the current one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectOp {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// A set of selected pixels on a canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    mask: TiledImage<bool>,
}

impl Selection {
    /// Nothing selected.
    pub fn empty(width: u32, height: u32) -> Self {
        Self {
            mask: TiledImage::new(width, height, false),
        }
    }

    /// The whole canvas.
    pub fn all(width: u32, height: u32) -> Self {
        Self {
            mask: TiledImage::new(width, height, true),
        }
    }

    /// The pixels of `points` that fall on the canvas.
    pub fn from_points(width: u32, height: u32, points: impl IntoIterator<Item = Point>) -> Self {
        let mut s = Self::empty(width, height);
        for p in points {
            s.mask.set(p.x, p.y, true);
        }
        s
    }

    pub fn rect(width: u32, height: u32, rect: Rect) -> Self {
        let mut s = Self::empty(width, height);
        s.mask.fill_rect(&rect, true);
        s
    }

    /// The filled ellipse inscribed in `rect`.
    pub fn ellipse(width: u32, height: u32, rect: Rect) -> Self {
        if rect.is_empty() {
            return Self::empty(width, height);
        }
        let a = Point::new(rect.x, rect.y);
        let b = Point::new(rect.right() - 1, rect.bottom() - 1);
        Self::from_points(width, height, shapes::ellipse_filled(a, b))
    }

    /// The inside and outline of a closed freehand path.
    pub fn lasso(width: u32, height: u32, points: &[Point]) -> Self {
        Self::from_points(width, height, shapes::polygon_filled(points))
    }

    pub fn width(&self) -> u32 {
        self.mask.width()
    }

    pub fn height(&self) -> u32 {
        self.mask.height()
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.mask.get(x, y)
    }

    pub fn set(&mut self, x: i32, y: i32, selected: bool) {
        self.mask.set(x, y, selected);
    }

    /// The bounding box of selected pixels.
    pub fn bounds(&self) -> Rect {
        if self.mask.fill() {
            return self.mask.bounds();
        }
        self.mask.content_bounds()
    }

    pub fn is_empty(&self) -> bool {
        self.bounds().is_empty()
    }

    /// Selected pixel count.
    pub fn count(&self) -> usize {
        self.bounds()
            .points()
            .filter(|p| self.contains(p.x, p.y))
            .count()
    }

    /// Applies `other` with `op`.
    pub fn combine(&mut self, other: &Selection, op: SelectOp) {
        match op {
            SelectOp::Replace => *self = other.clone(),
            SelectOp::Add => {
                for p in other.bounds().points() {
                    if other.contains(p.x, p.y) {
                        self.mask.set(p.x, p.y, true);
                    }
                }
            }
            SelectOp::Subtract => {
                for p in other.bounds().points() {
                    if other.contains(p.x, p.y) {
                        self.mask.set(p.x, p.y, false);
                    }
                }
            }
            SelectOp::Intersect => {
                for p in self.bounds().points() {
                    if !other.contains(p.x, p.y) {
                        self.mask.set(p.x, p.y, false);
                    }
                }
            }
        }
    }

    /// Selects what was not selected.
    pub fn invert(&mut self) {
        let fill = !self.mask.fill();
        let (w, h) = (self.width(), self.height());
        let old = std::mem::replace(&mut self.mask, TiledImage::new(w, h, fill));
        for p in old.bounds().points() {
            if old.get(p.x, p.y) == fill {
                self.mask.set(p.x, p.y, !fill);
            }
        }
        if fill {
            // Keep "everything but a few pixels" cheap to store.
            self.mask.compact();
        }
    }

    /// Grows the selection by `n` pixels in every direction, square corners
    /// (a pixel joins if any pixel within Chebyshev distance `n` is selected).
    pub fn grow(&mut self, n: u32) {
        self.morph(n as i32, true);
    }

    /// Shrinks the selection by `n` pixels (a pixel stays only if every pixel
    /// within Chebyshev distance `n` is selected; off-canvas counts as
    /// unselected).
    pub fn shrink(&mut self, n: u32) {
        self.morph(n as i32, false);
    }

    fn morph(&mut self, n: i32, grow: bool) {
        if n == 0 {
            return;
        }
        let canvas = self.mask.bounds();
        let area = if grow {
            self.bounds().inflate(n).intersect(&canvas)
        } else {
            self.bounds()
        };
        if area.is_empty() {
            return;
        }
        // Separable max (grow) or min (shrink) filter: rows, then columns.
        let (w, h) = (area.w as usize, area.h as usize);
        let get = |x: i32, y: i32| canvas.contains(x, y) && self.contains(x, y);
        let mut rows = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let (cx, cy) = (area.x + x as i32, area.y + y as i32);
                let mut v = !grow;
                for d in -n..=n {
                    let s = get(cx + d, cy);
                    if grow && s {
                        v = true;
                        break;
                    }
                    if !grow && !s {
                        v = false;
                        break;
                    }
                }
                rows[y * w + x] = v;
            }
        }
        let row_at = |x: i32, y: i32| -> bool {
            let (lx, ly) = (x - area.x, y - area.y);
            if lx < 0 || ly < 0 || lx >= area.w || ly >= area.h {
                // Outside the work area: nothing selected nearby when growing;
                // for shrinking the area is the selection's bounds, so outside
                // is unselected too.
                return false;
            }
            rows[ly as usize * w + lx as usize]
        };
        let mut out = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let (cx, cy) = (area.x + x as i32, area.y + y as i32);
                let mut v = !grow;
                for d in -n..=n {
                    let s = row_at(cx, cy + d);
                    if grow && s {
                        v = true;
                        break;
                    }
                    if !grow && !s {
                        v = false;
                        break;
                    }
                }
                out[y * w + x] = v;
            }
        }
        for y in 0..h {
            for x in 0..w {
                self.mask
                    .set(area.x + x as i32, area.y + y as i32, out[y * w + x]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_and_ops() {
        let mut s = Selection::rect(20, 20, Rect::new(2, 2, 6, 6));
        assert_eq!(s.count(), 36);
        s.combine(
            &Selection::rect(20, 20, Rect::new(5, 5, 6, 6)),
            SelectOp::Add,
        );
        assert_eq!(s.count(), 36 + 36 - 9);
        s.combine(
            &Selection::rect(20, 20, Rect::new(0, 0, 4, 20)),
            SelectOp::Subtract,
        );
        assert!(!s.contains(3, 3) && s.contains(4, 4));
        s.combine(
            &Selection::rect(20, 20, Rect::new(4, 4, 2, 2)),
            SelectOp::Intersect,
        );
        assert_eq!(s.count(), 4);
        assert_eq!(s.bounds(), Rect::new(4, 4, 2, 2));
        s.combine(
            &Selection::rect(20, 20, Rect::new(0, 0, 1, 1)),
            SelectOp::Replace,
        );
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn ellipse_and_lasso() {
        let e = Selection::ellipse(32, 32, Rect::new(0, 0, 9, 9));
        assert!(e.contains(4, 4) && e.contains(4, 0) && !e.contains(0, 0));
        let l = Selection::lasso(
            32,
            32,
            &[
                Point::new(1, 1),
                Point::new(10, 1),
                Point::new(10, 10),
                Point::new(1, 10),
            ],
        );
        assert_eq!(l.count(), 100);
    }

    #[test]
    fn invert_round_trips() {
        let mut s = Selection::rect(100, 100, Rect::new(10, 10, 5, 5));
        s.invert();
        assert_eq!(s.count(), 100 * 100 - 25);
        assert!(!s.contains(12, 12) && s.contains(0, 0));
        s.invert();
        assert_eq!(s, Selection::rect(100, 100, Rect::new(10, 10, 5, 5)));
    }

    #[test]
    fn grow_and_shrink_are_square() {
        let mut s = Selection::rect(30, 30, Rect::pixel(10, 10));
        s.grow(2);
        assert_eq!(s.bounds(), Rect::new(8, 8, 5, 5));
        assert_eq!(s.count(), 25);
        s.shrink(1);
        assert_eq!(s.bounds(), Rect::new(9, 9, 3, 3));
        s.shrink(5);
        assert!(s.is_empty());
        let mut edge = Selection::rect(30, 30, Rect::new(0, 0, 4, 4));
        edge.grow(3);
        assert_eq!(
            edge.bounds(),
            Rect::new(0, 0, 7, 7),
            "clipped to the canvas"
        );
    }
}
