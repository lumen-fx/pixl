//! The start screen: the recent documents with their thumbnails, the New
//! project dialog, opening, recovering and removing listed documents, and
//! the list kept across runs.

pub mod common;

use std::path::{Path, PathBuf};

use common::drive::{Outcome, Step, drive, editor_over};
use lumenui::App;
use pixl::files::recents::{Recents, Thumb, key};
use pixl::files::save_to;
use pixl::{Editor, Home};
use pixl_document::native;
use pixl_document::{Background, ColorMode, Document, Rgba};

/// A fresh folder under the test's scratch directory.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("home")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A document with a filled square of `color` in it.
fn art(w: u32, h: u32, color: Rgba) -> Document {
    let mut doc = Document::new(w, h, ColorMode::Rgba).unwrap();
    let id = doc.layers()[0].id;
    let raster = &mut doc.target(id, 0).unwrap().raster;
    for y in h as i32 / 4..h as i32 * 3 / 4 {
        for x in w as i32 / 4..w as i32 * 3 / 4 {
            raster.set_raw(x, y, color.to_u32());
        }
    }
    doc
}

fn pixel(doc: &Document, x: i32, y: i32) -> Rgba {
    let id = doc.layers()[0].id;
    doc.cel_raster(id, 0)
        .unwrap()
        .map_or(Rgba::TRANSPARENT, |r| Rgba::from_u32(r.get_raw(x, y)))
}

/// Saves `doc` to `path`.
fn save(doc: Document, path: &Path) {
    save_to(&mut Editor::new(doc), path).unwrap();
}

/// The editor on the start screen over the recent list kept in `data`.
fn start_screen(data: &Path) -> App {
    pixl::starting(
        editor_over(Document::new(64, 64, ColorMode::Rgba).unwrap()),
        Recents::load(data),
        None,
    )
}

/// Three saved documents, one of them with newer unsaved edits in its
/// recovery copy, and one listed document that is gone; listed newest
/// first as `knight`, `tiles`, `gone`, `sky`.
fn seeded(name: &str) -> (PathBuf, PathBuf) {
    let root = scratch(name);
    let (docs, data) = (root.join("art"), root.join("data"));
    std::fs::create_dir_all(&docs).unwrap();
    let mut recents = Recents::load(&data);
    let sky = docs.join("sky.pixl");
    let gone = docs.join("gone.pixl");
    let tiles = docs.join("tiles.pixl");
    let knight = docs.join("knight.pixl");
    for (path, doc) in [
        (&sky, art(160, 144, Rgba::opaque(0x33, 0x88, 0xdd))),
        (&gone, art(16, 16, Rgba::opaque(0x99, 0x99, 0x99))),
        (&tiles, art(256, 256, Rgba::opaque(0x44, 0xaa, 0x33))),
        (&knight, art(32, 48, Rgba::opaque(0xdd, 0x44, 0x22))),
    ] {
        save(doc.clone(), path);
        recents.note(path, &doc, Thumb::Redraw);
    }
    std::fs::remove_file(&gone).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    native::write_recovery(&art(256, 256, Rgba::opaque(0xff, 0xee, 0x00)), &tiles).unwrap();
    recents.finish_thumbnails();
    (docs, data)
}

#[test]
fn with_nothing_opened_yet_the_start_screen_says_so() {
    let data = scratch("empty").join("data");
    let out = drive(start_screen(&data), vec![Step::Wait]);
    common::save(&out.frame, "home-empty");
    assert!(out.home_open);
    assert!(out.recents.is_empty());
    // The start screen's own background, where the editor has its status
    // bar.
    assert_eq!(out.frame.pixel(100, 890), Some([0x12, 0x12, 0x12, 0xff]));
}

#[test]
fn the_list_shows_each_document_with_a_thumbnail_kept_in_the_data_folder() {
    let (docs, data) = seeded("list");
    let out = drive(start_screen(&data), vec![Step::Wait, Step::Wait]);
    common::save(&out.frame, "home-recents");
    let names: Vec<_> = out
        .recents
        .iter()
        .map(|r| r.path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["knight", "tiles", "gone", "sky"]);
    assert_eq!((out.recents[0].width, out.recents[0].height), (32, 48));
    for thumb in out.thumbs.iter().flatten() {
        assert!(
            thumb.starts_with(data.join("thumbs")),
            "{}",
            thumb.display()
        );
    }
    assert_eq!(out.thumbs.iter().flatten().count(), 4);
    // Nothing but the documents and their one recovery copy beside them.
    let mut beside: Vec<_> = std::fs::read_dir(&docs)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    beside.sort();
    assert_eq!(
        beside,
        [
            ".tiles.pixl.recovery",
            "knight.pixl",
            "sky.pixl",
            "tiles.pixl"
        ]
    );
    let shown = pixl::home::recents_markup(&Recents::load(&data), "", std::time::SystemTime::now());
    assert_eq!(shown.matches("Missing").count(), 1);
    assert_eq!(shown.matches("text=\"Recover\"").count(), 1);
}

