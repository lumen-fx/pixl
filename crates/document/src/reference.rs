//! Where a reference image sits over the canvas.
//!
//! A reference layer keeps its image at the image's own resolution and
//! places it over the document with a move, a scale per axis and a turn.
//! The editor draws it smoothly at screen resolution, independent of the
//! document's pixel grid; it is never painted on and never part of the
//! document's composite.

use crate::error::Error;
use crate::transform::Affine;

/// A reference image's place over the canvas, in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferencePlace {
    /// The document point the middle of the image sits on.
    pub center: (f64, f64),
    /// Document pixels per image pixel along each image axis. A negative
    /// factor mirrors that axis.
    pub scale: (f64, f64),
    /// Degrees the image turns clockwise around its middle.
    pub rotation: f64,
}

impl ReferencePlace {
    /// An image of `size` with its top-left corner on the document's, one
    /// image pixel to a document pixel: where a reference of the old 1:1 kind
    /// sat.
    pub fn top_left(size: (u32, u32)) -> Self {
        Self {
            center: (f64::from(size.0) / 2.0, f64::from(size.1) / 2.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
        }
    }

    /// An image of `size` in the middle of a canvas of `canvas`, scaled down
    /// to fit inside it when it is larger, never scaled up.
    pub fn fitted(size: (u32, u32), canvas: (u32, u32)) -> Self {
        let (iw, ih) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
        let (cw, ch) = (f64::from(canvas.0), f64::from(canvas.1));
        let k = (cw / iw).min(ch / ih).min(1.0);
        Self {
            center: (cw / 2.0, ch / 2.0),
            scale: (k, k),
            rotation: 0.0,
        }
    }

    /// Whether every number is finite and neither scale is zero.
    pub fn is_valid(&self) -> bool {
        let finite = [
            self.center.0,
            self.center.1,
            self.scale.0,
            self.scale.1,
            self.rotation,
        ]
        .iter()
        .all(|v| v.is_finite());
        finite && self.scale.0 != 0.0 && self.scale.1 != 0.0
    }

    /// `Ok` when [`Self::is_valid`].
    pub fn check(&self) -> Result<(), Error> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(Error::InvalidPlace)
        }
    }

    /// The map from image pixel coordinates to document coordinates for an
    /// image of `size`: scale and turn around the image's middle, then put
    /// the middle on [`Self::center`].
    pub fn matrix(&self, size: (u32, u32)) -> Affine {
        let half = (f64::from(size.0) / 2.0, f64::from(size.1) / 2.0);
        Affine::translate(-half.0, -half.1)
            .then(&Affine::scale(self.scale.0, self.scale.1))
            .then(&Affine::rotate(self.rotation))
            .then(&Affine::translate(self.center.0, self.center.1))
    }

    /// The place whose [`Self::matrix`] is `m`, for an image of `size`. A
    /// map with no shear (a move, turn, mirror or even scale of a placed
    /// image) comes back exactly; a shear, which a place cannot hold, is
    /// dropped and the axes keep their lengths. `None` for a map that
    /// collapses the image.
    pub fn from_matrix(m: &Affine, size: (u32, u32)) -> Option<Self> {
        let half = (f64::from(size.0) / 2.0, f64::from(size.1) / 2.0);
        let center = m.apply(half.0, half.1);
        let sx = m.a.hypot(m.b);
        let det = m.a * m.d - m.b * m.c;
        if sx < 1e-12 || det.abs() < 1e-12 {
            return None;
        }
        let rotation = m.b.atan2(m.a).to_degrees().rem_euclid(360.0);
        // The image's y axis, as long as it was, on the side the determinant
        // says: a mirror shows as a negative y scale.
        let sy = m.c.hypot(m.d).copysign(det);
        let place = Self {
            center,
            scale: (sx, sy),
            rotation,
        };
        place.is_valid().then_some(place)
    }

    /// The document positions of the image's corners, clockwise from its
    /// top-left.
    pub fn corners(&self, size: (u32, u32)) -> [(f64, f64); 4] {
        let m = self.matrix(size);
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| m.apply(x, y))
    }

    /// The width and height the image covers in document pixels, before it
    /// turns.
    pub fn extent(&self, size: (u32, u32)) -> (f64, f64) {
        (
            f64::from(size.0) * self.scale.0.abs(),
            f64::from(size.1) * self.scale.1.abs(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn top_left_maps_pixels_one_to_one() {
        let p = ReferencePlace::top_left((40, 30));
        let m = p.matrix((40, 30));
        assert!(near(m.apply(0.0, 0.0), (0.0, 0.0)));
        assert!(near(m.apply(40.0, 30.0), (40.0, 30.0)));
        assert!(near(m.apply(7.5, 2.0), (7.5, 2.0)));
    }

    #[test]
    fn scale_and_turn_happen_around_the_middle() {
        let p = ReferencePlace {
            center: (100.0, 50.0),
            scale: (0.5, 0.25),
            rotation: 90.0,
        };
        let size = (4000, 3000);
        let m = p.matrix(size);
        // The middle stays on the center.
        assert!(near(m.apply(2000.0, 1500.0), (100.0, 50.0)));
        // The right edge's middle, 2000 image pixels right of the middle,
        // is 1000 document pixels away and turned to point down.
        assert!(near(m.apply(4000.0, 1500.0), (100.0, 1050.0)));
        // The top edge's middle, 1500 up, is 375 away and points right.
        assert!(near(m.apply(2000.0, 0.0), (475.0, 50.0)));
        assert_eq!(p.extent(size), (2000.0, 750.0));
    }

    #[test]
    fn a_place_survives_its_own_matrix() {
        for place in [
            ReferencePlace {
                center: (12.5, -3.0),
                scale: (0.016, 0.016),
                rotation: 33.0,
            },
            ReferencePlace {
                center: (0.0, 0.0),
                scale: (2.0, -0.5),
                rotation: 271.0,
            },
        ] {
            let back = ReferencePlace::from_matrix(&place.matrix((640, 480)), (640, 480)).unwrap();
            let m = (place.matrix((640, 480)), back.matrix((640, 480)));
            for (x, y) in [(0.0, 0.0), (640.0, 0.0), (0.0, 480.0), (320.0, 99.0)] {
                assert!(
                    near(m.0.apply(x, y), m.1.apply(x, y)),
                    "{place:?} vs {back:?}"
                );
            }
        }
    }

    #[test]
    fn a_canvas_flip_mirrors_the_place() {
        let size = (100, 50);
        let place = ReferencePlace {
            center: (30.0, 20.0),
            scale: (0.5, 0.5),
            rotation: 10.0,
        };
        // Flip a 64-wide canvas left to right.
        let flip = Affine::scale(-1.0, 1.0).then(&Affine::translate(64.0, 0.0));
        let m = place.matrix(size).then(&flip);
        let back = ReferencePlace::from_matrix(&m, size).unwrap();
        assert!(near(back.center, (34.0, 20.0)));
        assert!(back.scale.0 * back.scale.1 < 0.0, "mirrored: {back:?}");
        let n = back.matrix(size);
        for (x, y) in [(0.0, 0.0), (100.0, 50.0), (13.0, 41.0)] {
            assert!(near(m.apply(x, y), n.apply(x, y)));
        }
    }

    #[test]
    fn fitting_scales_down_and_centers() {
        let p = ReferencePlace::fitted((4000, 3000), (64, 64));
        assert!((p.scale.0 - 0.016).abs() < 1e-12 && p.scale.0 == p.scale.1);
        assert_eq!(p.center, (32.0, 32.0));
        let small = ReferencePlace::fitted((16, 8), (64, 64));
        assert_eq!(small.scale, (1.0, 1.0));
    }

    #[test]
    fn invalid_places_are_refused() {
        let mut p = ReferencePlace::top_left((4, 4));
        assert!(p.check().is_ok());
        p.scale.1 = 0.0;
        assert_eq!(p.check(), Err(Error::InvalidPlace));
        p.scale.1 = f64::NAN;
        assert!(!p.is_valid());
        assert!(ReferencePlace::from_matrix(&Affine::scale(0.0, 1.0), (4, 4)).is_none());
    }
}
