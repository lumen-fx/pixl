//! Rasterizing geometry onto the pixel grid. These functions only produce
//! pixel coordinates; painting them is the tools' job.

use crate::geom::Point;

/// The pixels of a line from `a` to `b`, both included, in order.
///
/// The line is a series of runs along its major axis, one run per step on the
/// minor axis, with run lengths as even as integers allow (they differ by at
/// most one). Lines whose length is a whole number of runs, the 1:1, 1:2 and
/// 1:3 lines pixel artists draw, come out with identical steps, and no pixel
/// ever touches two others on the same side, so the line is pixel-perfect.
/// The result depends only on the offset `b - a`, so mirroring both ends
/// mirrors the line exactly.
pub fn line(a: Point, b: Point) -> Vec<Point> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (sx, sy) = (dx.signum(), dy.signum());
    let (adx, ady) = (dx.unsigned_abs() as i64, dy.unsigned_abs() as i64);
    let x_major = adx >= ady;
    let (major, minor) = if x_major { (adx, ady) } else { (ady, adx) };
    let total = major + 1;
    let runs = minor + 1;
    let mut out = Vec::with_capacity(total as usize);
    let mut m = 0i64;
    for i in 0..runs {
        let len = (i + 1) * total / runs - i * total / runs;
        for _ in 0..len {
            let (along, across) = (m as i32, i as i32);
            out.push(if x_major {
                Point::new(a.x + sx * along, a.y + sy * across)
            } else {
                Point::new(a.x + sx * across, a.y + sy * along)
            });
            m += 1;
        }
    }
    out
}

/// Pixels of a polyline through `points`, without repeating shared vertices.
pub fn polyline(points: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    match points {
        [] => {}
        [p] => out.push(*p),
        _ => {
            for w in points.windows(2) {
                let seg = line(w[0], w[1]);
                let skip = usize::from(!out.is_empty());
                out.extend(seg.into_iter().skip(skip));
            }
        }
    }
    out
}

/// The step ratios line snapping offers, as (major run, minor step).
const SNAP_RATIOS: [(i32, i32); 7] = [(1, 0), (0, 1), (1, 1), (2, 1), (1, 2), (3, 1), (1, 3)];

/// Moves `b` so the line from `a` takes the nearest clean slope (horizontal,
/// vertical, 1:1, 1:2, 2:1, 1:3, 3:1) with whole runs, so every step of the
/// line has the same length.
pub fn snap_line(a: Point, b: Point) -> Point {
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    if dx == 0.0 && dy == 0.0 {
        return b;
    }
    let angle = dy.abs().atan2(dx.abs());
    let (rx, ry) = SNAP_RATIOS
        .iter()
        .copied()
        .min_by(|p, q| {
            let da = (angle - (p.1 as f64).atan2(p.0 as f64)).abs();
            let db = (angle - (q.1 as f64).atan2(q.0 as f64)).abs();
            da.total_cmp(&db)
        })
        .expect("ratios are not empty");
    if ry == 0 {
        return Point::new(b.x, a.y);
    }
    if rx == 0 {
        return Point::new(a.x, b.y);
    }
    // k runs of `run` pixels each: the end sits k*run - 1 along the major
    // axis and k - 1 along the minor one.
    let x_major = rx >= ry;
    let run = rx.max(ry);
    let major_len = if x_major { dx.abs() } else { dy.abs() } + 1.0;
    let k = ((major_len / run as f64).round() as i32).max(1);
    let (along, across) = (k * run - 1, k - 1);
    let (ox, oy) = if x_major {
        (along, across)
    } else {
        (across, along)
    };
    let s = |v: i32| if v < 0 { -1 } else { 1 };
    Point::new(a.x + s(b.x - a.x) * ox, a.y + s(b.y - a.y) * oy)
}

/// Whether `prev, mid, next` form an L: `mid` touches both orthogonally and
/// they touch each other diagonally.
fn is_corner(prev: Point, mid: Point, next: Point) -> bool {
    let orth = |p: Point, q: Point| (p.x - q.x).abs() + (p.y - q.y).abs() == 1;
    orth(prev, mid)
        && orth(mid, next)
        && (prev.x - next.x).abs() == 1
        && (prev.y - next.y).abs() == 1
}

/// Appends `p` to a pixel-perfect path: drops a repeat of the last pixel and
/// removes the last pixel when it would form an L-shaped corner with `p`.
/// Returns whether the previous last pixel was removed.
pub fn push_pixel_perfect(path: &mut Vec<Point>, p: Point) -> bool {
    if path.last() == Some(&p) {
        return false;
    }
    let n = path.len();
    let removed = n >= 2 && is_corner(path[n - 2], path[n - 1], p);
    if removed {
        path.pop();
    }
    path.push(p);
    removed
}

