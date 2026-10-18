//! The tutorials other than the rig's, over a rendered editor: each opens
//! from the Help menu in its own layout, and each step ticks off when the
//! driven input does what it asks, not before.

pub mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use common::drive::{At, Step, drive, editor_over};
use common::script::{Script, drag, trace};
use lumenui::lumen_core::input::{NamedKey, PointerButton};
use lumenui::prelude::*;
use pixl::lessons::{ANIMATION, LAYERS, LEAF, LESSONS, PALETTES, SELECTIONS, STAR, START};
use pixl::shipped::Shipped;
use pixl::tutorial::Lesson;
use pixl::{Editor, Recents, Workspace};
use pixl_core::{ColorMode, Document, LayerId, Rgba};

fn blank() -> App {
    editor_over(Document::new(16, 16, ColorMode::Rgba).unwrap())
}

fn titles(lesson: &Lesson) -> Vec<&'static str> {
    lesson.steps.iter().map(|s| s.title).collect()
}

/// A click with the pointer, pressed and let go over an element.
fn press_on(id: &'static str) -> Vec<Step> {
    vec![
        Step::Move(At::Element(id)),
        Step::Down(At::Element(id), PointerButton::Primary),
        Step::Up(At::Element(id), PointerButton::Primary),
    ]
}

fn opening(lesson: &Lesson) -> [Step; 3] {
    [Step::Menu(lesson.id), Step::Wait, Step::Wait]
}

#[test]
fn every_lesson_opens_from_help_in_its_layout() {
    for lesson in LESSONS {
        let layout: Arc<Mutex<String>> = Arc::default();
        let seen = Arc::clone(&layout);
        let app = blank().add_systems(TickStage::Systems, move |ws: Res<Workspace>| {
            *seen.lock().unwrap() = ws.live.name.clone()
        });
        // With the eraser in use, no lesson's first step is done yet.
        let mut steps = vec![Step::Key("e", false), Step::Wait];
        steps.extend(opening(lesson));
        let out = drive(app, steps);
        assert_eq!(out.title, lesson.document.name(), "{}", lesson.title);
        assert_eq!(out.doc, lesson.document.load().unwrap());
        let last = out.tutorial.last().unwrap();
        assert_eq!(last.lesson, Some(lesson.id));
        assert!(last.open && last.step == 0, "{}: {last:?}", lesson.title);
        assert_eq!(*layout.lock().unwrap(), lesson.layout, "{}", lesson.title);
    }
}

#[test]
fn another_lesson_from_help_ends_the_one_running() {
    let mut steps = opening(&START).to_vec();
    steps.extend(opening(&LAYERS));
    let out = drive(blank(), steps);
    assert_eq!(out.title, "layers lesson");
    let last = out.tutorial.last().unwrap();
    assert_eq!((last.lesson, last.step), (Some(LAYERS.id), 0));
}

