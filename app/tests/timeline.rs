//! The timeline and the preview, driven over a rendered editor.

pub mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::{NamedKey, PointerButton};
use pixl_document::{ColorMode, Document, LayerId, LoopDirection, NewLayer, Placement, Rgba, Tag};

/// Three layers over six frames: a bouncing square on the top layer, a
/// ground line linked through every frame, and an empty middle layer.
fn animation() -> Document {
    let mut doc = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let ground = doc.layers()[0].id;
    doc.update_layer(ground, |p| p.name = "Ground".into())
        .unwrap();
    doc.add_layer(NewLayer::Raster, "Middle", Placement::default())
        .unwrap();
    let ball = doc
        .add_layer(NewLayer::Raster, "Ball", Placement::default())
        .unwrap();
    for f in 1..6 {
        doc.insert_frame(f, 100).unwrap();
    }
    {
        let t = doc.target(ground, 0).unwrap();
        for x in 0..32 {
            t.raster.set_raw(x, 30, Rgba::opaque(90, 160, 60).to_u32());
        }
    }
    for f in 1..6 {
        doc.link_cel(ground, 0, f).unwrap();
    }
    for f in 0..6 {
        let y = [4, 10, 18, 26, 18, 10][f];
        let t = doc.target(ball, f).unwrap();
        for dy in 0..4 {
            for dx in 0..4 {
                t.raster
                    .set_raw(14 + dx, y + dy, Rgba::opaque(220, 60, 60).to_u32());
            }
        }
    }
    doc.tags_mut().push(Tag {
        name: "fall".into(),
        from: 0,
        to: 3,
        direction: LoopDirection::Forward,
        repeat: 0,
        color: Rgba::opaque(0x4f, 0x74, 0xc0),
    });
    doc
}

#[test]
fn the_timeline_shows_layers_frames_and_tags() {
    let out = drive(editor_over(animation()), vec![Step::Wait, Step::Wait]);
    common::save(&out.frame, "timeline");
    assert_eq!(out.doc.frame_count(), 6);
    let _ = LayerId(0);
}

/// The ball layer of [`animation`].
const BALL: LayerId = LayerId(3);

#[test]
fn frames_are_added_copied_deleted_and_stepped_through() {
    let out = drive(
        editor_over(animation()),
        vec![
            Step::Click("tl-new"),
            Step::Key(".", false),
            Step::AltKey(NamedKey::ArrowLeft),
            Step::Key(",", false),
            Step::Click("tl-duplicate"),
            Step::Click("tl-delete"),
            Step::Click("tl-last"),
        ],
    );
    assert_eq!(out.doc.frame_count(), 7);
    assert_eq!(out.at.0, 6);
    assert_eq!(out.undo, ["New frame", "Copy frames", "Delete frames"]);
}

