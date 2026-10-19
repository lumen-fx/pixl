//! Brush dynamics: how pen pressure and the pointer's speed change a
//! freehand stroke as it goes. Each input passes through its own response
//! (a threshold below which it reads as nothing, one above which it reads
//! as full, and a curve between), and each of the brush's size, its angle
//! and its color can follow one input. Color runs from the background color
//! at no input to the foreground color at full input, blended or broken up
//! by a dither, so a dithered stroke only ever lays down the two colors.

use std::collections::HashMap;

use crate::tools::gradient::DitherPattern;
use crate::tools::{Brush, Ink};
use pixl_document::color::{Color, Rgba};

/// The input a part of the brush follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sensor {
    /// The part stays as the tool options set it.
    #[default]
    Off,
    /// Pen pressure. A mouse presses at full pressure.
    Pressure,
    /// How fast the pointer moves across the screen.
    Velocity,
}

/// How an input reading, 0 to 1, turns into the amount a part of the brush
/// follows: nothing up to `min`, all of it from `max` on, and between them
/// the stretch raised to `curve` (below 1 a little input goes far, above 1
/// it takes a lot).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Response {
    pub min: f32,
    pub max: f32,
    pub curve: f32,
}

impl Default for Response {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 1.0,
            curve: 1.0,
        }
    }
}

impl Response {
    /// The amount, 0 to 1, a reading of `input` gives.
    pub fn apply(&self, input: f32) -> f32 {
        let input = if input.is_finite() { input } else { 0.0 };
        let (min, max) = (self.min.clamp(0.0, 1.0), self.max.clamp(0.0, 1.0));
        let t = if max <= min {
            if input >= max { 1.0 } else { 0.0 }
        } else {
            ((input - min) / (max - min)).clamp(0.0, 1.0)
        };
        let curve = if self.curve.is_finite() {
            self.curve.clamp(0.1, 10.0)
        } else {
            1.0
        };
        t.powf(curve)
    }
}

/// What the inputs do to a stroke.
#[derive(Clone, Debug, PartialEq)]
pub struct Dynamics {
    pub pressure: Response,
    pub velocity: Response,
    /// The input the brush size follows, and the size at no input and at
    /// full input, as shares of the tool's size (1 is the size itself).
    pub size: Sensor,
    pub min_size: f32,
    pub max_size: f32,
    /// The input the brush angle follows, and the angle at no input and at
    /// full input, in degrees clockwise. A round brush looks the same at
    /// every angle.
    pub angle: Sensor,
    pub min_angle: f32,
    pub max_angle: f32,
    /// The input the color follows: the background color at no input, the
    /// foreground color at full input.
    pub color: Sensor,
    /// How the colors between are made: blended (`None`), or a dither of
    /// the two colors, so no other color appears.
    pub dither: Option<DitherPattern>,
}

impl Default for Dynamics {
    fn default() -> Self {
        Self {
            pressure: Response::default(),
            velocity: Response::default(),
            size: Sensor::Off,
            min_size: 0.0,
            max_size: 1.0,
            angle: Sensor::Off,
            min_angle: 0.0,
            max_angle: 90.0,
            color: Sensor::Off,
            dither: None,
        }
    }
}

/// The inputs at a point of a stroke, each 0 to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    pub pressure: f32,
    pub velocity: f32,
}

/// What a point of a stroke paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    /// The brush size in pixels, at least one.
    pub size: u32,
    /// The brush angle in whole degrees, 0 to 359.
    pub angle: i32,
    /// How much of the foreground color, 0 (the background) to 255, or
    /// `None` to paint the tool's own color.
    pub mix: Option<u8>,
}

impl Dynamics {
    /// Whether any part of the brush follows an input.
    pub fn is_dynamic(&self) -> bool {
        self.size != Sensor::Off || self.angle != Sensor::Off || self.color != Sensor::Off
    }

    /// Whether any part of the brush follows `sensor`.
    pub fn uses(&self, sensor: Sensor) -> bool {
        self.size == sensor || self.angle == sensor || self.color == sensor
    }

    /// The amount, 0 to 1, a part following `sensor` takes from `reading`.
    pub fn amount(&self, sensor: Sensor, reading: Reading) -> Option<f32> {
        match sensor {
            Sensor::Off => None,
            Sensor::Pressure => Some(self.pressure.apply(reading.pressure)),
            Sensor::Velocity => Some(self.velocity.apply(reading.velocity)),
        }
    }

    /// The share of the tool's size `reading` paints with.
    pub fn size_share(&self, reading: Reading) -> f32 {
        match self.amount(self.size, reading) {
            None => 1.0,
            Some(t) => {
                let (lo, hi) = (self.min_size.clamp(0.0, 4.0), self.max_size.clamp(0.0, 4.0));
                lo + (hi - lo) * t
            }
        }
    }

