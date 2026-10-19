//! Several documents open at once, a tab each: opening, switching, closing,
//! what each tab keeps of its own, and the window closing over unsaved
//! edits.

pub mod common;

use std::path::PathBuf;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl::files::recents::{Recents, key};
use pixl::files::save_to;
use pixl::{Documents, Editor};
use pixl_document::native;
use pixl_document::{ColorMode, Document, Rgba};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("tabs")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An 8x8 document with one pixel of `color` at (1, 1).
fn dotted(color: Rgba) -> Document {
    let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
    let id = doc.layers()[0].id;
    doc.target(id, 0)
        .unwrap()
        .raster
        .set_raw(1, 1, color.to_u32());
    doc
}

fn pixel(doc: &Document, x: i32, y: i32) -> Rgba {
    let id = doc.layers()[0].id;
    doc.cel_raster(id, 0)
        .unwrap()
        .map_or(Rgba::TRANSPARENT, |r| Rgba::from_u32(r.get_raw(x, y)))
}

/// A 512x512 document with one pixel set: one buffer of 1 MiB.
fn big_dotted() -> Document {
    let mut doc = Document::new(512, 512, ColorMode::Rgba).unwrap();
    let id = doc.layers()[0].id;
    doc.target(id, 0)
        .unwrap()
        .raster
        .set_raw(100, 100, RED.to_u32());
    doc
}

const RED: Rgba = Rgba::opaque(200, 20, 20);

/// The New project dialog, filled in with a size and a name.
fn new_document(name: &'static str, w: &'static str, h: &'static str) -> Vec<Step> {
    vec![
        Step::Menu("file-new"),
        Step::Wait,
        Step::Text("new_name", name),
        Step::Text("new_width", w),
        Step::Text("new_height", h),
        Step::Click("new-create"),
        Step::Wait,
    ]
}

#[test]
fn a_new_document_opens_in_a_tab_of_its_own_and_ctrl_tab_goes_back() {
    let mut steps = new_document("second", "40", "24");
    let opened = drive(editor_over(dotted(RED)), steps.clone());
    common::save(&opened.frame, "tabs-two");
    assert_eq!(opened.tabs, ["untitled", "second"]);
    assert_eq!(opened.front, Some(1));
    assert_eq!((opened.doc.width(), opened.doc.height()), (40, 24));

    steps.extend([Step::Ctrl("Tab", false), Step::Wait]);
    let back = drive(editor_over(dotted(RED)), steps.clone());
    assert_eq!(back.front, Some(0));
    assert_eq!(
        pixel(&back.doc, 1, 1),
        RED,
        "the first document is as it was"
    );
    // Ctrl+Shift+Tab steps the other way, round the row.
    steps.extend([Step::Ctrl("Tab", true), Step::Wait]);
    let round = drive(editor_over(dotted(RED)), steps);
    assert_eq!(round.front, Some(1));
    assert_eq!(round.doc.width(), 40);
}

#[test]
fn a_click_on_a_tab_brings_its_document_to_the_front() {
    let mut steps = new_document("second", "40", "24");
    steps.extend([Step::ClickInside("tab-1"), Step::Wait]);
    let out = drive(editor_over(dotted(RED)), steps);
    assert_eq!(out.front, Some(0));
    assert_eq!(out.doc.width(), 8);
}

#[test]
fn each_tab_keeps_its_own_undo_history_and_view() {
    // Draw on the first document, open a second, draw there and undo it,
    // then go back: the first still has its stroke and its undo step.
    let mut steps = vec![
        Step::Key("1", false),
        Step::Wait,
        Step::Down(At::Doc(4, 4), PointerButton::Primary),
        Step::Up(At::Doc(4, 4), PointerButton::Primary),
        Step::Wait,
    ];
    steps.extend(new_document("second", "16", "16"));
    steps.extend([
        Step::Down(At::Doc(2, 2), PointerButton::Primary),
        Step::Up(At::Doc(2, 2), PointerButton::Primary),
        Step::Wait,
        Step::Ctrl("z", false),
        Step::Wait,
    ]);
    let second = drive(editor_over(dotted(RED)), steps.clone());
    assert_eq!(second.front, Some(1));
    assert_eq!(pixel(&second.doc, 2, 2), Rgba::TRANSPARENT, "undone");
    assert!(second.undo.is_empty(), "{:?}", second.undo);

    steps.extend([Step::Ctrl("Tab", false), Step::Wait, Step::Wait]);
    let first = drive(editor_over(dotted(RED)), steps);
    common::save(&first.frame, "tabs-dirty");
    assert_eq!(first.front, Some(0));
    assert_eq!(pixel(&first.doc, 4, 4), Rgba::BLACK, "the stroke stays");
    assert_eq!(first.undo.len(), 1, "{:?}", first.undo);
    assert_eq!(first.tabs, ["untitled *", "second *"]);
    // The view comes back at the 100 percent the first tab was set to,
    // not fitted again as a new document would be.
    let view = first.views.last().unwrap();
    assert_eq!(view.zoom, 1.0);
}