#[test]
fn a_drag_over_cels_selects_them_and_link_shares_their_pixels() {
    let out = drive(
        editor_over(animation()),
        vec![
            Step::Move(At::Element("cel-3-1")),
            Step::Down(At::Element("cel-3-1"), PointerButton::Primary),
            Step::Move(At::Element("cel-3-3")),
            Step::Up(At::Element("cel-3-3"), PointerButton::Primary),
            Step::Click("tl-link"),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "timeline-linked");
    assert_eq!(out.doc.linked_frames(BALL, 1).unwrap(), vec![1, 2, 3]);
    assert_eq!(out.doc.linked_frames(BALL, 0).unwrap(), vec![0]);
    assert_eq!(out.undo, ["Link cels"]);
}

#[test]
fn frames_move_by_their_numbers() {
    let doc = animation();
    let first = doc.cel(BALL, 0).unwrap().unwrap().image;
    let out = drive(
        editor_over(doc),
        vec![
            Step::Move(At::Element("tl-frame-0")),
            Step::Down(At::Element("tl-frame-0"), PointerButton::Primary),
            Step::Up(At::Element("tl-frame-0"), PointerButton::Primary),
            Step::Down(At::Element("tl-frame-0"), PointerButton::Primary),
            Step::Move(At::Element("tl-frame-3")),
            Step::Up(At::Element("tl-frame-3"), PointerButton::Primary),
            Step::Wait,
        ],
    );
    assert_eq!(out.doc.cel(BALL, 3).unwrap().unwrap().image, first);
    assert_eq!(out.at.0, 3, "the moved frame stays current");
    assert_eq!(out.undo, ["Move frames"]);
}

#[test]
fn the_duration_field_sets_the_selected_frames() {
    let out = drive(
        editor_over(animation()),
        vec![
            Step::Move(At::Element("tl-frame-1")),
            Step::Down(At::Element("tl-frame-1"), PointerButton::Primary),
            Step::Move(At::Element("tl-frame-2")),
            Step::Up(At::Element("tl-frame-2"), PointerButton::Primary),
            Step::Text("tl_duration", "250"),
            Step::Wait,
        ],
    );
    let ms: Vec<u32> = out.doc.frames().iter().map(|f| f.duration_ms).collect();
    assert_eq!(ms, [100, 250, 250, 100, 100, 100]);
    assert_eq!(out.undo, ["Frame duration"]);
}

#[test]
fn the_tag_dialog_adds_a_tag() {
    let out = drive(
        editor_over(animation()),
        vec![
            Step::Click("tl-tag"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Text("tag_name", "bounce"),
            Step::Text("tag_direction", "Ping-pong"),
            Step::Text("tag_from", "5"),
            Step::Text("tag_to", "6"),
            Step::Click("tag-color-4"),
            Step::Click("tag-ok"),
            Step::Wait,
            Step::ContextMenu("tl-tag-0-1"),
            Step::Menu("tm-delete"),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "timeline-tags");
    assert_eq!(out.doc.tags().len(), 1);
    let t = &out.doc.tags()[0];
    assert_eq!((t.name.as_str(), t.from, t.to), ("bounce", 4, 5));
    assert_eq!(t.direction, LoopDirection::PingPong);
    assert_ne!(t.color, Rgba::opaque(0x4f, 0x74, 0xc0));
    assert_eq!(out.undo, ["New tag", "Delete tag"]);
}

/// The furthest frame playback reached, and how many frame changes it made.
static FURTHEST: AtomicUsize = AtomicUsize::new(0);
static CHANGES: AtomicUsize = AtomicUsize::new(0);
static LAST: AtomicUsize = AtomicUsize::new(0);

fn watch_frame(e: &mut pixl::Editor) {
    FURTHEST.fetch_max(e.frame, Ordering::Relaxed);
    if LAST.swap(e.frame, Ordering::Relaxed) != e.frame {
        CHANGES.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn playback_loops_inside_the_tag() {
    let mut steps = vec![Step::Click("tl-play")];
    for _ in 0..16 {
        steps.push(Step::Sleep(40));
        steps.push(Step::Edit(watch_frame));
    }
    let out = drive(editor_over(animation()), steps);
    // Frames 0 to 3 are tagged; at 100 ms a frame, 640 ms of playing go
    // through the tag and start it over.
    assert_eq!(FURTHEST.load(Ordering::Relaxed), 3);
    assert!(CHANGES.load(Ordering::Relaxed) >= 5);
    assert!(out.undo.is_empty(), "playing is not an edit");
}

#[test]
fn the_onion_skin_ghosts_the_next_frame() {
    let pixel = |out: &common::drive::Outcome, x: i32, y: i32| {
        let view = out.views.last().unwrap();
        let at = pixl::tools::screen_of(view, pixl_document::Point::new(x, y));
        out.frame.pixel(at.x as u32, at.y as u32).unwrap()
    };
    let plain = drive(editor_over(animation()), vec![Step::Wait, Step::Wait]);
    let onion = drive(
        editor_over(animation()),
        vec![
            Step::Flag("onion_on", true),
            Step::Number("onion_opacity", 80.0),
            Step::Wait,
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&onion.frame, "timeline-onion");
    // Frame 1's ball, tinted toward the later-frame color, over the
    // checkerboard where frame 0 has nothing.
    let (before, after) = (pixel(&plain, 15, 11), pixel(&onion, 15, 11));
    assert_ne!(before, after);
    assert!(after[2] > after[0], "tinted blue: {after:?}");
    // The current frame's ball is untouched.
    assert_eq!(pixel(&plain, 15, 5), pixel(&onion, 15, 5));
}

#[test]
fn a_tag_end_drags_to_another_frame() {
    let out = drive(
        editor_over(animation()),
        vec![
            Step::Move(At::Element("tl-tag-0-3")),
            Step::Down(At::Element("tl-tag-0-3"), PointerButton::Primary),
            Step::Move(At::Element("tl-frame-5")),
            Step::Up(At::Element("tl-frame-5"), PointerButton::Primary),
            Step::Wait,
        ],
    );
    let t = &out.doc.tags()[0];
    assert_eq!((t.from, t.to), (0, 5));
    assert_eq!(out.undo, ["Tag frames"]);
}

#[test]
fn the_onion_skin_can_go_over_the_frame() {
    let pixel = |out: &common::drive::Outcome, x: i32, y: i32| {
        let view = out.views.last().unwrap();
        let at = pixl::tools::screen_of(view, pixl_document::Point::new(x, y));
        out.frame.pixel(at.x as u32, at.y as u32).unwrap()
    };
    // On frame 2 the ball is where it is again two frames later, so with
    // two frames after, a ghost lies on the current ball.
    let run = |above: bool| {
        drive(
            editor_over(animation()),
            vec![
                Step::Edit(|e| e.frame = 2),
                Step::Flag("onion_on", true),
                Step::Text("onion_after", "2"),
                Step::Number("onion_opacity", 100.0),
                Step::Flag("onion_above", above),
                Step::Wait,
                Step::Wait,
                Step::Wait,
            ],
        )
    };
    let (under, over) = (run(false), run(true));
    common::save(&over.frame, "timeline-onion-above");
    let ball = pixel(&under, 15, 19);
    assert_eq!(
        &ball[..3],
        &[220, 60, 60],
        "under, the ball covers its ghost"
    );
    let tinted = pixel(&over, 15, 19);
    assert_ne!(tinted, ball, "over, the ghost tints the ball");
    assert!(tinted[2] > ball[2]);
}
