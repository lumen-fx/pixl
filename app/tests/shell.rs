//! Renders the editor with no window and checks the shape of the window.

use lumenui::HeadlessOptions;

#[test]
fn the_window_has_its_regions() {
    let frame = pixl::app()
        .screenshot(HeadlessOptions {
            dpr: 1.0,
            ticks: Some(4),
        })
        .expect("the editor renders");
    assert_eq!((frame.width(), frame.height()), (1440, 900));
    if let Some(dir) = std::env::var_os("PIXL_SHOTS") {
        frame
            .save_png(std::path::Path::new(&dir).join("shell.png"))
            .expect("write the screenshot");
    }
}
