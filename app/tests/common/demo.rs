//! The documents pixl ships, built from code: the rig demo, a small character
//! rigged and keyed as a walk, and the rig lesson the tutorial opens, the same
//! character with no skeleton. `tests/demo.rs` checks the files in
//! `assets/demo/` are what this builds.

use pixl_core::rig::{BonePose, Easing, NamedPose, Pose, PoseKey, RenderOptions, Vec2};
use pixl_core::{Binding, ColorMode, Document, LayerId, NewLayer, Placement, Rgba};

/// The canvas side of both documents.
pub const SIDE: u32 = 48;
/// The milliseconds each frame of the demo's walk lasts.
const WALK_MS: u32 = 100;
/// The frames of the lesson, and each one's milliseconds.
pub const LESSON_FRAMES: usize = 5;
const LESSON_MS: u32 = 150;

/// The front arm hangs from its shoulder to its hand along this column.
pub const ARM_X: i32 = 24;
pub const SHOULDER_Y: i32 = 19;
pub const HAND_Y: i32 = 27;

/// One body part: its layer's name, its pixels as rows of palette keys
/// placed with their top-left at `at`, and whether it is on the far side.
struct Part {
    layer: &'static str,
    at: (i32, i32),
    rows: &'static [&'static str],
    far: bool,
}

/// The character, facing right, in paint order from the back.
const PARTS: [Part; 6] = [
    Part {
        layer: "Back arm",
        at: (23, 19),
        rows: ARM,
        far: true,
    },
    Part {
        layer: "Back leg",
        at: (23, 30),
        rows: LEG,
        far: true,
    },
    Part {
        layer: "Body",
        at: (21, 18),
        rows: &[
            "oooooo", "obbbbo", "obbbBo", "obbbBo", "obbbBo", "obbbBo", "obbbBo", "obbbBo",
            "obbbBo", "obbbBo", "opppPo", "opppPo",
        ],
        far: false,
    },
    Part {
        layer: "Front leg",
        at: (23, 30),
        rows: LEG,
        far: false,
    },
    Part {
        layer: "Head",
        at: (20, 10),
        rows: &[
            ".oooooo.", "ohhhhhho", "ohhhssso", "ohhsseso", "ohssssso", ".ossssSo", "..osSSo.",
            "...oo...",
        ],
        far: false,
    },
    Part {
        layer: "Front arm",
        at: (23, 19),
        rows: ARM,
        far: false,
    },
];

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

/// The character on frame 0 of a document of `frames` frames, each part on
/// its own layer. Returns the document and the layers in [`PARTS`] order.
fn character(frames: usize, ms: u32) -> (Document, Vec<LayerId>) {
    let mut d = Document::new(SIDE, SIDE, ColorMode::Rgba).expect("a valid size");
    d.set_frame_duration(0, ms).expect("frame 0");
    for f in 1..frames {
        d.insert_frame(f, ms).expect("a new frame");
    }
    let mut layers = Vec::new();
    for (i, part) in PARTS.iter().enumerate() {
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
    let (mut d, layers) = character(LESSON_FRAMES, LESSON_MS);
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
    let (mut d, layers) = character(8, WALK_MS);
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
