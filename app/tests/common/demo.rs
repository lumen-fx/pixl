//! The documents pixl ships, built from code: the rig demo, a small character
//! rigged and keyed as a walk; the rig lesson the rig tutorial opens, the same
//! character with no skeleton, its arm held out and no far arm; and the small
//! canvases the other tutorials open. `tests/demo.rs` checks the files in
//! `assets/demo/` are what this builds.

use pixl_core::rig::{BonePose, Easing, NamedPose, Pose, PoseKey, RenderOptions, Vec2};
use pixl_core::{Binding, ColorMode, Document, LayerId, NewLayer, Placement, Rgba};

/// The canvas side of the rig demo and the rig lesson.
pub const SIDE: u32 = 48;
/// The milliseconds each frame of the demo's walk lasts.
const WALK_MS: u32 = 100;
/// The frames of the lesson, and each one's milliseconds.
pub const LESSON_FRAMES: usize = 5;
const LESSON_MS: u32 = 150;

/// The demo's front arm hangs from its shoulder to its hand along this
/// column.
const ARM_X: i32 = 24;
const SHOULDER_Y: i32 = 19;
const HAND_Y: i32 = 27;

/// The lesson's landmarks, the pixels the tutorial's bones run between: the
/// body from the hips up to the neck, the head from the neck up to its top,
/// and the arm held out from the neck to the hand.
pub const HIPS: (i32, i32) = (24, 29);
pub const NECK: (i32, i32) = (24, 18);
pub const HEAD_TOP: (i32, i32) = (24, 10);
pub const HAND: (i32, i32) = (31, 19);

/// One body part: its layer's name, its pixels as rows of palette keys
/// placed with their top-left at `at`, and whether it is on the far side.
struct Part {
    layer: &'static str,
    at: (i32, i32),
    rows: &'static [&'static str],
    far: bool,
}

/// The character, facing right, in paint order from the back.
const PARTS: [Part; 6] = [BACK_ARM, BACK_LEG, BODY, FRONT_LEG, HEAD, FRONT_ARM];

/// The lesson's character: no far arm, and the near one held out in front.
const LESSON_PARTS: [Part; 5] = [
    BACK_LEG,
    BODY,
    FRONT_LEG,
    HEAD,
    Part {
        layer: "Front arm",
        at: (24, 18),
        rows: &["ooooooooo", "bbbbbbsso", "ooooooooo"],
        far: false,
    },
];

const BACK_ARM: Part = Part {
    layer: "Back arm",
    at: (23, 19),
    rows: ARM,
    far: true,
};
const BACK_LEG: Part = Part {
    layer: "Back leg",
    at: (23, 30),
    rows: LEG,
    far: true,
};
const BODY: Part = Part {
    layer: "Body",
    at: (21, 18),
    rows: &[
        "oooooo", "obbbbo", "obbbBo", "obbbBo", "obbbBo", "obbbBo", "obbbBo", "obbbBo", "obbbBo",
        "obbbBo", "opppPo", "opppPo",
    ],
    far: false,
};
const FRONT_LEG: Part = Part {
    layer: "Front leg",
    at: (23, 30),
    rows: LEG,
    far: false,
};
const HEAD: Part = Part {
    layer: "Head",
    at: (20, 10),
    rows: &[
        ".oooooo.", "ohhhhhho", "ohhhssso", "ohhsseso", "ohssssso", ".ossssSo", "..osSSo.",
        "...oo...",
    ],
    far: false,
};
const FRONT_ARM: Part = Part {
    layer: "Front arm",
    at: (23, 19),
    rows: ARM,
    far: false,
};

const ARM: &[&str] = &[
    "obo", "obo", "obo", "obo", "obo", "obo", "obo", "oso", "oso", "ooo",
];

const LEG: &[&str] = &[
    "opo..", "opo..", "opo..", "opo..", "opo..", "opo..", "opo..", "opo..", "opo..", "okkko",
    "okkko", "ooooo",
];

