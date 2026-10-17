//! The markup as a release build embeds it: the entry file and stylesheet
//! compiled in, and the parts they include read beside them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lumenui::prelude::*;

#[test]
fn embedded_markup_finds_its_included_parts() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.lmn");
    let source = Source::embedded_from(
        dir,
        include_str!("../src/main.lmn"),
        Some(include_str!("../src/main.css")),
    );
    let found = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&found);
    pixl::editor(source)
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
        })
        .expect("the editor renders");
    assert!(found.load(Ordering::Relaxed), "the timeline parts are in");
}
