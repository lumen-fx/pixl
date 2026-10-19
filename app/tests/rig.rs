//! The Rig panel over a rendered editor: drawing and binding bones, posing
//! them into a frame, the pose library and rendering keyed poses.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl_document::rig::{BonePose, Easing, Pose, PoseKey, Vec2};
use pixl_document::{Binding, ColorMode, Document, LayerId, LayerKind, NewLayer, Placement, Rgba};

const L: PointerButton = PointerButton::Primary;

/// A 32x32 document of two frames with a 2x8 bar at x = 8..10, y = 8..16 on
/// frame 0.
fn doc() -> Document {
    let mut d = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let art = d.layers()[0].id;
    d.insert_frame(1, 100).unwrap();
    let t = d.target(art, 0).unwrap();
    for y in 8..16 {
        for x in 8..10 {
            t.raster.set_raw(x, y, Rgba::opaque(200, 60, 40).to_u32());
        }
    }
    d
}

fn art(doc: &Document) -> LayerId {
    doc.walk()
        .into_iter()
        .find(|(_, l)| l.is_raster())
        .map(|(_, l)| l.id)
        .unwrap()
}

fn skeleton(doc: &Document) -> LayerId {
    doc.walk()
        .into_iter()
        .find(|(_, l)| matches!(l.kind, LayerKind::Skeleton { .. }))
        .map(|(_, l)| l.id)
        .expect("a skeleton layer")
}

fn painted(doc: &Document, frame: usize) -> Vec<(i32, i32)> {
    let Some(r) = doc.cel_raster(art(doc), frame).unwrap() else {
        return Vec::new();
    };
    r.bounds()
        .points()
        .filter(|p| Rgba::from_u32(r.get_raw(p.x, p.y)).a > 0)
        .map(|p| (p.x, p.y))
        .collect()
}

fn drag(a: (i32, i32), b: (i32, i32)) -> Vec<Step> {
    let mut steps = vec![Step::Move(At::Doc(a.0, a.1)), Step::Wait];
    steps.push(Step::Down(At::Doc(a.0, a.1), L));
    for i in 1..=4 {
        let x = a.0 + (b.0 - a.0) * i / 4;
        let y = a.1 + (b.1 - a.1) * i / 4;
        steps.push(Step::Move(At::Doc(x, y)));
    }
    steps.push(Step::Up(At::Doc(b.0, b.1), L));
    steps.push(Step::Wait);
    steps
}

/// Draws a bone down the bar and binds the bar's layer to it.
fn rig_the_bar() -> Vec<Step> {
    let mut steps = vec![Step::Click("rig-new"), Step::Wait];
    steps.extend(drag((8, 8), (8, 15)));
    steps.push(Step::Click("rig-bind-layer"));
    steps.push(Step::Wait);
    steps
}

#[test]
fn bones_draw_bind_and_save_with_undo() {
    let out = drive(editor_over(doc()), rig_the_bar());
    let skel = skeleton(&out.doc);
    let rig = out.doc.rig(skel).unwrap();
    assert_eq!(rig.bones().len(), 1);
    let bone = &rig.bones()[0];
    assert_eq!(bone.name, "Bone");
    assert_eq!(bone.head, Vec2::new(8.5, 8.5));
    assert_eq!(bone.tail(), Vec2::new(8.5, 15.5));
    assert_eq!(rig.bindings().len(), 1);
    assert_eq!(rig.bindings()[0].layer, art(&out.doc));
    assert_eq!(rig.bindings()[0].pivot, Vec2::new(8.5, 8.5));
    assert_eq!(out.undo, vec!["New skeleton", "Add bone", "Bind layer"]);
    common::save(&out.frame, "rig-bones");
}

#[test]
fn posing_a_frame_renders_it_live_and_keys_the_pose() {
    let mut steps = rig_the_bar();
    steps.push(Step::Click("rig-mode-pose"));
    steps.push(Step::Edit(|e| e.frame = 1));
    steps.push(Step::Wait);
    // Turn the bone from pointing down to pointing right.
    steps.extend(drag((8, 15), (15, 8)));
    let out = drive(editor_over(doc()), steps);
    let skel = skeleton(&out.doc);
    let rig = out.doc.rig(skel).unwrap();
    let key = rig.frames()[1].key.as_ref().expect("the frame is keyed");
    let b = rig.bones()[0].id;
    assert_eq!(key.pose.get(b).rotation, -90.0);
    assert!(rig.frames()[1].generated);
    // The 2x8 bar turned a quarter about its top-left pixel's center: it
    // now lies along y = 7..9 from x = 8 to the right.
    let px = painted(&out.doc, 1);
    assert_eq!(px.len(), 16, "{px:?}");
    assert!(
        px.iter()
            .all(|p| (8..16).contains(&p.0) && (7..9).contains(&p.1)),
        "{px:?}"
    );
    assert_eq!(
        painted(&out.doc, 0).len(),
        16,
        "the rest frame is untouched"
    );
    assert_eq!(out.undo.last().map(String::as_str), Some("Turn bone"));
    common::save(&out.frame, "rig-pose-live");
}

