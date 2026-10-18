//! The tool bar's icons: masks inked by the theme, pixel for pixel, and
//! switched to the accent ink on the active tool.

pub mod common;

use std::collections::BTreeSet;
use std::path::Path;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl::Tool;
use pixl_core::io::decode_png;
use pixl_core::{ColorMode, Document};

/// The icons decode on worker threads from the first tick, and nothing an
/// app can see says when they are done (lumen-fx/lumen#541); this pause is
/// many times what decoding them takes, and the ticks after it bring them
/// to the frame.
const DECODED: u64 = 250;

/// The positions of `ink` in the tool bar column, relative to the first.
fn ink_in_toolbar(frame: &lumenui::Frame, ink: [u8; 4]) -> BTreeSet<(u32, u32)> {
    let mut found = BTreeSet::new();
    for y in 60..700 {
        for x in 0..71 {
            if frame.pixel(x, y) == Some(ink) {
                found.insert((x, y));
            }
        }
    }
    normalize(found)
}

fn normalize(points: BTreeSet<(u32, u32)>) -> BTreeSet<(u32, u32)> {
    let x0 = points.iter().map(|p| p.0).min().unwrap_or(0);
    let y0 = points.iter().map(|p| p.1).min().unwrap_or(0);
    points.iter().map(|&(x, y)| (x - x0, y - y0)).collect()
}

/// The fully inked pixels of an icon's 12x12 map, each drawn as a `scale`
/// square. The mask files hold the map at four times its size.
fn icon_ink(name: &str, scale: u32) -> BTreeSet<(u32, u32)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/icons")
        .join(format!("{name}.png"));
    let png = decode_png(&std::fs::read(path).unwrap()).unwrap();
    let (w, _) = png.size();
    let mut out = BTreeSet::new();
    for (i, px) in png.to_rgba().iter().enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        if px.a == 255 && x % 4 == 0 && y % 4 == 0 {
            let (x, y) = (x / 4, y / 4);
            for dy in 0..scale {
                for dx in 0..scale {
                    out.insert((x * scale + dx, y * scale + dy));
                }
            }
        }
    }
    normalize(out)
}

#[test]
fn the_active_tool_shows_its_icon_in_the_accent_ink_at_twice_its_size() {
    let out = drive(
        editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap()),
        vec![
            Step::Key("e", false),
            Step::Sleep(DECODED),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "toolbar");
    assert_eq!(out.tool, Tool::Eraser);
    // The dark set's --on-accent ink appears only in the active button, as
    // the eraser icon scaled 2x without smoothing.
    let active = ink_in_toolbar(&out.frame, [0x08, 0x11, 0x1c, 0xff]);
    assert_eq!(active, icon_ink("eraser", 2));
}

#[test]
fn the_resting_icons_are_drawn_in_the_text_ink() {
    let out = drive(
        editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap()),
        vec![Step::Sleep(DECODED), Step::Wait, Step::Wait],
    );
    // --text at the resting opacity over the tool bar's --surface-2.
    let resting = ink_in_toolbar(&out.frame, [0x90, 0x90, 0x90, 0xff]);
    let slice_icon = icon_ink("slice", 2);
    // The slice tool is the last button, alone in its row: its icon is the
    // bottom of the set.
    let bottom = resting.iter().map(|p| p.1).max().unwrap();
    let top = bottom - slice_icon.iter().map(|p| p.1).max().unwrap();
    let tail: BTreeSet<(u32, u32)> =
        normalize(resting.iter().copied().filter(|p| p.1 >= top).collect());
    assert_eq!(tail, slice_icon);
}

/// The pointer comes down on the middle of the button, over its icon:
/// Lumen names the icon as the clicked element, and the button's tool is
/// picked all the same.
#[test]
fn a_press_on_a_tool_s_icon_picks_the_tool() {
    let out = drive(
        editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap()),
        vec![
            Step::Sleep(DECODED),
            Step::Wait,
            Step::Move(At::Element("tool-eraser")),
            Step::Down(At::Element("tool-eraser"), PointerButton::Primary),
            Step::Up(At::Element("tool-eraser"), PointerButton::Primary),
            Step::Wait,
        ],
    );
    assert_eq!(out.tool, Tool::Eraser);
}
