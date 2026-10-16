//! Integer geometry on the pixel grid.

/// A pixel coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

impl From<(i32, i32)> for Point {
    fn from((x, y): (i32, i32)) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle of pixels. `x, y` is the top-left pixel; a
/// rectangle with a zero or negative width or height is empty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// The empty rectangle.
    pub const EMPTY: Rect = Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// The rectangle spanning two corner pixels, both included, in any order.
    pub fn from_corners(a: Point, b: Point) -> Self {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Self::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }

    /// A 1x1 rectangle covering one pixel.
    pub const fn pixel(x: i32, y: i32) -> Self {
        Self::new(x, y, 1, 1)
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// One past the rightmost column.
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    /// One past the bottom row.
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub const fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            self.w as i64 * self.h as i64
        }
    }

    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    /// The overlap of two rectangles, or [`Rect::EMPTY`].
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            Rect::EMPTY
        } else {
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        }
    }

    /// The smallest rectangle covering both. Empty inputs are ignored.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Grows the rectangle to cover the pixel `(x, y)`.
    pub fn include(&mut self, x: i32, y: i32) {
        *self = self.union(&Rect::pixel(x, y));
    }

    /// The rectangle grown by `n` pixels on every side (shrunk if negative).
    pub fn inflate(&self, n: i32) -> Rect {
        Rect::new(self.x - n, self.y - n, self.w + 2 * n, self.h + 2 * n)
    }

    /// Iterates every pixel, row by row.
    pub fn points(&self) -> impl Iterator<Item = Point> + use<> {
        let r = *self;
        let (w, h) = (r.w.max(0), r.h.max(0));
        (0..h).flat_map(move |dy| (0..w).map(move |dx| Point::new(r.x + dx, r.y + dy)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_and_union() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Rect::new(5, 5, 5, 5));
        assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
        assert!(a.intersect(&Rect::new(10, 0, 5, 5)).is_empty());
        assert_eq!(Rect::EMPTY.union(&b), b);
    }

    #[test]
    fn corners_are_inclusive_and_ordered() {
        let r = Rect::from_corners(Point::new(4, 7), Point::new(1, 2));
        assert_eq!(r, Rect::new(1, 2, 4, 6));
        assert_eq!(r.points().count(), 24);
    }

    #[test]
    fn include_grows_from_empty() {
        let mut r = Rect::EMPTY;
        r.include(3, 4);
        r.include(1, 9);
        assert_eq!(r, Rect::new(1, 4, 3, 6));
    }
}
