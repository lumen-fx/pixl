//! The rig tutorial over a rendered editor: Help > Rig tutorial opens the
//! lesson and its card, and each step ticks off when the driven input does
//! what it asks, not before.

pub mod common;

use std::sync::{Arc, Mutex};

use common::demo::{HAND, HEAD_TOP, HIPS, LESSON_FRAMES, NECK};
use common::drive::{At, Step, drive, editor_over};
use common::script::{Script, click, drag, press};
use lumenui::lumen_core::input::{NamedKey, PointerButton};
use lumenui::prelude::*;
use pixl::Workspace;
use pixl::files::shipped::Shipped;
use pixl::tutorial::lessons::RIG;
use pixl_document::rig::Rig;
use pixl_document::{ColorMode, Document, LayerKind};

const L: PointerButton = PointerButton::Primary;

/// The middle of the body bone, and of the head bone.
const BODY_MID: (i32, i32) = (HIPS.0, (HIPS.1 + NECK.1) / 2);
const HEAD_MID: (i32, i32) = (NECK.0, (NECK.1 + HEAD_TOP.1) / 2);

/// The id of a lesson layer's row in the timeline.
fn layer_row(name: &str) -> &'static str {
    let doc = Shipped::RigLesson.load().unwrap();
    let id = doc
        .walk()
        .into_iter()
        .find(|(_, l)| l.props.name == name)
        .map(|(_, l)| l.id)
        .expect("the layer");
    Box::leak(
        pixl::panels::layers::List::Timeline
            .row(id)
            .into_boxed_str(),
    )
}

fn skeleton_rig(doc: &Document) -> &Rig {
    let skel = doc
        .walk()
        .into_iter()
        .find(|(_, l)| matches!(l.kind, LayerKind::Skeleton { .. }))
        .map(|(_, l)| l.id)
        .expect("a skeleton layer");
    doc.rig(skel).unwrap()
}

#[test]
fn each_step_ticks_off_when_the_editor_shows_it_done() {
    let mut s = Script::default();
    s.then([Step::Menu("help-rig-tutorial"), Step::Wait, Step::Wait])
        .ticks(0, vec![Step::Click("rig-new")])
        .ticks(1, drag(HIPS, NECK))
        // The body is selected: a drag from its end draws a bone joined to
        // it.
        .ticks(2, drag(NECK, HEAD_TOP))
        // Closing the card keeps the lesson; Help shows it again.
        .then([Step::Click("tutor-close"), Step::Wait])
        .then([Step::Menu("help-rig-tutorial"), Step::Wait])
        .ticks(3, press(BODY_MID))
        .then([Step::Up(At::Doc(BODY_MID.0, BODY_MID.1), L), Step::Wait])
        .ticks(4, drag(NECK, HAND))
        // The arm is selected and its layer is the active one.
        .ticks(5, vec![Step::Click("rig-bind-layer")])
        .then(click(HEAD_MID))
        .then([Step::Click(layer_row("Head")), Step::Wait])
        .ticks(6, vec![Step::Click("rig-bind-layer")])
        .then(click(BODY_MID))
        .then([Step::Click(layer_row("Body")), Step::Wait])
        .ticks(7, vec![Step::Click("rig-bind-layer")])
        .ticks(8, vec![Step::Click("rig-mode-pose")])
        .ticks(9, vec![Step::Click("tl-next")])
        // Raise the arm by its hand.
        .ticks(10, drag(HAND, (HAND.0 - 1, HAND.1 - 6)))
        .then([Step::Click("tl-last"), Step::Wait])
        // The last frame holds the raised arm; the body is still at rest.
        .ticks(11, drag(BODY_MID, (BODY_MID.0 + 4, BODY_MID.1 + 1)))
        .ticks(12, vec![Step::Click("rig-render-keyed")])
        .ticks(13, vec![Step::Click("tl-play")]);
    let lesson = Document::new(16, 16, ColorMode::Rgba).unwrap();
    let out = drive(editor_over(lesson), s.steps.clone());

    assert_eq!(out.title, "rig lesson *");
    assert_eq!(out.doc.frame_count(), LESSON_FRAMES);
    let titles: Vec<&str> = RIG.steps.iter().map(|s| s.title).collect();
    s.check(&out, &titles);
    // The card hid while closed and showed again from the Help menu.
    let closed = out.tutorial.iter().position(|t| !t.open && t.step == 3);
    let reopened = out.tutorial.iter().rposition(|t| t.open && t.step == 3);
    assert!(closed.is_some() && reopened > closed, "{:?}", out.tutorial);

    // A skeleton of three bones: the head and the arm joined to the body.
    let rig = skeleton_rig(&out.doc);
    let bones = rig.bones();
    assert_eq!(bones.len(), 3);
    let (body, head, arm) = (bones[0].id, bones[1].id, bones[2].id);
    assert_eq!(bones[0].parent, None);
    assert_eq!((bones[1].parent, bones[2].parent), (Some(body), Some(body)));
    assert_eq!(
        bones[1].head,
        bones[0].tail(),
        "the head starts at the body's end"
    );
    assert_eq!(bones[2].head, bones[0].tail(), "so does the arm");
    let layer = |name: &str| {
        out.doc
            .walk()
            .into_iter()
            .find(|(_, l)| l.props.name == name)
            .map(|(_, l)| l.id)
            .unwrap()
    };
    let mut bound: Vec<_> = rig.bindings().iter().map(|b| (b.layer, b.bone)).collect();
    bound.sort();
    let mut want = vec![
        (layer("Front arm"), arm),
        (layer("Head"), head),
        (layer("Body"), body),
    ];
    want.sort();
    assert_eq!(bound, want);

    let keyed: Vec<usize> = (0..LESSON_FRAMES)
        .filter(|&f| rig.frames()[f].key.is_some())
        .collect();
    assert_eq!(keyed, vec![1, LESSON_FRAMES - 1]);
    assert!((1..LESSON_FRAMES).all(|f| rig.frames()[f].generated));
    // Turning the body on the last frame carried the head along: its tip
    // moved though the head's own pose did not change.
    let end = rig.pose_at(LESSON_FRAMES - 1).unwrap();
    let posed = |pose: &pixl_document::rig::Pose, b| {
        rig.posed(pose).into_iter().find(|x| x.id == b).unwrap()
    };
    let rest = pixl_document::rig::Pose::default();
    assert_eq!(end.get(head).rotation, 0.0);
    assert_ne!(end.get(body).rotation, 0.0);
    assert!((posed(&end, head).tail - posed(&rest, head).tail).length() > 2.0);
    common::save(&out.frame, "rig-tutorial");
}