    /// What a stroke of a tool of `size` pixels paints with at `reading`.
    /// The size never drops below one pixel or grows past four times the
    /// tool's.
    pub fn dab(&self, size: u32, reading: Reading) -> Dab {
        let size = size.max(1);
        let px = (size as f32 * self.size_share(reading)).round().max(1.0) as u32;
        let angle = match self.amount(self.angle, reading) {
            None => 0.0,
            Some(t) => self.min_angle + (self.max_angle - self.min_angle) * t,
        };
        let angle = if angle.is_finite() { angle } else { 0.0 };
        Dab {
            size: px.min(size * 4),
            angle: (angle.round() as i32).rem_euclid(360),
            mix: self
                .amount(self.color, reading)
                .map(|t| (t * 255.0).round() as u8),
        }
    }

    /// The ink a dab paints with between `fg` and `bg`: the blend of the
    /// two, or a dither of them.
    pub fn ink(&self, fg: Rgba, bg: Rgba, mix: u8) -> Ink {
        match &self.dither {
            Some(pattern) => Ink::Dither {
                first: Color::Rgba(fg),
                second: Some(Color::Rgba(bg)),
                pattern: pattern.clone(),
                amount: mix,
            },
            None => Ink::Paint(Color::Rgba(blend(bg, fg, mix))),
        }
    }
}

/// `a` moved toward `b` by `t` of 255, channel by channel.
pub fn blend(a: Rgba, b: Rgba, t: u8) -> Rgba {
    let t = u32::from(t);
    let ch = |x: u8, y: u8| ((u32::from(x) * (255 - t) + u32::from(y) * t + 127) / 255) as u8;
    Rgba::new(ch(a.r, b.r), ch(a.g, b.g), ch(a.b, b.b), ch(a.a, b.a))
}

/// Measures how fast the pointer moves, as Aseprite does: the distance on
/// screen between readings over the time between them, smoothed so one
/// jumpy event does not swing the brush. Readings closer together than
/// [`Velocity::MIN_STEP`] seconds wait for the next, since several events
/// can arrive at once.
#[derive(Clone, Debug, Default)]
pub struct Velocity {
    last: Option<(f64, f64, f64)>,
    speed: f64,
}

impl Velocity {
    /// The speed, in screen pixels a second, that reads as full velocity.
    pub const FULL: f64 = 2000.0;
    /// The time over which a new speed fully replaces the old one.
    pub const SMOOTHING: f64 = 0.05;
    /// The shortest time between two readings that measures a speed.
    pub const MIN_STEP: f64 = 0.004;

    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the pointer's screen position at `t` seconds and returns the
    /// velocity reading, 0 to 1.
    pub fn feed(&mut self, x: f64, y: f64, t: f64) -> f32 {
        match self.last {
            None => self.last = Some((x, y, t)),
            Some((lx, ly, lt)) => {
                let dt = t - lt;
                if dt >= Self::MIN_STEP {
                    let v = ((x - lx).powi(2) + (y - ly).powi(2)).sqrt() / dt;
                    let a = (dt / Self::SMOOTHING).clamp(0.0, 1.0);
                    self.speed = (1.0 - a) * self.speed + a * v;
                    self.last = Some((x, y, t));
                }
            }
        }
        self.reading()
    }

    /// The velocity reading, 0 to 1.
    pub fn reading(&self) -> f32 {
        (self.speed / Self::FULL).clamp(0.0, 1.0) as f32
    }
}

/// The brushes a dynamic stroke has made, by size and angle, so a stroke
/// that comes back to a size or an angle does not turn its brush again.
#[derive(Clone, Debug, Default)]
pub struct DabBrushes {
    made: HashMap<(u32, i32), Brush>,
}

impl DabBrushes {
    pub fn new() -> Self {
        Self::default()
    }