#[test]
fn the_search_field_filters_by_name_and_folder() {
    let (_, data) = seeded("filter");
    let recents = Recents::load(&data);
    let now = std::time::SystemTime::now();
    let by_name = pixl::home::recents_markup(&recents, "KNI", now);
    assert!(by_name.contains("knight.pixl") && !by_name.contains("sky.pixl"));
    let by_folder = pixl::home::recents_markup(&recents, "filter/art", now);
    let rows = |markup: &str| {
        markup
            .match_indices("<row id=\"recent-")
            .filter(|(i, _)| !markup[*i..].starts_with("<row id=\"recent-re"))
            .count()
    };
    assert_eq!(rows(&by_folder), 4);
    let other_slash = pixl::home::recents_markup(&recents, "filter\\art", now);
    assert_eq!(rows(&other_slash), 4);
    let none = pixl::home::recents_markup(&recents, "zzz", now);
    assert!(none.contains("No recent document matches"));
}

#[test]
fn a_click_on_a_listed_document_opens_it_in_the_editor() {
    let (docs, data) = seeded("open");
    let knight = docs.join("knight.pixl");
    let row: &'static str = Box::leak(format!("recent-{}", key(&knight)).into_boxed_str());
    let out = drive(
        start_screen(&data),
        // A click on the thumbnail is a click on its row.
        vec![Step::Wait, Step::ClickInside(row), Step::Wait, Step::Wait],
    );
    common::save(&out.frame, "home-opened-recent");
    assert!(!out.home_open);
    assert_eq!((out.doc.width(), out.doc.height()), (32, 48));
    assert_eq!(pixel(&out.doc, 16, 24), Rgba::opaque(0xdd, 0x44, 0x22));
    assert_eq!(out.title, "knight.pixl");
}

#[test]
fn recover_opens_the_newer_recovery_copy() {
    let (docs, data) = seeded("recover");
    let tiles = docs.join("tiles.pixl");
    let button: &'static str =
        Box::leak(format!("recent-recover-{}", key(&tiles)).into_boxed_str());
    let out = drive(
        start_screen(&data),
        vec![Step::Wait, Step::Click(button), Step::Wait],
    );
    assert!(!out.home_open);
    assert_eq!(pixel(&out.doc, 128, 128), Rgba::opaque(0xff, 0xee, 0x00));
    assert!(out.modified, "the recovered edits are not saved yet");
    assert_eq!(out.title, "tiles.pixl *");
}

#[test]
fn a_missing_document_shows_as_missing_and_can_be_removed() {
    let (docs, data) = seeded("remove");
    let gone = docs.join("gone.pixl");
    let row: &'static str = Box::leak(format!("recent-{}", key(&gone)).into_boxed_str());
    let remove: &'static str = Box::leak(format!("recent-remove-{}", key(&gone)).into_boxed_str());
    let out = drive(
        start_screen(&data),
        vec![
            Step::Wait,
            Step::Click(row),
            Step::Wait,
            Step::Click(remove),
            Step::Wait,
        ],
    );
    assert!(out.home_open, "a missing document does not open");
    assert!(out.message.unwrap().contains("missing"));
    assert_eq!(out.recents.len(), 3);
    assert!(out.recents.iter().all(|r| r.path != gone));
    // The removal is kept for the next run.
    assert_eq!(Recents::load(&data).entries.len(), 3);
}