/// The path with L-shaped corners removed, the pixel-perfect stroke filter.
pub fn pixel_perfect(points: &[Point]) -> Vec<Point> {
    let mut out = Vec::with_capacity(points.len());
    for &p in points {
        push_pixel_perfect(&mut out, p);
    }
    out
}

/// The outline of the ellipse inscribed in the box with corners `a` and `b`
/// (both included). Uses Alois Zingl's integer midpoint algorithm ("A
/// Rasterizing Algorithm for Drawing Curves", 2012, `plotEllipseRect`), which
/// handles even and odd sizes and draws a one-pixel outline without doubled
/// corners. Points are unique but unordered.
pub fn ellipse_outline(a: Point, b: Point) -> Vec<Point> {
    let (mut x0, mut y0, mut x1, mut y1) = (a.x as i64, a.y as i64, b.x as i64, b.y as i64);
    let aa = (x1 - x0).abs();
    let bb = (y1 - y0).abs();
    let mut b1 = bb & 1;
    let mut dx = 4 * (1 - aa) * bb * bb;
    let mut dy = 4 * (b1 + 1) * aa * aa;
    let mut err = dx + dy + b1 * aa * aa;
    if x0 > x1 {
        x0 = x1;
        x1 += aa;
    }
    if y0 > y1 {
        y0 = y1;
    }
    y0 += (bb + 1) / 2;
    y1 = y0 - b1;
    let a8 = 8 * aa * aa;
    b1 = 8 * bb * bb;
    let mut out = Vec::new();
    let mut put = |x: i64, y: i64| out.push(Point::new(x as i32, y as i32));
    loop {
        put(x1, y0);
        put(x0, y0);
        put(x0, y1);
        put(x1, y1);
        let e2 = 2 * err;
        if e2 <= dy {
            y0 += 1;
            y1 -= 1;
            dy += a8;
            err += dy;
        }
        if e2 >= dx || 2 * err > dy {
            x0 += 1;
            x1 -= 1;
            dx += b1;
            err += dx;
        }
        if x0 > x1 {
            break;
        }
    }
    while y0 - y1 <= bb {
        put(x0 - 1, y0);
        put(x1 + 1, y0);
        put(x0 - 1, y1);
        put(x1 + 1, y1);
        y0 += 1;
        y1 -= 1;
    }
    out.sort_by_key(|p| (p.y, p.x));
    out.dedup();
    out
}

/// Every pixel of the filled ellipse inscribed in the box `a`..`b`: each row
/// spans its outline pixels.
pub fn ellipse_filled(a: Point, b: Point) -> Vec<Point> {
    let outline = ellipse_outline(a, b);
    let mut out = Vec::new();
    let mut i = 0;
    while i < outline.len() {
        let y = outline[i].y;
        let mut j = i;
        while j < outline.len() && outline[j].y == y {
            j += 1;
        }
        let (x0, x1) = (outline[i].x, outline[j - 1].x);
        out.extend((x0..=x1).map(|x| Point::new(x, y)));
        i = j;
    }
    out
}

/// The outline of the rectangle with corners `a` and `b`.
pub fn rect_outline(a: Point, b: Point) -> Vec<Point> {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    let mut out = Vec::new();
    for x in x0..=x1 {
        out.push(Point::new(x, y0));
        if y1 != y0 {
            out.push(Point::new(x, y1));
        }
    }
    for y in y0 + 1..y1 {
        out.push(Point::new(x0, y));
        if x1 != x0 {
            out.push(Point::new(x1, y));
        }
    }
    out
}