    /// The brush of `dab`: `make` builds it at the dab's size, and it is
    /// then turned to the dab's angle.
    pub fn get(&mut self, dab: &Dab, make: impl FnOnce(u32) -> Brush) -> Brush {
        self.made
            .entry((dab.size, dab.angle))
            .or_insert_with(|| {
                let b = make(dab.size);
                if dab.angle == 0 {
                    b
                } else {
                    b.rotated(f64::from(dab.angle))
                }
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{BrushShape, Canvas, ToolContext};
    use pixl_document::Bayer;
    use pixl_document::color::{ColorMode, Palette};
    use pixl_document::document::Target;
    use pixl_document::geom::Point;
    use pixl_document::image::Raster;

    fn reading(pressure: f32, velocity: f32) -> Reading {
        Reading { pressure, velocity }
    }

    #[test]
    fn a_response_clips_at_its_thresholds_and_bends_between() {
        let r = Response {
            min: 0.2,
            max: 0.6,
            curve: 1.0,
        };
        assert_eq!(r.apply(0.1), 0.0);
        assert_eq!(r.apply(0.2), 0.0);
        assert!((r.apply(0.4) - 0.5).abs() < 1e-6);
        assert_eq!(r.apply(0.9), 1.0);
        let firm = Response {
            curve: 2.0,
            ..Response::default()
        };
        assert!((firm.apply(0.5) - 0.25).abs() < 1e-6);
        // Equal thresholds make a switch.
        let switch = Response {
            min: 0.5,
            max: 0.5,
            curve: 1.0,
        };
        assert_eq!(switch.apply(0.49), 0.0);
        assert_eq!(switch.apply(0.5), 1.0);
        assert_eq!(Response::default().apply(f32::NAN), 0.0);
    }

    #[test]
    fn pressure_sizes_the_dab_between_its_shares() {
        let d = Dynamics {
            size: Sensor::Pressure,
            ..Dynamics::default()
        };
        assert_eq!(d.dab(8, reading(1.0, 0.0)).size, 8);
        assert_eq!(d.dab(8, reading(0.5, 0.0)).size, 4);
        assert_eq!(d.dab(8, reading(0.0, 0.0)).size, 1, "never below a pixel");
        let curved = Dynamics {
            pressure: Response {
                curve: 2.0,
                ..Response::default()
            },
            ..d.clone()
        };
        assert_eq!(curved.dab(8, reading(0.5, 0.0)).size, 2);
        let floor = Dynamics {
            min_size: 0.5,
            max_size: 4.0,
            ..d.clone()
        };
        assert_eq!(floor.dab(8, reading(0.0, 0.0)).size, 4);
        assert_eq!(floor.dab(8, reading(1.0, 0.0)).size, 32);
        // Velocity drives nothing here.
        assert_eq!(d.dab(8, reading(1.0, 1.0)), d.dab(8, reading(1.0, 0.0)));
        let off = Dynamics::default();
        assert_eq!(off.dab(5, reading(0.0, 0.0)).size, 5);
        assert!(!off.is_dynamic() && d.is_dynamic());
    }

    #[test]
    fn velocity_turns_the_dab_and_colors_it() {
        let d = Dynamics {
            angle: Sensor::Velocity,
            min_angle: 0.0,
            max_angle: 90.0,
            color: Sensor::Velocity,
            ..Dynamics::default()
        };
        let slow = d.dab(4, reading(1.0, 0.0));
        let fast = d.dab(4, reading(1.0, 1.0));
        let mid = d.dab(4, reading(1.0, 0.5));
        assert_eq!((slow.angle, mid.angle, fast.angle), (0, 45, 90));
        assert_eq!(
            (slow.mix, mid.mix, fast.mix),
            (Some(0), Some(128), Some(255))
        );
        let back = Dynamics {
            min_angle: 0.0,
            max_angle: -30.0,
            ..d
        };
        assert_eq!(back.dab(4, reading(1.0, 1.0)).angle, 330);
        assert_eq!(Dynamics::default().dab(4, reading(0.0, 0.0)).mix, None);
    }

    #[test]
    fn the_color_runs_from_the_background_to_the_foreground() {
        let (fg, bg) = (Rgba::opaque(200, 0, 0), Rgba::opaque(0, 0, 100));
        let blended = Dynamics::default();
        assert_eq!(blended.ink(fg, bg, 255), Ink::Paint(Color::Rgba(fg)));
        assert_eq!(blended.ink(fg, bg, 0), Ink::Paint(Color::Rgba(bg)));
        assert_eq!(
            blended.ink(fg, bg, 128),
            Ink::Paint(Color::Rgba(Rgba::opaque(100, 0, 50)))
        );
        let dithered = Dynamics {
            dither: Some(DitherPattern::Bayer(Bayer::B4)),
            ..Dynamics::default()
        };
        assert!(matches!(dithered.ink(fg, bg, 128), Ink::Dither { .. }));
    }

    /// Paints a 16x16 square of `ink` on an indexed canvas and returns the
    /// palette entries it used.
    fn indices_painted(ink: &Ink) -> Vec<u32> {
        let palette = Palette::new(vec![
            Rgba::TRANSPARENT,
            Rgba::opaque(0, 0, 0),
            Rgba::opaque(255, 255, 255),
            Rgba::opaque(128, 128, 128),
        ])
        .unwrap();
        let mut raster = Raster::empty(ColorMode::Indexed, 16, 16, 0);
        let ctx = ToolContext::default();
        let target = Target {
            raster: &mut raster,
            palette: &palette,
            transparent_index: 0,
            alpha_locked: false,
        };
        let mut canvas = Canvas::new(target, &ctx);
        for y in 0..16 {
            for x in 0..16 {
                canvas.plot(Point::new(x, y), ink);
            }
        }
        let mut used: Vec<u32> = (0..16)
            .flat_map(|y| (0..16).map(move |x| (x, y)))
            .map(|(x, y)| raster.get_raw(x, y))
            .collect();
        used.sort();
        used.dedup();
        used
    }

    #[test]
    fn a_dithered_color_lays_down_only_the_two_colors() {
        let (fg, bg) = (Rgba::opaque(0, 0, 0), Rgba::opaque(255, 255, 255));
        for pattern in [
            DitherPattern::Bayer(Bayer::B4),
            DitherPattern::BlueNoise,
            DitherPattern::Custom {
                width: 2,
                height: 1,
                thresholds: std::sync::Arc::new(vec![64, 192]),
            },
        ] {
            let d = Dynamics {
                dither: Some(pattern.clone()),
                ..Dynamics::default()
            };
            assert_eq!(indices_painted(&d.ink(fg, bg, 128)), [1, 2], "{pattern:?}");
        }
        // A blend lands on the palette's middle grey instead.
        assert_eq!(indices_painted(&Dynamics::default().ink(fg, bg, 128)), [3]);
    }

    #[test]
    fn velocity_is_smoothed_screen_speed() {
        let mut v = Velocity::new();
        assert_eq!(v.feed(0.0, 0.0, 0.0), 0.0);
        // Too soon after the last reading: waits.
        assert_eq!(v.feed(500.0, 0.0, 0.001), 0.0);
        // 2000 px/s over a full smoothing step reads as full.
        let full = v.feed(100.0, 0.0, 0.05);
        assert!((full - 1.0).abs() < 1e-6, "{full}");
        // Holding still slows it down, by the share of the smoothing time.
        let slower = v.feed(100.0, 0.0, 0.075);
        assert!((slower - 0.5).abs() < 1e-6, "{slower}");
        let mut slow = Velocity::new();
        slow.feed(0.0, 0.0, 0.0);
        let r = slow.feed(10.0, 0.0, 0.05);
        assert!((r - 0.1).abs() < 1e-6, "{r}");
    }

    #[test]
    fn a_turned_brush_keeps_its_line_whole() {
        // A one-pixel line, five long.
        let line = Brush {
            shape: BrushShape::Custom {
                width: 5,
                height: 1,
                mask: std::sync::Arc::new(vec![true; 5]),
            },
            size: 5,
        };
        for degrees in [15.0, 30.0, 45.0, 60.0, 90.0, 135.0] {
            let turned = line.rotated(degrees);
            let px = turned.offsets();
            // A diagonal covers the length in fewer pixels.
            assert!(
                (3..=6).contains(&px.len()),
                "{degrees}: {} pixels",
                px.len()
            );
            // Every pixel touches another, sides or corners: one line.
            for p in &px {
                assert!(
                    px.iter()
                        .any(|q| q != p && (q.x - p.x).abs() <= 1 && (q.y - p.y).abs() <= 1),
                    "{degrees}: {p:?} stands alone"
                );
            }
        }
        // A square turned by quarters, and a circle, stay as they are.
        assert_eq!(Brush::square(3).rotated(90.0), Brush::square(3));
        assert_eq!(Brush::circle(5).rotated(33.0), Brush::circle(5));
        let diamond = Brush::square(5).rotated(45.0);
        assert!(matches!(diamond.shape, BrushShape::Custom { .. }));
        assert!(!diamond.offsets().contains(&Point::new(-3, -3)));
    }

    #[test]
    fn a_custom_brush_scales_to_its_size() {
        let b = Brush {
            shape: BrushShape::Custom {
                width: 4,
                height: 2,
                mask: std::sync::Arc::new(vec![true; 8]),
            },
            size: 4,
        };
        let big = b.resized(8);
        assert_eq!(big.size, 8);
        assert_eq!(big.offsets().len(), 32);
        assert_eq!(b.resized(2).offsets().len(), 2);
        assert_eq!(Brush::square(3).resized(5), Brush::square(5));
    }

    #[test]
    fn the_brushes_are_made_once_per_size_and_angle() {
        let mut made = DabBrushes::new();
        let mut calls = 0;
        let dab = Dab {
            size: 3,
            angle: 30,
            mix: None,
        };
        let a = made.get(&dab, |s| {
            calls += 1;
            Brush::square(s)
        });
        let b = made.get(&dab, |s| {
            calls += 1;
            Brush::square(s)
        });
        assert_eq!(a, b);
        assert_eq!(calls, 1);
        assert_eq!(a, Brush::square(3).rotated(30.0));
    }
}
