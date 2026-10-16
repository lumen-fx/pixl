//! Bucket fill and magic wand: both grow a region of matching pixels.

use crate::color::{Palette, Rgba};
use crate::document::{Document, LayerId};
use crate::error::Error;
use crate::geom::Point;
use crate::image::Raster;
use crate::selection::Selection;
use crate::tools::{Canvas, Ink};

/// Which neighbors a region grows into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connectivity {
    /// Left, right, up, down.
    #[default]
    Four,
    /// Also the diagonals.
    Eight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillOptions {
    pub connectivity: Connectivity,
    /// Largest per-channel difference that still matches; 0 matches the
    /// exact stored value.
    pub tolerance: u8,
    /// Grow from the seed (true) or take every matching pixel (false).
    pub contiguous: bool,
}

/// Where a region's colors are read from.
#[derive(Clone, Copy, Debug)]
pub enum Sampler<'a> {
    /// One image (the current layer).
    Raster {
        raster: &'a Raster,
        palette: &'a Palette,
        transparent_index: u8,
    },
    /// An empty cel: every pixel is transparent.
    Blank { width: u32, height: u32 },
    /// A flattened composite (merged sampling), row-major.
    Merged {
        pixels: &'a [Rgba],
        width: u32,
        height: u32,
    },
}

impl<'a> Sampler<'a> {
    /// Samples one cel of a document.
    pub fn cel(doc: &'a Document, layer: LayerId, frame: usize) -> Result<Self, Error> {
        Ok(match doc.cel_raster(layer, frame)? {
            Some(raster) => Sampler::Raster {
                raster,
                palette: doc.palette(),
                transparent_index: doc.transparent_index(),
            },
            None => Sampler::Blank {
                width: doc.width(),
                height: doc.height(),
            },
        })
    }

    /// Samples the image a canvas paints on.
    pub fn canvas(canvas: &'a Canvas<'_>) -> Self {
        Sampler::Raster {
            raster: canvas.raster(),
            palette: canvas.palette(),
            transparent_index: canvas.transparent_index(),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        match self {
            Sampler::Raster { raster, .. } => (raster.width(), raster.height()),
            Sampler::Blank { width, height } | Sampler::Merged { width, height, .. } => {
                (*width, *height)
            }
        }
    }

    fn rgba(&self, x: i32, y: i32) -> Rgba {
        match self {
            Sampler::Raster {
                raster,
                palette,
                transparent_index,
            } => match raster {
                Raster::Rgba(_) => Rgba::from_u32(raster.get_raw(x, y)),
                Raster::Indexed(_) => {
                    let i = raster.get_raw(x, y) as u8;
                    if i == *transparent_index {
                        Rgba::TRANSPARENT
                    } else {
                        palette.get(i)
                    }
                }
                Raster::Mask(_) => {
                    let v = raster.get_raw(x, y) as u8;
                    Rgba::new(v, v, v, 255)
                }
            },
            Sampler::Blank { .. } => Rgba::TRANSPARENT,
            Sampler::Merged { pixels, width, .. } => pixels[(y as u32 * width + x as u32) as usize],
        }
    }

    /// Whether `(x, y)` matches the pixel at `seed`.
    fn matcher(&self, seed: Point, tolerance: u8) -> impl Fn(i32, i32) -> bool + '_ {
        let raw_seed = match self {
            Sampler::Raster { raster, .. } => Some(raster.get_raw(seed.x, seed.y)),
            _ => None,
        };
        let rgba_seed = self.rgba(seed.x, seed.y);
        move |x, y| match (self, raw_seed) {
            (Sampler::Raster { raster, .. }, Some(s)) if tolerance == 0 => {
                raster.get_raw(x, y) == s
            }
            _ => self.rgba(x, y).distance(rgba_seed) <= tolerance,
        }
    }
}