#[test]
fn binding_outlines_the_bone_the_layer_and_bind_layer_in_turn() {
    let mut steps = vec![
        Step::Menu("help-rig-tutorial"),
        Step::Wait,
        Step::Wait,
        Step::Click("rig-new"),
        Step::Wait,
    ];
    steps.extend(drag(HIPS, NECK));
    steps.extend(drag(NECK, HEAD_TOP));
    steps.extend(click(BODY_MID));
    steps.extend(drag(NECK, HAND));
    steps.extend([Step::Click("rig-bind-layer"), Step::Wait]);
    steps.extend(click(HEAD_MID));
    steps.extend([Step::Click(layer_row("Head")), Step::Wait, Step::Wait]);
    let out = drive(
        editor_over(Document::new(16, 16, ColorMode::Rgba).unwrap()),
        steps,
    );
    common::save(&out.frame, "rig-tutorial-bind");
    let rig = skeleton_rig(&out.doc);
    let head = rig.bones()[1].id;
    // While binding the head: its bone first, then its layer, then the
    // button, each outlined on the panel that shows it.
    let targets: Vec<String> = out
        .tutorial
        .iter()
        .filter(|t| t.step == 6 && t.outlined)
        .filter_map(|t| t.target.clone())
        .fold(Vec::new(), |mut seen, t| {
            if seen.last() != Some(&t) {
                seen.push(t);
            }
            seen
        });
    assert_eq!(
        targets,
        [
            format!("rig-bone-{}", head.0),
            layer_row("Head").to_string(),
            "rig-bind-layer".to_string(),
        ]
    );
    // While drawing, nothing is outlined: the step says where to drag.
    assert!(
        out.tutorial
            .iter()
            .filter(|t| t.step == 1)
            .all(|t| t.target.is_none())
    );
}

#[test]
fn steps_wait_for_their_own_input() {
    let mut steps = vec![
        Step::Menu("help-rig-tutorial"),
        Step::Wait,
        Step::Wait,
        Step::Click("rig-new"),
        Step::Wait,
        // Leaving Bones mode outlines Bones again, and coming back unticks
        // nothing.
        Step::KeyDown(NamedKey::Escape),
        Step::KeyUp(NamedKey::Escape),
        Step::Wait,
        Step::Click("rig-mode-bones"),
        Step::Wait,
    ];
    steps.extend(drag(HIPS, NECK));
    // A click on empty canvas selects no bone, so the next bone drawn joins
    // nothing: the head is not drawn yet.
    steps.extend(click((8, 40)));
    steps.extend(drag((8, 4), (8, 12)));
    // Posing before the bones are joined and bound: Pose mode comes later.
    steps.extend([Step::Click("rig-mode-pose"), Step::Wait, Step::Wait]);
    let out = drive(
        editor_over(Document::new(16, 16, ColorMode::Rgba).unwrap()),
        steps,
    );
    assert_eq!(out.tutorial.last().unwrap().step, 2, "the head is next");
    let rig = skeleton_rig(&out.doc);
    assert_eq!(rig.bones().len(), 2);
    assert!(rig.bones().iter().all(|b| b.parent.is_none()));
    assert!(
        out.tutorial
            .iter()
            .any(|t| t.step == 1 && t.target.as_deref() == Some("rig-mode-bones")),
        "out of Bones mode, the drawing step outlines Bones"
    );
}

#[test]
fn opening_another_document_ends_the_lesson() {
    let out = drive(
        editor_over(Document::new(16, 16, ColorMode::Rgba).unwrap()),
        vec![
            Step::Menu("help-rig-tutorial"),
            Step::Wait,
            Step::Wait,
            Step::Menu("help-rig-demo"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert_eq!(out.title, "rig demo");
    assert!(out.tutorial.iter().any(|t| t.open), "the card showed");
    assert!(
        !out.tutorial.last().unwrap().open,
        "and went with the lesson"
    );
}

#[test]
fn the_lesson_works_in_the_rig_layout() {
    let layout: Arc<Mutex<String>> = Arc::default();
    let seen = Arc::clone(&layout);
    let app = editor_over(Document::new(16, 16, ColorMode::Rgba).unwrap())
        .add_systems(TickStage::Systems, move |ws: Res<Workspace>| {
            *seen.lock().unwrap() = ws.live.name.clone()
        });
    drive(
        app,
        vec![Step::Menu("help-rig-tutorial"), Step::Wait, Step::Wait],
    );
    assert_eq!(*layout.lock().unwrap(), "Rig");
}