/// Pixels inside a closed polygon (even-odd rule at pixel centers) plus its
/// outline, the lasso and contour shape.
pub fn polygon_filled(points: &[Point]) -> Vec<Point> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut out = polyline(points);
    out.extend(line(*points.last().expect("not empty"), points[0]));
    let y0 = points.iter().map(|p| p.y).min().expect("not empty");
    let y1 = points.iter().map(|p| p.y).max().expect("not empty");
    let n = points.len();
    for y in y0..=y1 {
        let cy = y as f64 + 0.5;
        let mut xs: Vec<f64> = Vec::new();
        for i in 0..n {
            let (p, q) = (points[i], points[(i + 1) % n]);
            let (py, qy) = (p.y as f64 + 0.5, q.y as f64 + 0.5);
            if (py <= cy) != (qy <= cy) {
                let t = (cy - py) / (qy - py);
                xs.push(p.x as f64 + 0.5 + t * (q.x - p.x) as f64);
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks(2) {
            if let [l, r] = pair {
                let start = (l - 0.5).ceil() as i32;
                let end = (r - 0.5).floor() as i32;
                out.extend((start..=end).map(|x| Point::new(x, y)));
            }
        }
    }
    out.sort_by_key(|p| (p.y, p.x));
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(points: &[Point]) -> Vec<usize> {
        let mut out = vec![1];
        for w in points.windows(2) {
            if w[0].y == w[1].y {
                *out.last_mut().unwrap() += 1;
            } else {
                out.push(1);
            }
        }
        out
    }

    #[test]
    fn lines_have_even_steps() {
        let l = line(Point::new(0, 0), Point::new(5, 2));
        assert_eq!(runs(&l), vec![2, 2, 2]);
        let l = line(Point::new(0, 0), Point::new(8, 2));
        assert_eq!(runs(&l), vec![3, 3, 3]);
        let l = line(Point::new(0, 0), Point::new(9, 3));
        assert_eq!(runs(&l), vec![2, 3, 2, 3], "uneven only when it must be");
        assert_eq!(
            line(Point::new(3, 3), Point::new(3, 3)),
            vec![Point::new(3, 3)]
        );
    }

    #[test]
    fn lines_are_connected_and_end_on_target() {
        for (bx, by) in [(7, -3), (-4, 9), (-6, -6), (0, -5), (11, 1)] {
            let b = Point::new(bx, by);
            let l = line(Point::new(0, 0), b);
            assert_eq!(*l.last().unwrap(), b);
            for w in l.windows(2) {
                assert!((w[0].x - w[1].x).abs() <= 1 && (w[0].y - w[1].y).abs() <= 1);
            }
            assert_eq!(pixel_perfect(&l), l, "no L corners in a line to {b:?}");
        }
    }

    #[test]
    fn mirrored_ends_mirror_the_line() {
        let l = line(Point::new(1, 1), Point::new(9, 4));
        let m = line(Point::new(-1, 1), Point::new(-9, 4));
        let mirrored: Vec<Point> = l.iter().map(|p| Point::new(-p.x, p.y)).collect();
        assert_eq!(m, mirrored);
    }

    #[test]
    fn snapping_picks_clean_ratios() {
        let a = Point::new(10, 10);
        assert_eq!(snap_line(a, Point::new(30, 11)), Point::new(30, 10));
        assert_eq!(snap_line(a, Point::new(19, 19)), Point::new(19, 19));
        let b = snap_line(a, Point::new(20, 15));
        assert!(runs(&line(a, b)).iter().all(|r| *r == 2), "{b:?}");
        let b = snap_line(a, Point::new(0, 5));
        assert!(runs(&line(a, b)).iter().all(|r| *r == 2), "{b:?}");
        let b = snap_line(a, Point::new(20, -20));
        assert_eq!((b.x - a.x, b.y - a.y), (9, -29), "1:3 steep, 10 runs of 3");
    }

    #[test]
    fn pixel_perfect_removes_corners() {
        let path = [
            Point::new(0, 0),
            Point::new(1, 0),
            Point::new(1, 1),
            Point::new(2, 1),
            Point::new(2, 1),
            Point::new(3, 1),
        ];
        assert_eq!(
            pixel_perfect(&path),
            vec![
                Point::new(0, 0),
                Point::new(1, 1),
                Point::new(2, 1),
                Point::new(3, 1)
            ]
        );
    }

    #[test]
    fn ellipse_outline_is_symmetric_and_thin() {
        for (w, h) in [(1, 1), (2, 2), (3, 3), (8, 5), (9, 14), (16, 16)] {
            let pts = ellipse_outline(Point::new(0, 0), Point::new(w - 1, h - 1));
            let set: std::collections::HashSet<Point> = pts.iter().copied().collect();
            for p in &pts {
                assert!(p.x >= 0 && p.y >= 0 && p.x < w && p.y < h, "{w}x{h} {p:?}");
                assert!(
                    set.contains(&Point::new(w - 1 - p.x, p.y)),
                    "{w}x{h} mirror x"
                );
                assert!(
                    set.contains(&Point::new(p.x, h - 1 - p.y)),
                    "{w}x{h} mirror y"
                );
            }
            // Touches all four sides of its box.
            assert!(pts.iter().any(|p| p.x == 0) && pts.iter().any(|p| p.x == w - 1));
            assert!(pts.iter().any(|p| p.y == 0) && pts.iter().any(|p| p.y == h - 1));
        }
        // A 3x3 circle is a plus.
        let plus = ellipse_filled(Point::new(0, 0), Point::new(2, 2));
        assert_eq!(plus.len(), 5);
    }

    #[test]
    fn polygon_fill_covers_inside() {
        let tri = polygon_filled(&[Point::new(0, 0), Point::new(10, 0), Point::new(0, 10)]);
        assert!(tri.contains(&Point::new(2, 2)));
        assert!(!tri.contains(&Point::new(8, 8)));
        let square = polygon_filled(&[
            Point::new(0, 0),
            Point::new(4, 0),
            Point::new(4, 4),
            Point::new(0, 4),
        ]);
        assert_eq!(square.len(), 25);
    }
}