/// The pixels matching the seed, grown from it or taken globally, never
/// leaving `limit` when one is given.
pub fn region(
    sampler: &Sampler,
    seed: Point,
    opts: &FillOptions,
    limit: Option<&Selection>,
) -> Selection {
    let (w, h) = sampler.size();
    let mut out = Selection::empty(w, h);
    let inside = |x: i32, y: i32| {
        x >= 0 && y >= 0 && x < w as i32 && y < h as i32 && limit.is_none_or(|s| s.contains(x, y))
    };
    if !inside(seed.x, seed.y) {
        return out;
    }
    let matches = sampler.matcher(seed, opts.tolerance);
    if !opts.contiguous {
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                if inside(x, y) && matches(x, y) {
                    out.set(x, y, true);
                }
            }
        }
        return out;
    }
    let mut seen = vec![false; w as usize * h as usize];
    let idx = |x: i32, y: i32| y as usize * w as usize + x as usize;
    let mut stack = vec![seed];
    seen[idx(seed.x, seed.y)] = true;
    const FOUR: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    const EIGHT: [(i32, i32); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    let dirs: &[(i32, i32)] = match opts.connectivity {
        Connectivity::Four => &FOUR,
        Connectivity::Eight => &EIGHT,
    };
    while let Some(p) = stack.pop() {
        out.set(p.x, p.y, true);
        for &(dx, dy) in dirs {
            let (x, y) = (p.x + dx, p.y + dy);
            if inside(x, y) && !seen[idx(x, y)] {
                seen[idx(x, y)] = true;
                if matches(x, y) {
                    stack.push(Point::new(x, y));
                }
            }
        }
    }
    out
}

/// Fills the region at `seed` with `ink`. `merged` samples a flattened
/// composite instead of the canvas's own pixels. The region never leaves the
/// canvas's selection. Symmetry does not apply to fills.
pub fn bucket_fill(
    canvas: &mut Canvas,
    seed: Point,
    ink: &Ink,
    opts: &FillOptions,
    merged: Option<&Sampler>,
) {
    let area = {
        let own = Sampler::canvas(canvas);
        region(merged.unwrap_or(&own), seed, opts, canvas.selection())
    };
    for p in area.bounds().points() {
        if area.contains(p.x, p.y)
            && let Some(v) = canvas.ink_value(ink, p.x, p.y)
        {
            canvas.write(p.x, p.y, v);
        }
    }
}