/// The color of a palette key, darker on the far side.
fn color(key: char, far: bool) -> Option<Rgba> {
    let c = match key {
        'o' => Rgba::opaque(42, 31, 45),
        'e' => Rgba::opaque(42, 31, 45),
        's' => Rgba::opaque(242, 194, 155),
        'S' => Rgba::opaque(215, 154, 116),
        'h' => Rgba::opaque(107, 61, 38),
        'b' => Rgba::opaque(74, 127, 208),
        'B' => Rgba::opaque(51, 90, 156),
        'p' => Rgba::opaque(75, 68, 88),
        'P' => Rgba::opaque(53, 48, 65),
        'k' => Rgba::opaque(110, 74, 50),
        _ => return None,
    };
    if !far || key == 'o' {
        return Some(c);
    }
    let dim = |v: u8| (u16::from(v) * 7 / 10) as u8;
    Some(Rgba::opaque(dim(c.r), dim(c.g), dim(c.b)))
}

/// The character made of `parts` on frame 0 of a document of `frames`
/// frames, each part on its own layer. Returns the document and the layers
/// in `parts` order.
fn character(parts: &[Part], frames: usize, ms: u32) -> (Document, Vec<LayerId>) {
    let mut d = Document::new(SIDE, SIDE, ColorMode::Rgba).expect("a valid size");
    d.set_frame_duration(0, ms).expect("frame 0");
    for f in 1..frames {
        d.insert_frame(f, ms).expect("a new frame");
    }
    let mut layers = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let layer = if i == 0 {
            let first = d.layers()[0].id;
            d.update_layer(first, |p| p.name = part.layer.into())
                .expect("the first layer");
            first
        } else {
            d.add_layer(NewLayer::Raster, part.layer, Placement::default())
                .expect("a layer")
        };
        let t = d.target(layer, 0).expect("frame 0");
        for (y, row) in part.rows.iter().enumerate() {
            for (x, key) in row.chars().enumerate() {
                if let Some(c) = color(key, part.far) {
                    t.raster
                        .set_raw(part.at.0 + x as i32, part.at.1 + y as i32, c.to_u32());
                }
            }
        }
        layers.push(layer);
    }
    (d, layers)
}

/// The lesson: the character on every frame (each frame's cels linked to
/// frame 0's) and no skeleton yet.
pub fn rig_lesson() -> Document {
    let (mut d, layers) = character(&LESSON_PARTS, LESSON_FRAMES, LESSON_MS);
    for layer in layers {
        for f in 1..LESSON_FRAMES {
            d.link_cel(layer, 0, f).expect("a frame");
        }
    }
    d
}

