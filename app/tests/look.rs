//! Renders the window in each look and checks the theme reached it.

use std::sync::Mutex;

use lumenui::prelude::*;
use pixl::Look;

/// Lumen keeps one app's DOM per process, so the renders take turns.
static ONE_APP_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Renders the editor with `look` put on the window before the first frame.
fn shot(look: Look, name: &str) -> Frame {
    let _turn = ONE_APP_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let frame = pixl::app()
        .add_systems(TickStage::Systems, move |mut shown: ResMut<Look>| {
            *shown = look;
        })
        .screenshot(HeadlessOptions {
            dpr: 1.0,
            ticks: Some(6),
        })
        .expect("the editor renders");
    if let Some(dir) = std::env::var_os("PIXL_SHOTS") {
        frame
            .save_png(std::path::Path::new(&dir).join(format!("{name}.png")))
            .expect("write the screenshot");
    }
    frame
}

/// The status bar's background, a token of each set.
fn status_bar(frame: &Frame) -> [u8; 4] {
    frame
        .pixel(700, frame.height() - 4)
        .expect("inside the frame")
}

#[test]
fn the_dark_set_is_the_default() {
    let frame = shot(Look::default(), "look-dark");
    assert_eq!(status_bar(&frame), [0x1a, 0x1a, 0x1a, 0xff]);
}

#[test]
fn the_light_set_comes_from_the_same_tokens() {
    let frame = shot(
        Look {
            light: true,
            plain_font: false,
        },
        "look-light",
    );
    assert_eq!(status_bar(&frame), [0xe2, 0xe2, 0xe2, 0xff]);
}

#[test]
fn the_plain_font_setting_renders() {
    shot(
        Look {
            light: false,
            plain_font: true,
        },
        "look-plain-font",
    );
}