/// The magic wand: the region at `seed` as a selection.
pub fn magic_wand(sampler: &Sampler, seed: Point, opts: &FillOptions) -> Selection {
    region(sampler, seed, opts, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{Color, ColorMode};
    use crate::composite::{CompositeOptions, composite_frame};
    use crate::document::{NewLayer, Placement};
    use crate::geom::Rect;
    use crate::history::History;
    use crate::tools::{ToolContext, edit_cel};

    const WALL: Rgba = Rgba::BLACK;

    /// A 9x9 cel with a 1px black ring around (2,2)-(6,6) whose corners are
    /// open diagonally: 4-connected fills stay in, 8-connected leak out.
    fn ring() -> (Document, LayerId) {
        let mut d = Document::new(9, 9, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let mut h = History::default();
        edit_cel(&mut d, &mut h, "ring", l, 0, &ToolContext::default(), |c| {
            for i in 3..6 {
                for p in [
                    Point::new(i, 2),
                    Point::new(i, 6),
                    Point::new(2, i),
                    Point::new(6, i),
                ] {
                    c.plot(p, &Ink::Paint(WALL.into()));
                }
            }
        })
        .unwrap();
        (d, l)
    }

    fn fill_at(d: &mut Document, l: LayerId, seed: Point, opts: FillOptions) -> Rect {
        let mut h = History::default();
        let (_, dirty) = edit_cel(d, &mut h, "fill", l, 0, &ToolContext::default(), |c| {
            bucket_fill(c, seed, &Ink::Paint(Rgba::WHITE.into()), &opts, None)
        })
        .unwrap();
        dirty
    }

    #[test]
    fn four_connected_stays_inside() {
        let (mut d, l) = ring();
        let dirty = fill_at(
            &mut d,
            l,
            Point::new(4, 4),
            FillOptions {
                contiguous: true,
                ..Default::default()
            },
        );
        assert_eq!(dirty, Rect::new(3, 3, 3, 3));
    }

    #[test]
    fn eight_connected_leaks_through_corners() {
        let (mut d, l) = ring();
        let dirty = fill_at(
            &mut d,
            l,
            Point::new(4, 4),
            FillOptions {
                contiguous: true,
                connectivity: Connectivity::Eight,
                tolerance: 0,
            },
        );
        assert_eq!(dirty, Rect::new(0, 0, 9, 9));
    }

    #[test]
    fn global_fill_and_tolerance() {
        let mut d = Document::new(4, 1, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let mut h = History::default();
        let colors = [
            Rgba::opaque(100, 0, 0),
            Rgba::BLACK,
            Rgba::opaque(110, 0, 0),
            Rgba::opaque(130, 0, 0),
        ];
        edit_cel(&mut d, &mut h, "row", l, 0, &ToolContext::default(), |c| {
            for (x, col) in colors.iter().enumerate() {
                c.plot(Point::new(x as i32, 0), &Ink::Paint((*col).into()));
            }
        })
        .unwrap();
        let s = Sampler::cel(&d, l, 0).unwrap();
        let global = magic_wand(
            &s,
            Point::new(0, 0),
            &FillOptions {
                tolerance: 10,
                contiguous: false,
                ..Default::default()
            },
        );
        assert_eq!(global.count(), 2);
        assert!(global.contains(2, 0));
        let contiguous = magic_wand(
            &s,
            Point::new(0, 0),
            &FillOptions {
                tolerance: 10,
                contiguous: true,
                ..Default::default()
            },
        );
        assert_eq!(contiguous.count(), 1);
    }

    #[test]
    fn merged_sampling_reads_other_layers_and_writes_this_one() {
        let (mut d, wall_layer) = ring();
        let top = d
            .add_layer(NewLayer::Raster, "paint", Placement::default())
            .unwrap();
        let flat = composite_frame(&d, 0, &CompositeOptions::EXPORT);
        let merged = Sampler::Merged {
            pixels: &flat,
            width: 9,
            height: 9,
        };
        let mut h = History::default();
        edit_cel(
            &mut d,
            &mut h,
            "fill",
            top,
            0,
            &ToolContext::default(),
            |c| {
                bucket_fill(
                    c,
                    Point::new(4, 4),
                    &Ink::Paint(Color::Rgba(Rgba::WHITE)),
                    &FillOptions {
                        contiguous: true,
                        ..Default::default()
                    },
                    Some(&merged),
                )
            },
        )
        .unwrap();
        let r = d.cel_raster(top, 0).unwrap().unwrap();
        assert_eq!(r.content_bounds(), Rect::new(3, 3, 3, 3));
        assert_eq!(
            d.cel_raster(wall_layer, 0).unwrap().unwrap().get_raw(4, 4),
            0
        );
    }

    #[test]
    fn fill_stays_in_selection() {
        let mut d = Document::new(10, 10, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let mut h = History::default();
        let ctx = ToolContext {
            selection: Some(Selection::rect(10, 10, Rect::new(0, 0, 3, 10))),
            ..Default::default()
        };
        let (_, dirty) = edit_cel(&mut d, &mut h, "fill", l, 0, &ctx, |c| {
            bucket_fill(
                c,
                Point::new(1, 1),
                &Ink::Paint(Rgba::WHITE.into()),
                &FillOptions {
                    contiguous: true,
                    ..Default::default()
                },
                None,
            )
        })
        .unwrap();
        assert_eq!(dirty, Rect::new(0, 0, 3, 10));
    }

    #[test]
    fn indexed_exact_match_keeps_duplicate_entries_apart() {
        let mut d = Document::new(3, 1, ColorMode::Indexed).unwrap();
        d.set_palette_color(5, Rgba::BLACK).unwrap();
        let l = d.layers()[0].id;
        let mut h = History::default();
        edit_cel(&mut d, &mut h, "row", l, 0, &ToolContext::default(), |c| {
            c.plot(Point::new(0, 0), &Ink::Paint(Color::Index(1)));
            c.plot(Point::new(1, 0), &Ink::Paint(Color::Index(5)));
            c.plot(Point::new(2, 0), &Ink::Paint(Color::Index(1)));
        })
        .unwrap();
        let s = Sampler::cel(&d, l, 0).unwrap();
        let exact = magic_wand(&s, Point::new(0, 0), &FillOptions::default());
        assert_eq!(exact.count(), 2);
        let loose = magic_wand(
            &s,
            Point::new(0, 0),
            &FillOptions {
                tolerance: 1,
                ..Default::default()
            },
        );
        assert_eq!(loose.count(), 3);
    }
}
