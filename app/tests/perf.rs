//! The performance overlay, rendered with no window: F3 shows it, a stroke
//! is timed to the screen, and the frames are counted.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl_document::{ColorMode, Document};

#[test]
fn f3_shows_the_overlay_and_a_stroke_is_timed() {
    let doc = Document::new(64, 64, ColorMode::Rgba).unwrap();
    let mut steps = vec![Step::Key("F3", false), Step::Wait];
    steps.push(Step::Down(At::Doc(4, 4), PointerButton::Primary));
    for x in 5..30 {
        steps.push(Step::Move(At::Doc(x, 4 + x / 3)));
        // About 60 frames a second, with a slow one now and then.
        steps.push(Step::Sleep(if x % 10 == 0 { 40 } else { 16 }));
    }
    steps.push(Step::Up(At::Doc(30, 14), PointerButton::Primary));
    steps.extend([Step::Sleep(300), Step::Wait, Step::Wait]);
    let out = drive(editor_over(doc), steps);
    common::save(&out.frame, "perf-overlay");
    assert!(out.settings.performance.show_fps);
    assert!(out.perf_frames > 20, "{} frames", out.perf_frames);
    assert!(out.last_stroke.is_some());
    assert_eq!(out.perf_lines.len(), 6, "{:?}", out.perf_lines);
    assert!(
        out.perf_lines[2].starts_with("stroke to screen"),
        "{:?}",
        out.perf_lines
    );
}

#[test]
fn f3_again_hides_it() {
    let doc = Document::new(64, 64, ColorMode::Rgba).unwrap();
    let out = drive(
        editor_over(doc),
        vec![
            Step::Key("F3", false),
            Step::Wait,
            Step::Key("F3", false),
            Step::Wait,
        ],
    );
    assert!(!out.settings.performance.show_fps);
}

#[test]
fn the_perf_log_gets_a_row_per_frame() {
    let path = std::env::temp_dir().join(format!("pixl-perf-{}.csv", std::process::id()));
    let log = pixl::perf::PerfLog::create(&path).unwrap();
    let doc = Document::new(64, 64, ColorMode::Rgba).unwrap();
    let mut steps = vec![Step::Down(At::Doc(4, 4), PointerButton::Primary)];
    for x in 5..15 {
        steps.push(Step::Move(At::Doc(x, 4)));
    }
    steps.push(Step::Up(At::Doc(15, 4), PointerButton::Primary));
    let out = drive(
        editor_over(doc).insert_resource(pixl::perf::Perf::logging(log)),
        steps,
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(pixl::perf::LOG_HEADER));
    let rows: Vec<&str> = lines.collect();
    // The harness looks before the last tick closes its frame.
    let n = rows.len() as u64;
    assert!(n == out.perf_frames || n == out.perf_frames + 1, "{n}");
    let columns = pixl::perf::LOG_HEADER.split(',').count();
    assert!(
        rows.iter().all(|r| r.split(',').count() == columns),
        "{rows:?}"
    );
    // The stroke wrote tiles into the layer's buffer.
    assert!(rows.iter().any(|r| r.split(',').nth(5) != Some("0")));
}