#[test]
fn the_new_project_dialog_makes_the_document_it_describes() {
    let data = scratch("new").join("data");
    let shot = drive(
        start_screen(&data),
        vec![
            Step::Click("home-new"),
            Step::Wait,
            Step::Click("preset-160x144"),
            Step::Text("new_background", "Color"),
            Step::Text("new_bg_hex", "#3388dd"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&shot.frame, "home-new-project");
    let out: Outcome = drive(
        start_screen(&data),
        vec![
            Step::Click("home-new"),
            Step::Wait,
            Step::Text("new_name", "hero"),
            Step::Click("preset-32x48"),
            Step::Text("new_mode", "Indexed"),
            Step::Text("new_background", "Color"),
            Step::Text("new_bg_hex", "#ff0000"),
            Step::Wait,
            Step::Click("new-create"),
            Step::Wait,
        ],
    );
    assert!(!out.home_open);
    assert_eq!((out.doc.width(), out.doc.height()), (32, 48));
    assert_eq!(out.doc.mode(), ColorMode::Indexed);
    assert_eq!(
        out.doc.background(),
        Background::Color(Rgba::opaque(0xff, 0, 0))
    );
    assert_eq!(out.title, "hero");
    assert!(
        out.recents.is_empty(),
        "nothing is listed before it is saved"
    );
}

#[test]
fn saving_lists_the_document_and_draws_its_thumbnail() {
    let root = scratch("save");
    let path = root.join("saved.pixl");
    save(art(8, 8, Rgba::opaque(1, 2, 3)), &path);
    let mut editor = Editor::new(art(8, 8, Rgba::opaque(1, 2, 3)));
    editor.path = Some(path.clone());
    let app = pixl::editor(lumenui::prelude::Source::Disk {
        dir: common::app_dir("cpu"),
    })
    .insert_resource(editor)
    .insert_resource(Recents::load(root.join("data")));
    let out = drive(
        app,
        vec![
            Step::Edit(|e| e.touch()),
            Step::Ctrl("s", false),
            Step::Wait,
        ],
    );
    assert!(!out.modified);
    assert_eq!(out.recents.len(), 1);
    assert_eq!(out.recents[0].path, path);
    let thumb = out.thumbs[0].clone().expect("a thumbnail");
    let png = pixl_io::png::decode_png(&std::fs::read(thumb).unwrap()).unwrap();
    assert_eq!(png.size(), (8, 8));
    // A second run reads the same list.
    assert_eq!(Recents::load(root.join("data")).entries, out.recents);
}

#[test]
fn a_file_named_at_start_skips_the_start_screen() {
    let (docs, data) = seeded("argument");
    let sky = docs.join("sky.pixl");
    let app = pixl::starting(
        editor_over(Document::new(64, 64, ColorMode::Rgba).unwrap()),
        Recents::load(&data),
        Some(sky.clone()),
    );
    let out = drive(app, vec![Step::Wait]);
    assert!(!out.home_open);
    assert_eq!((out.doc.width(), out.doc.height()), (160, 144));
    assert_eq!(out.recents[0].path, sky, "opening it lists it first");

    let missing = pixl::starting(
        editor_over(Document::new(64, 64, ColorMode::Rgba).unwrap()),
        Recents::load(&data),
        Some(docs.join("nope.pixl")),
    );
    let out = drive(missing, vec![Step::Wait]);
    assert!(
        out.home_open,
        "a file that does not open leaves the start screen"
    );
    assert!(out.message.is_some());
}

#[test]
fn close_and_home_go_back_to_the_start_screen() {
    let data = scratch("close").join("data");
    let app = pixl::starting(
        editor_over(art(16, 16, Rgba::opaque(9, 9, 9))),
        Recents::load(&data),
        None,
    )
    .insert_resource(Home::default());
    let home = drive(app, vec![Step::Click("go-home"), Step::Wait]);
    common::save(&home.frame, "home-from-editor");
    assert!(home.home_open);
    assert_eq!(
        pixel(&home.doc, 8, 8),
        Rgba::opaque(9, 9, 9),
        "Home keeps it"
    );

    let app = pixl::starting(
        editor_over(art(16, 16, Rgba::opaque(9, 9, 9))),
        Recents::load(&data),
        None,
    )
    .insert_resource(Home::default());
    let back = drive(
        app,
        vec![
            Step::Click("go-home"),
            Step::Wait,
            Step::Click("home-back"),
            Step::Wait,
        ],
    );
    assert!(!back.home_open);
    assert_eq!(pixel(&back.doc, 8, 8), Rgba::opaque(9, 9, 9));

    let app = pixl::starting(
        editor_over(art(16, 16, Rgba::opaque(9, 9, 9))),
        Recents::load(&data),
        None,
    )
    .insert_resource(Home::default());
    let closed = drive(
        app,
        vec![
            Step::Edit(|e| e.touch()),
            Step::Menu("file-close"),
            Step::Wait,
            Step::Click("discard-yes"),
            Step::Wait,
        ],
    );
    assert!(closed.home_open);
    assert_eq!(pixel(&closed.doc, 8, 8), Rgba::TRANSPARENT);
}

#[test]
fn every_preset_has_its_button() {
    let markup =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.lmn"))
            .unwrap();
    for (w, h) in pixl::files::PRESETS {
        let id = pixl::files::preset_id(*w, *h);
        assert!(markup.contains(&format!("id=\"{id}\"")), "{id}");
    }
    assert_eq!(
        markup.matches("id=\"preset-").count(),
        pixl::files::PRESETS.len()
    );
}

#[test]
fn the_rig_demo_and_getting_started_are_listed_after_the_recent_documents() {
    let (_, data) = seeded("demo-listed");
    let recents = Recents::load(&data);
    let now = std::time::SystemTime::now();
    let all = pixl::home::recents_markup(&recents, "", now);
    let demo = all
        .find("<row id=\"home-rig-demo\"")
        .expect("the demo is listed");
    assert_eq!(all.matches("<row id=\"home-rig-demo\"").count(), 1);
    let last_recent = all.rfind("<row id=\"recent-").unwrap();
    assert!(demo > last_recent, "the demo comes after the recents");
    assert!(all[demo..].contains("Rig demo") && all[demo..].contains("48 x 48 px"));
    // The search finds it by name like any listed document, and leaves it
    // out when it searches for another.
    assert!(pixl::home::recents_markup(&recents, "RIG", now).contains("home-rig-demo"));
    assert!(!pixl::home::recents_markup(&recents, "knight", now).contains("home-rig-demo"));
    let none = pixl::home::recents_markup(&recents, "zzz", now);
    assert!(none.contains("No recent document matches") && !none.contains("home-rig-demo"));
    // The Getting started tutorial follows it, and the search finds it as
    // a tutorial.
    let start = all
        .find("<row id=\"home-start-lesson\"")
        .expect("the tutorial is listed");
    assert!(start > demo && all[start..].contains("32 x 32 px"));
    let found = pixl::home::recents_markup(&recents, "tutorial", now);
    assert!(found.contains("home-start-lesson") && !found.contains("home-rig-demo"));
    // With nothing opened yet they are the only entries, under the line
    // saying so.
    let empty = pixl::home::recents_markup(&Recents::in_memory(), "", now);
    assert!(empty.contains("No recent documents yet"));
    assert!(empty.contains("<row id=\"home-rig-demo\""));
    assert_eq!(empty.matches("<row ").count(), 2);
}

#[test]
fn the_rig_demo_thumbnail_is_drawn_once_into_the_data_folder() {
    let data = scratch("demo-thumb").join("data");
    let stale = data.join("thumbs").join("rig-demo-0000000000000000.png");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, b"old").unwrap();
    let mut recents = Recents::load(&data);
    assert_eq!(
        recents.shipped_thumb(pixl::files::shipped::Shipped::RigDemo),
        None
    );
    recents.draw_shipped_thumb(pixl::files::shipped::Shipped::RigDemo);
    recents.finish_thumbnails();
    let thumb = recents
        .shipped_thumb(pixl::files::shipped::Shipped::RigDemo)
        .expect("drawn");
    assert!(thumb.starts_with(data.join("thumbs")));
    assert!(!stale.exists(), "an older picture of the demo is dropped");
    assert!(
        recents.entries.is_empty(),
        "the demo is not a recent document"
    );
    let markup = pixl::home::recents_markup(&recents, "", std::time::SystemTime::now());
    assert!(markup.contains(&*thumb.to_string_lossy()));
}

#[test]
fn a_click_on_the_listed_demo_opens_an_untitled_copy() {
    let data = scratch("demo-open").join("data");
    let out = drive(
        start_screen(&data),
        vec![
            Step::Wait,
            Step::Wait,
            Step::ClickInside("home-rig-demo"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert!(!out.home_open);
    assert_eq!(out.title, "rig demo");
    assert_eq!(out.doc.frame_count(), 8);
    assert!(out.recents.is_empty(), "an untitled copy is not listed");
    assert!(Recents::load(&data).entries.is_empty());
}