#[test]
fn the_library_saves_mirrors_and_renders_new_frames() {
    let mut steps = rig_the_bar();
    steps.push(Step::Click("rig-mode-pose"));
    steps.push(Step::Wait);
    // On the rest frame the pose moves the bones only.
    steps.extend(drag((8, 15), (15, 8)));
    steps.push(Step::Text("rig_pose_name", "reach"));
    steps.push(Step::Click("rig-pose-save"));
    steps.push(Step::Wait);
    steps.push(Step::Click("rig-mirror"));
    steps.push(Step::Wait);
    steps.push(Step::Click("rig-render-new"));
    steps.push(Step::Wait);
    steps.push(Step::Wait);
    let out = drive(editor_over(doc()), steps);
    let skel = skeleton(&out.doc);
    let rig = out.doc.rig(skel).unwrap();
    let b = rig.bones()[0].id;
    assert_eq!(rig.library.len(), 1);
    assert_eq!(rig.library[0].name, "reach");
    assert_eq!(rig.library[0].pose.get(b).rotation, -90.0);
    assert_eq!(rig.pose.get(b).rotation, 90.0, "mirrored");
    assert_eq!(out.doc.frame_count(), 3);
    assert!(rig.frames()[1].generated);
    // Mirrored: the bar points left from the pivot.
    let px = painted(&out.doc, 1);
    assert_eq!(px.len(), 16, "{px:?}");
    assert!(px.iter().all(|p| p.0 <= 9), "{px:?}");
    assert_eq!(painted(&out.doc, 0).len(), 16);
    common::save(&out.frame, "rig-library");
}

#[test]
fn keyed_poses_render_eased_in_betweens() {
    let mut d = doc();
    for f in 2..5 {
        d.insert_frame(f, 100).unwrap();
    }
    let a = art(&d);
    let skel = d
        .add_layer(NewLayer::Skeleton, "Skeleton", Placement::default())
        .unwrap();
    let rig = d.rig_mut(skel).unwrap();
    let b = rig
        .add_bone("arm", None, Vec2::new(8.5, 8.5), Vec2::new(8.5, 15.5))
        .unwrap();
    rig.bind(Binding {
        layer: a,
        region: None,
        bone: b,
        pivot: Vec2::new(8.5, 8.5),
    })
    .unwrap();
    let key = |rotation| {
        let mut p = Pose::default();
        p.set(
            b,
            BonePose {
                rotation,
                offset: Vec2::ZERO,
            },
        );
        Some(PoseKey {
            pose: p,
            easing: Easing::Linear,
        })
    };
    rig.frame_mut(1).unwrap().key = key(0.0);
    rig.frame_mut(4).unwrap().key = key(-90.0);
    let steps = vec![
        Step::Click("rig-mode-pose"),
        Step::Wait,
        Step::Text("rig_from", "2"),
        Step::Text("rig_to", "5"),
        Step::Click("rig-render-keyed"),
        Step::Wait,
        Step::Wait,
    ];
    let out = drive(editor_over(d), steps);
    let rig = out.doc.rig(skel).unwrap();
    let generated: Vec<usize> = (0..5).filter(|&f| rig.frames()[f].generated).collect();
    assert_eq!(generated, vec![1, 2, 3, 4]);
    assert_eq!(out.message.as_deref(), Some("Rendered 4 frames"));
    assert_eq!(
        out.undo.last().map(String::as_str),
        Some("Render keyed poses")
    );
    // Frames 2 and 3 turn a third and two thirds of the way.
    for f in 1..5 {
        assert!(!painted(&out.doc, f).is_empty(), "frame {f}");
    }
    assert_ne!(painted(&out.doc, 2), painted(&out.doc, 3));
    common::save(&out.frame, "rig-keyed");
}

/// One part of [`figure`].
struct Part {
    name: &'static str,
    rect: pixl_document::Rect,
    color: Rgba,
    head: Vec2,
    tail: Vec2,
    parent: Option<usize>,
}

