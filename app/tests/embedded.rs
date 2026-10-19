//! The UI as the build compiles it into the executable: every part the entry
//! includes is in it, so the editor started over an app directory with no
//! `src/` (what a shipped `pixl.exe` has beside it) shows them all.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lumenui::prelude::*;

#[test]
fn the_compiled_ui_carries_its_included_parts() {
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("app-no-src");
    std::fs::create_dir_all(&dir).expect("make the app directory");
    let toml = std::fs::read_to_string(app.join("lumen.toml")).expect("read lumen.toml");
    std::fs::write(
        dir.join("lumen.toml"),
        format!("{toml}\n[render]\nbackend = \"cpu\"\n\n[mcp]\nport = 0\n"),
    )
    .expect("write lumen.toml");
    assert!(!dir.join("src").exists());

    let found = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&found);
    pixl::editor(pixl::ui())
        .app_dir(&dir)
        .add_systems(TickStage::Systems, move || {
            // One part in the window's layout, one beside the root.
            if dom::get_by_id("tl-grid").is_some() && dom::get_by_id("probe-onion-before").is_some()
            {
                seen.store(true, Ordering::Relaxed);
            }
        })
        .screenshot(HeadlessOptions {
            dpr: 1.0,
            ticks: Some(8),
            ..Default::default()
        })
        .expect("the editor renders");
    assert!(found.load(Ordering::Relaxed), "the timeline parts are in");
}
