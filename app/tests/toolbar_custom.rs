//! The tool bar's arrangement: the compact arrangement groups tools, the
//! tool in use comes up on its group's button, a right-click opens the
//! group's flyout, and a hidden tool stays hidden in a new window.

pub mod common;

use std::sync::{Arc, Mutex};

use common::drive::{Step, drive, editor_over};
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{ContextMenuEvent, PointerButton};
use lumenui::prelude::*;
use pixl::Tool;
use pixl::store::Store;
use pixl::toolbar::Toolbar;
use pixl_core::{ColorMode, Document};

#[derive(Clone, Debug, Default)]
struct Bar {
    off: Vec<String>,
    grouped: Vec<String>,
    one_column: bool,
    flyout: Vec<String>,
}

type Seen = Arc<Mutex<Bar>>;

fn watch(app: App) -> (App, Seen) {
    let seen: Seen = Arc::default();
    let out = Arc::clone(&seen);
    let app = app.add_systems(TickStage::Systems, move || {
        let mut bar = Bar::default();
        if let Some(node) = dom::get_by_id("toolbar") {
            bar.one_column = node.has_class("one-column");
            for b in node.children() {
                let Some(id) = b.id() else { continue };
                if b.has_class("tool-off") {
                    bar.off.push(id.clone());
                }
                if b.has_class("grouped") {
                    bar.grouped.push(id);
                }
            }
        }
        if let Some(f) = dom::get_by_id("tool-flyout")
            && f.has_class("open")
        {
            bar.flyout = f.children().into_iter().filter_map(|c| c.id()).collect();
        }
        *out.lock().unwrap() = bar;
    });
    (app, seen)
}

fn editor() -> App {
    editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap())
}

#[test]
fn the_compact_bar_groups_tools_and_the_tool_used_comes_up() {
    let (app, seen) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Menu("window-toolbar"),
            Step::Wait,
            Step::Wait,
            Step::Click("tb-compact"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "toolbar-compact");
    let b = seen.lock().unwrap().clone();
    assert!(b.one_column);
    assert!(b.grouped.contains(&"tool-rect".to_string()));
    assert!(b.off.contains(&"tool-ellipse".to_string()));
    assert!(!b.off.contains(&"tool-rect".to_string()));
    assert!(!b.off.contains(&"tool-pencil".to_string()));

    let (app, seen) = watch(editor());
    drive(
        app,
        vec![
            Step::Menu("window-toolbar"),
            Step::Wait,
            Step::Wait,
            Step::Click("tb-compact"),
            Step::Click("tb-close"),
            Step::Wait,
            Step::Key("u", true),
            Step::Wait,
            Step::Wait,
        ],
    );
    let b = seen.lock().unwrap().clone();
    assert!(b.off.contains(&"tool-rect".to_string()));
    assert!(!b.off.contains(&"tool-ellipse".to_string()));
}

/// Right-clicks the button `tool` on the tick after the steps start.
fn right_click(app: App, tool: &'static str) -> App {
    app.add_systems(
        TickStage::Systems,
        (move |mut tick: Local<usize>,
               mut out: MessageWriter<ContextMenuEvent>,
               ids: Query<(Entity, &LumenId)>| {
            *tick += 1;
            if *tick != 10 {
                return;
            }
            let entity = ids
                .iter()
                .find(|(_, id)| id.0 == tool)
                .map(|(e, _)| e)
                .expect("the button is there");
            out.write(ContextMenuEvent {
                entity,
                position: Vec2::new(40.0, 200.0),
                button: PointerButton::Secondary,
                local: None,
            });
        })
        .before(pixl::toolbar::toolbar_input),
    )
}

#[test]
fn a_right_click_opens_the_flyout_and_a_pick_uses_the_tool() {
    let (app, seen) = watch(right_click(editor(), "tool-rect"));
    let out = drive(
        app,
        vec![
            Step::Menu("window-toolbar"),
            Step::Wait,
            Step::Click("tb-compact"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "toolbar-flyout");
    let b = seen.lock().unwrap().clone();
    assert_eq!(
        b.flyout.len(),
        4,
        "rectangle, ellipse, polygon and contour: {:?}",
        b.flyout
    );

    let (app, seen) = watch(right_click(editor(), "tool-rect"));
    let out = drive(
        app,
        vec![
            Step::Menu("window-toolbar"),
            Step::Wait,
            Step::Click("tb-compact"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::ClickInside("tool-flyout"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert_eq!(out.tool, Tool::Rectangle);
    assert!(seen.lock().unwrap().flyout.is_empty(), "a pick closes it");
}

#[test]
fn a_hidden_tool_stays_hidden_in_a_new_window() {
    let dir = std::env::temp_dir().join(format!("pixl-toolbar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (app, _) = watch(editor().insert_resource(Toolbar::load(Store::at(&dir))));
    drive(
        app,
        vec![
            Step::Menu("window-toolbar"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Click("tb-show-tool-spray"),
            Step::Wait,
        ],
    );
    let (app, seen) = watch(editor().insert_resource(Toolbar::load(Store::at(&dir))));
    drive(app, vec![Step::Wait, Step::Wait]);
    assert_eq!(seen.lock().unwrap().off, ["tool-spray"]);
    let _ = std::fs::remove_dir_all(dir);
}