#[test]
fn closing_a_tab_with_edits_asks_and_closing_the_last_shows_the_start_screen() {
    let edited = vec![
        Step::Edit(|e| e.touch()),
        Step::Ctrl("w", false),
        Step::Wait,
    ];
    let asked = drive(editor_over(dotted(RED)), edited.clone());
    assert_eq!(
        asked.asking.as_deref(),
        Some("untitled has edits that are not saved.")
    );
    assert_eq!(asked.tabs.len(), 1);

    let mut kept = edited.clone();
    kept.extend([Step::Click("discard-no"), Step::Wait]);
    let kept = drive(editor_over(dotted(RED)), kept);
    assert!(kept.asking.is_none());
    assert_eq!(kept.tabs, ["untitled *"]);

    let mut closed = edited;
    closed.extend([Step::Click("discard-yes"), Step::Wait, Step::Wait]);
    let closed = drive(editor_over(dotted(RED)), closed);
    common::save(&closed.frame, "tabs-closed-last");
    assert!(closed.tabs.is_empty());
    assert!(closed.home_open, "no document left: the start screen");
}

#[test]
fn closing_the_front_tab_brings_the_next_one_and_a_middle_click_closes() {
    let mut steps = new_document("second", "16", "16");
    steps.extend(new_document("third", "24", "24"));
    steps.extend([
        Step::ClickInside("tab-2"),
        Step::Wait,
        Step::Ctrl("w", false),
        Step::Wait,
    ]);
    let out = drive(editor_over(dotted(RED)), steps.clone());
    assert_eq!(out.tabs, ["untitled", "third"]);
    assert_eq!(out.front, Some(1), "the tab after the closed one");
    assert_eq!(out.doc.width(), 24);

    // The press closes it; the button comes up where the tab was.
    steps.extend([
        Step::Wait,
        Step::Move(At::Element("tab-1")),
        Step::Wait,
        Step::Down(At::Element("tab-1"), PointerButton::Middle),
        Step::Up(
            At::Window(lumenui::glam::Vec2::new(60.0, 60.0)),
            PointerButton::Middle,
        ),
        Step::Wait,
    ]);
    let out = drive(editor_over(dotted(RED)), steps);
    assert_eq!(out.tabs, ["third"]);
}

#[test]
fn the_tab_menu_closes_the_others_and_those_to_the_right() {
    let mut steps = new_document("second", "16", "16");
    steps.extend(new_document("third", "24", "24"));
    let mut others = steps.clone();
    others.extend([
        Step::ContextMenu("tab-2"),
        Step::Wait,
        Step::Menu("tab-menu-close-others"),
        Step::Wait,
    ]);
    let out = drive(editor_over(dotted(RED)), others);
    assert_eq!(out.tabs, ["second"]);
    assert_eq!(out.doc.width(), 16);

    steps.extend([
        Step::ContextMenu("tab-1"),
        Step::Wait,
        Step::Menu("tab-menu-close-right"),
        Step::Wait,
    ]);
    let out = drive(editor_over(dotted(RED)), steps);
    assert_eq!(out.tabs, ["untitled"]);
    assert_eq!(out.doc.width(), 8);
}

#[test]
fn a_tab_drags_to_another_place_in_the_row() {
    let mut steps = new_document("second", "16", "16");
    steps.extend(new_document("third", "24", "24"));
    steps.extend([
        Step::Move(At::Element("tab-3")),
        Step::Wait,
        Step::Down(At::Element("tab-3"), PointerButton::Primary),
        Step::Move(At::Element("tab-2")),
        Step::Wait,
        Step::Move(At::Element("tab-1")),
        Step::Wait,
        Step::Up(At::Element("tab-1"), PointerButton::Primary),
        Step::Wait,
    ]);
    let out = drive(editor_over(dotted(RED)), steps);
    assert_eq!(out.tabs, ["third", "untitled", "second"]);
    assert_eq!(out.doc.width(), 24, "the dragged tab stays in front");
}

#[test]
fn opening_a_file_already_open_brings_its_tab_forward() {
    let dir = scratch("reopen");
    let path = dir.join("art.pixl");
    save_to(&mut Editor::new(dotted(RED)), &path).unwrap();
    let mut recents = Recents::load(dir.join("data"));
    recents.note(&path, &dotted(RED), pixl::files::recents::Thumb::IfStale);
    let app = || {
        pixl::starting(
            editor_over(Document::new(64, 64, ColorMode::Rgba).unwrap()),
            Recents::load(dir.join("data")),
            Some(path.clone()),
        )
    };
    let id = format!("recent-{}", key(&path));
    let id: &'static str = Box::leak(id.into_boxed_str());
    let mut steps = vec![Step::Wait];
    steps.extend(new_document("second", "16", "16"));
    steps.extend([
        Step::ClickInside("tab-home"),
        Step::Wait,
        Step::Wait,
        Step::ClickInside(id),
        Step::Wait,
    ]);
    let out = drive(app(), steps);
    assert!(!out.home_open);
    assert_eq!(out.tabs, ["art.pixl", "second"]);
    assert_eq!(out.front, Some(0));
}