/// The demo: the character on a skeleton, its walk keyed on eight frames
/// and rendered into them, with the contact and passing poses saved.
pub fn rig_demo() -> Document {
    let (mut d, layers) = character(&PARTS, 8, WALK_MS);
    let skel = d
        .add_layer(NewLayer::Skeleton, "Skeleton", Placement::default())
        .expect("a skeleton layer");
    let rig = d.rig_mut(skel).expect("a rig");
    let x = f64::from(ARM_X) + 0.5;
    let v = |y: f64| Vec2::new(x, y);
    let body = rig
        .add_bone("body", None, v(29.5), v(18.5))
        .expect("a bone");
    let bone = |rig: &mut pixl_core::rig::Rig, name, head, tail| {
        rig.add_bone(name, Some(body), v(head), v(tail))
            .expect("a bone")
    };
    let (shoulder, hand) = (f64::from(SHOULDER_Y) + 0.5, f64::from(HAND_Y) + 0.5);
    let head = bone(rig, "head", 17.5, 10.5);
    let arm_r = bone(rig, "arm.R", shoulder, hand);
    let leg_r = bone(rig, "leg.R", 30.5, 40.5);
    let leg_l = bone(rig, "leg.L", 30.5, 40.5);
    let arm_l = bone(rig, "arm.L", shoulder, hand);
    // PARTS order: the back arm and leg are the right ones, seen from the
    // character's left.
    let bones = [arm_r, leg_r, body, leg_l, head, arm_l];
    for (layer, b) in layers.iter().zip(bones) {
        let pivot = rig.bone(b).expect("the bone").head;
        rig.bind(Binding {
            layer: *layer,
            region: None,
            bone: b,
            pivot,
        })
        .expect("a binding");
    }
    // A downward bone turned by a negative angle swings forward.
    let turn = |rotation| BonePose {
        rotation,
        offset: Vec2::ZERO,
    };
    let contact = |front: f64| {
        let mut p = Pose::default();
        p.set(leg_l, turn(-front));
        p.set(leg_r, turn(front));
        p.set(arm_l, turn(front * 0.8));
        p.set(arm_r, turn(-front * 0.8));
        // The legs apart are shorter: the body drops a pixel.
        p.set(
            body,
            BonePose {
                rotation: 0.0,
                offset: Vec2::new(0.0, 1.0),
            },
        );
        p
    };
    let passing = Pose::default();
    let (a, b) = (contact(25.0), contact(-25.0));
    let keys = [
        (0, passing.clone()),
        (2, a.clone()),
        (4, passing.clone()),
        (6, b.clone()),
        (7, Pose::lerp(&b, &passing, 0.5)),
    ];
    for (frame, pose) in keys {
        rig.frame_mut(frame).expect("a frame").key = Some(PoseKey {
            pose,
            easing: Easing::Linear,
        });
    }
    rig.library.push(NamedPose {
        name: "Contact".into(),
        pose: a,
    });
    rig.library.push(NamedPose {
        name: "Passing".into(),
        pose: passing,
    });
    d.render_keyed(skel, 0, 7, RenderOptions::default())
        .expect("the walk renders");
    d
}

/// Draws `rows` of keys onto frame 0 of `layer`, with the top-left at `at`;
/// a key `colors` does not name stays empty.
fn draw(d: &mut Document, layer: LayerId, at: (i32, i32), rows: &[&str], colors: &[(char, Rgba)]) {
    let t = d.target(layer, 0).expect("frame 0");
    for (y, row) in rows.iter().enumerate() {
        for (x, key) in row.chars().enumerate() {
            if let Some((_, c)) = colors.iter().find(|(k, _)| *k == key) {
                t.raster
                    .set_raw(at.0 + x as i32, at.1 + y as i32, c.to_u32());
            }
        }
    }
}

/// A filled circle of `color` around `at`.
fn disc(d: &mut Document, layer: LayerId, at: (i32, i32), r: i32, color: Rgba) {
    let t = d.target(layer, 0).expect("frame 0");
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r + r / 2 {
                t.raster.set_raw(at.0 + x, at.1 + y, color.to_u32());
            }
        }
    }
}

/// The side of the tutorials' lessons other than the rig's.
pub const LESSON_SIDE: u32 = 32;

/// A lesson canvas with its one layer named `layer`.
fn canvas(layer: &str) -> (Document, LayerId) {
    let mut d = Document::new(LESSON_SIDE, LESSON_SIDE, ColorMode::Rgba).expect("a valid size");
    let id = d.layers()[0].id;
    d.update_layer(id, |p| p.name = layer.into())
        .expect("the first layer");
    (d, id)
}

/// Getting started: the outline of a heart to color in.
pub fn start_lesson() -> Document {
    let (mut d, layer) = canvas("Layer 1");
    let heart = [
        "..ooo...ooo..",
        ".o...o.o...o.",
        "o.....o.....o",
        "o...........o",
        "o...........o",
        ".o.........o.",
        "..o.......o..",
        "...o.....o...",
        "....o...o....",
        ".....o.o.....",
        "......o......",
    ];
    let pink = Rgba::opaque(0xe0, 0x6f, 0x8b);
    draw(&mut d, layer, (10, 11), &heart, &[('o', pink)]);
    d
}

