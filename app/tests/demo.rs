//! The documents pixl ships: the files in `assets/demo/` are what
//! `common/demo.rs` builds, and the rig demo opens as an untitled copy that
//! walks when played.
//!
//! After a change to the builder or to the `.pixl` format, write the files
//! again with `PIXL_WRITE_DEMO=1 cargo test -p pixl --test demo`.

pub mod common;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::demo::{
    animation_lesson, layers_lesson, palette_lesson, rig_demo, rig_lesson, selection_lesson,
    start_lesson,
};
use common::drive::{Step, drive, editor_over};
use pixl::Recents;
use pixl::files::shipped::Shipped;
use pixl_document::native::{self, SaveOptions};
use pixl_document::{ColorMode, CompositeOptions, Document, composite_frame};

#[test]
fn the_shipped_documents_are_what_the_code_builds() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo");
    for (which, file, build) in [
        (
            Shipped::RigDemo,
            "rig-demo.pixl",
            rig_demo as fn() -> Document,
        ),
        (Shipped::RigLesson, "rig-lesson.pixl", rig_lesson),
        (Shipped::StartLesson, "start-lesson.pixl", start_lesson),
        (Shipped::LayersLesson, "layers-lesson.pixl", layers_lesson),
        (
            Shipped::SelectionLesson,
            "selections-lesson.pixl",
            selection_lesson,
        ),
        (
            Shipped::AnimationLesson,
            "animation-lesson.pixl",
            animation_lesson,
        ),
        (
            Shipped::PaletteLesson,
            "palettes-lesson.pixl",
            palette_lesson,
        ),
    ] {
        let bytes = native::save(&build(), SaveOptions::default());
        assert_eq!(
            bytes,
            native::save(&build(), SaveOptions::default()),
            "{file} builds the same twice"
        );
        if std::env::var_os("PIXL_WRITE_DEMO").is_some() {
            native::write_atomic(&dir.join(file), &bytes).expect("write the file");
            continue;
        }
        assert!(
            bytes == which.bytes(),
            "assets/demo/{file} is not what tests/common/demo.rs builds: write it again with \
             PIXL_WRITE_DEMO=1 cargo test -p pixl --test demo"
        );
        let loaded = native::load(which.bytes()).expect("it opens");
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }
}

#[test]
fn every_frame_of_the_walk_differs_from_the_next() {
    let doc = Shipped::RigDemo.load().unwrap();
    let frames: Vec<_> = (0..doc.frame_count())
        .map(|f| composite_frame(&doc, f, &CompositeOptions::default()))
        .collect();
    assert_eq!(frames.len(), 8);
    for f in 0..frames.len() {
        let next = (f + 1) % frames.len();
        assert_ne!(frames[f], frames[next], "frames {f} and {next}");
    }
    // The lesson's frames all show the character at rest.
    let lesson = Shipped::RigLesson.load().unwrap();
    let rest = composite_frame(&lesson, 0, &CompositeOptions::default());
    assert!(rest.iter().any(|c| c.a > 0));
    for f in 1..lesson.frame_count() {
        assert_eq!(
            composite_frame(&lesson, f, &CompositeOptions::default()),
            rest
        );
    }
}

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
fn help_opens_an_untitled_demo_that_walks_when_played() {
    let mut steps = vec![
        Step::Menu("help-rig-demo"),
        Step::Wait,
        Step::Edit(|e| {
            assert_eq!(e.path, None, "the demo has no file");
            assert!(!e.is_modified());
        }),
        Step::Click("tl-play"),
    ];
    for _ in 0..12 {
        steps.push(Step::Sleep(40));
        steps.push(Step::Edit(watch_frame));
    }
    let blank = Document::new(16, 16, ColorMode::Rgba).unwrap();
    let out = drive(editor_over(blank), steps);
    assert_eq!(out.title, "rig demo");
    assert_eq!(
        out.doc,
        Shipped::RigDemo.load().unwrap(),
        "playing edits nothing"
    );
    assert!(out.undo.is_empty());
    // At 100 ms a frame, about half a second of playing passes frames.
    assert!(FURTHEST.load(Ordering::Relaxed) >= 3);
    assert!(CHANGES.load(Ordering::Relaxed) >= 3);
    common::save(&out.frame, "rig-demo");
}

#[test]
fn the_start_screen_opens_the_demo() {
    let app = pixl::starting(
        editor_over(Document::new(64, 64, ColorMode::Rgba).unwrap()),
        Recents::in_memory(),
        None,
    );
    let out = drive(app, vec![Step::Click("home-rig-demo"), Step::Wait]);
    assert!(!out.home_open);
    assert_eq!(out.title, "rig demo");
    assert_eq!(out.doc.frame_count(), 8);
    assert!(out.recents.is_empty(), "an untitled copy is not listed");
}