#[test]
fn the_start_screen_opens_documents_in_tabs_and_home_stays_reachable() {
    let data = scratch("home").join("data");
    let app = pixl::starting(
        editor_over(Document::new(64, 64, ColorMode::Rgba).unwrap()),
        Recents::load(&data),
        None,
    );
    let mut steps = vec![
        Step::Wait,
        Step::Wait,
        Step::ClickInside("home-rig-demo"),
        Step::Wait,
    ];
    steps.extend([Step::ClickInside("tab-home"), Step::Wait, Step::Wait]);
    let out = drive(app, steps.clone());
    common::save(&out.frame, "tabs-home");
    assert!(out.home_open);
    assert_eq!(out.tabs, ["rig demo"]);

    // The rig demo opened again is a second untitled copy, in a tab of its
    // own.
    steps.extend([Step::ClickInside("home-rig-demo"), Step::Wait]);
    let app = pixl::starting(
        editor_over(Document::new(64, 64, ColorMode::Rgba).unwrap()),
        Recents::load(&data),
        None,
    );
    let out = drive(app, steps);
    assert!(!out.home_open);
    assert_eq!(out.tabs, ["rig demo", "rig demo"]);
    assert_eq!(out.front, Some(1));
}

#[test]
fn closing_the_window_over_unsaved_edits_asks_once_naming_them() {
    let mut steps = new_document("second", "16", "16");
    steps.extend(new_document("third", "16", "16"));
    steps.push(Step::Edit(|e| e.touch()));
    steps.extend([
        Step::Ctrl("Tab", false),
        Step::Wait,
        Step::Edit(|e| e.touch()),
    ]);
    steps.extend([Step::CloseWindow, Step::Wait]);
    let out = drive(editor_over(dotted(RED)), steps);
    assert_eq!(
        out.asking.as_deref(),
        Some("These documents have edits that are not saved: untitled, third.")
    );
}

#[test]
fn a_document_behind_its_tab_keeps_its_recovery_copy_up_to_date() {
    let dir = scratch("recovery");
    let path = dir.join("kept.pixl");
    save_to(&mut Editor::new(dotted(RED)), &path).unwrap();
    let app = pixl::starting(
        editor_over(Document::new(64, 64, ColorMode::Rgba).unwrap()),
        Recents::load(dir.join("data")),
        Some(path.clone()),
    );
    let mut steps = vec![Step::Wait, Step::Key("1", false), Step::Wait];
    steps.extend([
        Step::Down(At::Doc(5, 5), PointerButton::Primary),
        Step::Up(At::Doc(5, 5), PointerButton::Primary),
        Step::Wait,
    ]);
    steps.extend(new_document("second", "16", "16"));
    let out = drive(app, steps);
    assert_eq!(out.tabs, ["kept.pixl *", "second"]);
    let copy = native::load_file(&native::recovery_path(&path)).expect("a recovery copy");
    assert_eq!(pixel(&copy.document, 5, 5), Rgba::BLACK);
}

#[test]
fn documents_behind_their_tabs_keep_their_buffers_within_the_budget() {
    // Small documents: both keep their buffers, so going back draws at once.
    let mut steps = new_document("second", "16", "16");
    steps.extend([Step::Wait, Step::Wait]);
    let small = drive(editor_over(dotted(RED)), steps.clone());
    assert!(small.behind_bytes > 0);
    // Coming back to the first writes no tile: its buffers are as it left
    // them.
    steps.extend([Step::Ctrl("Tab", false), Step::Wait, Step::Wait]);
    let back = drive(editor_over(dotted(RED)), steps);
    assert_eq!(back.front, Some(0));
    assert_eq!(back.tiles_written, small.tiles_written);

    // With a budget the front document fills, the one behind lets go of
    // its buffers.
    let mut settings = pixl::Settings::default();
    settings.performance.buffer_budget_mib = 1;
    let mut steps = new_document("second", "512", "512");
    steps.extend([Step::Wait, Step::Wait]);
    let big = drive(
        editor_over(big_dotted()).insert_resource(settings.clone()),
        steps.clone(),
    );
    assert_eq!(big.tabs.len(), 2);
    assert_eq!(big.behind_bytes, 0);
    // Coming back to the first draws it anew.
    steps.extend([Step::Ctrl("Tab", false), Step::Wait, Step::Wait]);
    let back = drive(editor_over(big_dotted()).insert_resource(settings), steps);
    assert!(back.tiles_written > big.tiles_written);
}

#[test]
fn one_tab_per_document_by_default() {
    let docs = Documents::default();
    assert_eq!(docs.count(), 1);
    assert_eq!(Documents::empty().count(), 0);
}