/// The layers lesson: a tree on its own layer over a sky and grass.
pub fn layers_lesson() -> Document {
    let (mut d, background) = canvas("Background");
    {
        let t = d.target(background, 0).expect("frame 0");
        for y in 0..LESSON_SIDE as i32 {
            let c = if y < 24 {
                Rgba::opaque(0x31, 0xa2, 0xf2)
            } else {
                Rgba::opaque(0x44, 0x89, 0x1a)
            };
            for x in 0..LESSON_SIDE as i32 {
                t.raster.set_raw(x, y, c.to_u32());
            }
        }
    }
    let tree = d
        .add_layer(NewLayer::Raster, "Tree", Placement::default())
        .expect("a layer");
    let trunk = Rgba::opaque(0x49, 0x3c, 0x2b);
    draw(&mut d, tree, (15, 15), &["tt"; 11], &[('t', trunk)]);
    disc(&mut d, tree, (16, 11), 6, Rgba::opaque(0xa3, 0xce, 0x27));
    d
}

/// The selections lesson: an apple, a star and a leaf to select.
pub fn selection_lesson() -> Document {
    use pixl::lessons::{APPLE, LEAF, STAR};
    let (mut d, layer) = canvas("Picture");
    disc(&mut d, layer, APPLE, 4, Rgba::opaque(0xbe, 0x26, 0x33));
    let stem = Rgba::opaque(0x49, 0x3c, 0x2b);
    draw(
        &mut d,
        layer,
        (APPLE.0, APPLE.1 - 6),
        &["s", "s"],
        &[('s', stem)],
    );
    let star = [
        "....y....",
        "....y....",
        "...yyy...",
        "yyyyyyyyy",
        ".yyyyyyy.",
        "..yyyyy..",
        "..yy.yy..",
        ".yy...yy.",
        "y.......y",
    ];
    let yellow = Rgba::opaque(0xf7, 0xe2, 0x6b);
    draw(
        &mut d,
        layer,
        (STAR.0 - 4, STAR.1 - 4),
        &star,
        &[('y', yellow)],
    );
    let leaf = [
        "....gggg.....",
        "..gggggggg...",
        "gggggggggggg.",
        "..gggggggg...",
        "....gggg.....",
    ];
    let green = Rgba::opaque(0x44, 0x89, 0x1a);
    draw(
        &mut d,
        layer,
        (LEAF.0 - 6, LEAF.1 - 2),
        &leaf,
        &[('g', green)],
    );
    d
}

/// The milliseconds the animation lesson's frame lasts.
pub const BOUNCE_MS: u32 = 120;

/// The animation lesson: one frame, the ball at the top.
pub fn animation_lesson() -> Document {
    let (mut d, layer) = canvas("Ball");
    d.set_frame_duration(0, BOUNCE_MS).expect("frame 0");
    disc(&mut d, layer, (16, 6), 3, Rgba::opaque(0xbe, 0x26, 0x33));
    d
}

/// The palettes lesson: a pot with a bare stem, its clay in colors the
/// palette does not have.
pub fn palette_lesson() -> Document {
    let (mut d, layer) = canvas("Plant");
    let pot = [
        "lllllllllll",
        "ddddddddddd",
        ".lllllllll.",
        ".llllllldl.",
        "..lllllld..",
        "..llllldd..",
        "...ddddd...",
    ];
    let clay = [
        ('l', Rgba::opaque(196, 112, 74)),
        ('d', Rgba::opaque(141, 72, 50)),
    ];
    draw(&mut d, layer, (11, 24), &pot, &clay);
    let stem = Rgba::opaque(0x2f, 0x48, 0x4e);
    draw(&mut d, layer, (16, 12), &["s"; 12], &[('s', stem)]);
    d
}