#[test]
fn the_start_screen_starts_getting_started() {
    let app = pixl::starting(blank(), Recents::in_memory(), None);
    let out = drive(
        app,
        vec![
            Step::ClickInside("home-start-lesson"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert!(!out.home_open);
    assert_eq!(out.title, "getting started");
    let last = out.tutorial.last().unwrap();
    assert_eq!(last.lesson, Some(START.id));
    assert!(last.open);
}

/// Where the Getting started lesson saves in the tests: the file a save
/// dialog would have named.
fn save_here(e: &mut Editor) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lessons");
    std::fs::create_dir_all(&dir).unwrap();
    e.path = Some(dir.join("getting-started.pixl"));
}

#[test]
fn getting_started_ticks_off_step_by_step() {
    let mut s = Script::default();
    // The eraser is in use, so picking the pencil is the first thing to do.
    s.then([Step::Key("e", false), Step::Wait])
        .then(opening(&START))
        .ticks(0, vec![Step::Click("tool-pencil")])
        .ticks(1, vec![Step::Click("swatch-4")])
        .ticks(2, drag((13, 14), (19, 14)))
        // Picking the eraser erases nothing yet.
        .then([Step::Key("e", false), Step::Wait])
        .ticks(3, drag((14, 14), (16, 14)))
        .ticks(4, vec![Step::Ctrl("z", false)])
        .ticks(5, vec![Step::Ctrl("z", true)])
        .ticks(6, vec![Step::Wheel(At::Doc(16, 16), -40.0)])
        // Another zoom is not a pan.
        .then([Step::Wheel(At::Doc(16, 16), 40.0), Step::Wait])
        .ticks(7, vec![Step::Hold("d"), Step::Sleep(60)])
        .then([Step::Release("d"), Step::Wait])
        .then([Step::Edit(save_here)])
        .ticks(8, vec![Step::Ctrl("s", false)]);
    let out = drive(blank(), s.steps.clone());
    common::save(&out.frame, "lesson-start");
    s.check(&out, &titles(&START));
    assert!(!out.modified);
    assert_eq!(out.fg, Rgba::opaque(0xbe, 0x26, 0x33));
    // The stroke is back after the undo and redo of the erasing.
    assert_eq!(out.undo, ["Pencil", "Eraser"]);
}

#[test]
fn the_layers_lesson_ticks_off_step_by_step() {
    let doc = Shipped::LayersLesson.load().unwrap();
    let id = |name: &str| {
        doc.walk()
            .iter()
            .find(|(_, l)| l.props.name == name)
            .map(|(_, l)| l.id)
            .unwrap()
    };
    let (background, tree) = (id("Background"), id("Tree"));
    let added = LayerId(tree.0.max(background.0) + 1);
    let eye = |l: LayerId| -> &'static str { Box::leak(format!("layer-eye-{}", l.0).into()) };
    let mut s = Script::default();
    s.then(opening(&LAYERS))
        .ticks(0, vec![Step::Click("layer-add")])
        // The dialog fills its fields over two ticks.
        .then([
            Step::Click("layer-properties"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
        ])
        .then([Step::Text("lp_name", "Shadow"), Step::Wait])
        .ticks(1, vec![Step::Click("lp-ok")])
        .ticks(2, drag((12, 26), (20, 26)))
        .ticks(3, vec![Step::Click("layer-down")])
        .ticks(4, press_on(eye(tree)))
        .then([Step::Alt(true)])
        .ticks(5, press_on(eye(added)))
        // Back to what showed before: the tree is still hidden.
        .then(press_on(eye(added)))
        .then([Step::Wait, Step::Alt(false)])
        .ticks(6, press_on(eye(tree)))
        .ticks(7, vec![Step::Number("layer_opacity", 50.0)])
        .ticks(8, vec![Step::Menu("lm-merge")]);
    let out = drive(editor_over(doc.clone()), s.steps.clone());
    common::save(&out.frame, "lesson-layers");
    s.check(&out, &titles(&LAYERS));
    // The shadow merged into the background, under the tree.
    let names: Vec<&str> = out
        .doc
        .walk()
        .iter()
        .map(|(_, l)| l.props.name.as_str())
        .collect();
    assert_eq!(names, ["Background", "Tree"]);
    assert_ne!(
        out.doc.cel_raster(background, 0).unwrap(),
        doc.cel_raster(background, 0).unwrap()
    );
}

#[test]
fn the_selections_lesson_ticks_off_step_by_step() {
    let around = |at: (i32, i32), r: i32| {
        vec![
            (at.0 - r, at.1 - r),
            (at.0 + r, at.1 - r),
            (at.0 + r, at.1 + r),
            (at.0 - r, at.1 + r),
            (at.0 - r, at.1 - r + 1),
        ]
    };
    let mut s = Script::default();
    s.then(opening(&SELECTIONS))
        .ticks(0, vec![Step::Click("tool-select")])
        .ticks(1, drag((3, 2), (13, 14)))
        .then([Step::Click("tool-move"), Step::Wait])
        .ticks(2, drag((8, 9), (11, 9)))
        .ticks(3, vec![Step::KeyDown(NamedKey::Escape)])
        .then([Step::KeyUp(NamedKey::Escape), Step::Wait])
        // The lasso alone selects nothing yet.
        .then([Step::Click("tool-lasso"), Step::Wait])
        .ticks(4, trace(&around(STAR, 6)))
        .then([Step::Shift(true)])
        .ticks(5, trace(&around(LEAF, 7)))
        .then([Step::Shift(false), Step::Click("tool-pencil"), Step::Wait])
        .ticks(6, drag((15, STAR.1), (31, STAR.1)));
    let doc = Shipped::SelectionLesson.load().unwrap();
    let out = drive(editor_over(doc.clone()), s.steps.clone());
    common::save(&out.frame, "lesson-selections");
    s.check(&out, &titles(&SELECTIONS));
    assert_eq!(out.undo, ["Move", "Pencil"]);
    let layer = out.doc.layers()[0].id;
    let px = |d: &Document, x, y| d.cel_raster(layer, 0).unwrap().unwrap().get_raw(x, y);
    // The scribble painted inside the lasso and nothing outside it.
    assert_eq!(px(&out.doc, STAR.0, STAR.1), Rgba::BLACK.to_u32());
    assert_eq!(px(&out.doc, 30, STAR.1), px(&doc, 30, STAR.1));
    assert_eq!(px(&out.doc, 16, STAR.1), px(&doc, 16, STAR.1));
}

#[test]
fn the_animation_lesson_ticks_off_step_by_step() {
    let mut s = Script::default();
    s.then(opening(&ANIMATION))
        .ticks(0, vec![Step::Click("tl-new")])
        .ticks(1, vec![Step::Flag("onion_on", true)])
        .ticks(2, drag((13, 28), (19, 28)))
        .ticks(3, vec![Step::Click("tl-new")])
        .ticks(4, drag((15, 17), (17, 17)))
        .then(press_on("tl-frame-1"))
        .then([Step::Wait])
        .ticks(5, vec![Step::Text("tl_duration", "60")])
        .ticks(6, vec![Step::Click("tl-play")])
        .then([Step::Click("tl-play"), Step::Wait])
        .ticks(7, vec![Step::Menu("file-export-sheet")]);
    let out = drive(blank(), s.steps.clone());
    common::save(&out.frame, "lesson-animation");
    s.check(&out, &titles(&ANIMATION));
    let ms: Vec<u32> = out.doc.frames().iter().map(|f| f.duration_ms).collect();
    assert_eq!(ms, [120, 60, 120]);
}

#[test]
fn the_palettes_lesson_ticks_off_step_by_step() {
    // A dialog's buttons are there two ticks after it opens, and outlined
    // on the tick after that.
    let mut s = Script::default();
    s.then(opening(&PALETTES))
        .ticks(0, vec![Step::Click("swatch-11")])
        .then([Step::Menu("palette-ramp")])
        .then(std::iter::repeat_n(Step::Wait, 4))
        .ticks(1, vec![Step::Click("ramp-ok")])
        .ticks(2, vec![Step::Click("swatch-18")])
        .ticks(3, drag((17, 14), (21, 10)))
        .then([Step::Key("o", false), Step::Wait])
        .ticks(4, drag((17, 14), (19, 12)))
        .then([Step::Menu("image-indexed")])
        .then(std::iter::repeat_n(Step::Wait, 4))
        .ticks(5, vec![Step::Click("indexed-ok")])
        .then([
            Step::Until(|e| e.doc.mode() == ColorMode::Indexed),
            Step::Wait,
        ]);
    let out = drive(blank(), s.steps.clone());
    common::save(&out.frame, "lesson-palettes");
    s.check(&out, &titles(&PALETTES));
    assert_eq!(out.doc.mode(), ColorMode::Indexed);
    assert_eq!(
        out.doc.palette().len(),
        21,
        "the default 16 and a ramp of 5"
    );
}