/// A 48x48 figure of six parts on frame 0 of two: torso, head, upper arm,
/// forearm and two legs, each on its own layer and bound to its own bone.
fn figure() -> (Document, LayerId, Vec<pixl_document::rig::BoneId>) {
    let mut d = Document::new(48, 48, ColorMode::Rgba).unwrap();
    d.insert_frame(1, 100).unwrap();
    let first = d.layers()[0].id;
    let skin = Rgba::opaque(232, 180, 140);
    let shirt = Rgba::opaque(60, 110, 200);
    let pants = Rgba::opaque(70, 60, 50);
    // Each part: its name, its rectangle, its color, its bone's head and
    // tail, and the index of the parent bone.
    let part = |name, (x, y, w, h), c, head: (f64, f64), tail: (f64, f64), parent| Part {
        name,
        rect: pixl_document::Rect::new(x, y, w, h),
        color: c,
        head: Vec2::new(head.0, head.1),
        tail: Vec2::new(tail.0, tail.1),
        parent,
    };
    let parts = [
        part(
            "torso",
            (20, 16, 8, 12),
            shirt,
            (24.0, 28.0),
            (24.0, 16.0),
            None,
        ),
        part(
            "head",
            (20, 8, 8, 8),
            skin,
            (24.0, 16.0),
            (24.0, 8.0),
            Some(0),
        ),
        part(
            "arm.R",
            (28, 17, 7, 3),
            shirt,
            (28.5, 18.5),
            (35.0, 18.5),
            Some(0),
        ),
        part(
            "forearm.R",
            (35, 17, 7, 3),
            skin,
            (35.0, 18.5),
            (41.5, 18.5),
            Some(2),
        ),
        part(
            "leg.L",
            (20, 28, 3, 12),
            pants,
            (21.5, 28.5),
            (21.5, 39.5),
            Some(0),
        ),
        part(
            "leg.R",
            (25, 28, 3, 12),
            pants,
            (26.5, 28.5),
            (26.5, 39.5),
            Some(0),
        ),
    ];
    let skel = d
        .add_layer(NewLayer::Skeleton, "Skeleton", Placement::default())
        .unwrap();
    let mut bones = Vec::new();
    for (i, p) in parts.into_iter().enumerate() {
        let name = p.name;
        let layer = if i == 0 {
            d.update_layer(first, |p| p.name = name.into()).unwrap();
            first
        } else {
            d.add_layer(
                NewLayer::Raster,
                name,
                Placement {
                    parent: None,
                    index: Some(d.layers().len() - 1),
                },
            )
            .unwrap()
        };
        let t = d.target(layer, 0).unwrap();
        t.raster.fill_rect_raw(&p.rect, p.color.to_u32());
        let rig = d.rig_mut(skel).unwrap();
        let b = rig
            .add_bone(name, p.parent.map(|i| bones[i]), p.head, p.tail)
            .unwrap();
        rig.bind(Binding {
            layer,
            region: None,
            bone: b,
            pivot: p.head,
        })
        .unwrap();
        bones.push(b);
    }
    (d, skel, bones)
}

#[test]
fn ik_bends_an_arm_to_the_pointer() {
    let (d, skel, bones) = figure();
    let mut steps = vec![
        Step::Click("rig-mode-pose"),
        Step::Wait,
        Step::Wait,
        Step::Click("rig-ik"),
        Step::Edit(|e| e.frame = 1),
        Step::Wait,
    ];
    // The forearm's end, dragged up toward the head.
    steps.extend(drag((41, 18), (33, 9)));
    let out = drive(editor_over(d), steps);
    let rig = out.doc.rig(skel).unwrap();
    let pose = &rig.frames()[1].key.as_ref().expect("keyed").pose;
    let tail = rig.posed(pose)[3].tail;
    assert!(
        (tail - Vec2::new(33.5, 9.5)).length() < 0.01,
        "the forearm's end reaches the pointer: {tail:?}"
    );
    assert_eq!(
        rig.posed(pose)[2].head,
        Vec2::new(28.5, 18.5),
        "the shoulder stays"
    );
    assert!(pose.get(bones[2]).rotation != 0.0 && pose.get(bones[3]).rotation != 0.0);
    assert_eq!(out.undo.last().map(String::as_str), Some("Pose (IK)"));
    common::save(&out.frame, "rig-ik");
}

#[test]
fn bones_mode_shows_bindings_and_pivots() {
    let (d, _, _) = figure();
    let mut steps = vec![Step::Click("rig-mode-bones"), Step::Wait];
    // Select the forearm by its middle.
    steps.extend(vec![
        Step::Move(At::Doc(38, 18)),
        Step::Wait,
        Step::Down(At::Doc(38, 18), L),
        Step::Up(At::Doc(38, 18), L),
        Step::Wait,
        Step::Wait,
    ]);
    let out = drive(editor_over(d), steps);
    assert_eq!(out.undo, Vec::<String>::new(), "selecting edits nothing");
    common::save(&out.frame, "rig-bones-mode");
}

#[test]
fn the_parent_dropdown_reparents_the_selected_bone() {
    let (d, skel, bones) = figure();
    let mut steps = vec![Step::Click("rig-mode-bones"), Step::Wait];
    steps.extend(vec![
        Step::Move(At::Doc(38, 18)),
        Step::Wait,
        Step::Down(At::Doc(38, 18), L),
        Step::Up(At::Doc(38, 18), L),
        Step::Wait,
        Step::Wait,
        // The torso is bone 1.
        Step::Text("rig_parent", "torso (1)"),
        Step::Wait,
        Step::Wait,
    ]);
    let out = drive(editor_over(d), steps);
    let rig = out.doc.rig(skel).unwrap();
    assert_eq!(rig.bone(bones[3]).unwrap().parent, Some(bones[0]));
    assert_eq!(out.undo, vec!["Reparent bone"]);
}
