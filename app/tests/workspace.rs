//! The workspace: panels moved between docks, collapsed, hidden and shown
//! again; the built-in layouts; a layout saved, switched to and kept.

pub mod common;

use std::sync::{Arc, Mutex};

use common::drive::{Step, drive, editor_over};
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{ContextMenuEvent, PointerButton};
use lumenui::prelude::*;
use pixl::Workspace;
use pixl::store::Store;
use pixl::workspace::Dock;
use pixl_core::{ColorMode, Document};

/// What the window showed of the docks at the end.
#[derive(Clone, Debug, Default)]
struct Docks {
    /// Each dock's panels in order, and whether the dock shows.
    docks: Vec<(Dock, Vec<String>, bool)>,
    collapsed: Vec<String>,
    closed: Vec<String>,
    /// The left and right docks' widths.
    widths: (f32, f32),
}

impl Docks {
    fn panels(&self, dock: Dock) -> Vec<&str> {
        self.docks
            .iter()
            .find(|(d, _, _)| *d == dock)
            .map(|(_, p, _)| p.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    fn shows(&self, dock: Dock) -> bool {
        self.docks.iter().any(|(d, _, s)| *d == dock && *s)
    }
}

/// Records the docks every tick; the last record is the end state.
fn watch(app: App) -> (App, Arc<Mutex<Docks>>) {
    let seen: Arc<Mutex<Docks>> = Arc::default();
    let out = Arc::clone(&seen);
    let app = app.add_systems(TickStage::Systems, move || {
        let mut d = Docks::default();
        for dock in Dock::ALL {
            let Some(node) = dom::get_by_id(dock.element_id()) else {
                continue;
            };
            let mut panels = Vec::new();
            for child in node.children() {
                let Some(id) = child.id() else { continue };
                if child.has_class("collapsed") {
                    d.collapsed.push(id.clone());
                }
                if child.has_class("closed") {
                    d.closed.push(id.clone());
                }
                panels.push(id);
            }
            let shows = node.rect().is_some_and(|r| r.width > 0.0 && r.height > 0.0);
            d.docks.push((dock, panels, shows));
        }
        let width = |id: &str| {
            dom::get_by_id(id)
                .and_then(|n| n.rect())
                .map_or(0.0, |r| r.width)
        };
        d.widths = (width("dock-left"), width("dock-right"));
        *out.lock().unwrap() = d;
    });
    (app, seen)
}

fn editor() -> App {
    editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap())
}

#[test]
fn paint_is_the_default_layout_with_the_panels_right_and_the_timeline_below() {
    let (app, docks) = watch(editor());
    let out = drive(app, vec![Step::Wait, Step::Wait]);
    common::save(&out.frame, "workspace-paint");
    let d = docks.lock().unwrap().clone();
    assert_eq!(
        d.panels(Dock::Right),
        [
            "panel-color",
            "panel-palette",
            "panel-layers",
            "panel-rig",
            "panel-history"
        ]
    );
    assert!(d.shows(Dock::Right));
    assert_eq!(d.panels(Dock::Bottom), ["panel-timeline", "panel-preview"]);
    assert!(d.shows(Dock::Bottom));
    assert_eq!(d.closed, ["panel-preview"], "Paint leaves the preview out");
    assert!(!d.shows(Dock::Left));
}

#[test]
fn the_rig_layout_puts_the_colors_on_the_left() {
    let (app, docks) = watch(editor());
    let out = drive(
        app,
        vec![Step::Menu("window-layout-rig"), Step::Wait, Step::Wait],
    );
    common::save(&out.frame, "workspace-rig");
    let d = docks.lock().unwrap().clone();
    assert_eq!(d.panels(Dock::Left), ["panel-color", "panel-palette"]);
    assert_eq!(
        d.panels(Dock::Right),
        ["panel-layers", "panel-rig", "panel-history"]
    );
    assert!(d.shows(Dock::Left));
    assert!(d.widths.0 > 100.0, "the left dock has room: {:?}", d.widths);
}

#[test]
fn header_buttons_collapse_and_hide_and_the_window_menu_shows_all() {
    let (app, docks) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Wait,
            Step::Click("panel-color-collapse"),
            Step::Click("panel-history-close"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "workspace-collapsed");
    let d = docks.lock().unwrap().clone();
    // The Rig panel starts folded outside the Rig layout.
    assert_eq!(d.collapsed, ["panel-color", "panel-rig"]);
    assert_eq!(d.closed, ["panel-history", "panel-preview"]);

    let (app, docks) = watch(editor());
    drive(
        app,
        vec![
            Step::Wait,
            Step::Click("panel-history-close"),
            Step::Wait,
            Step::Menu("window-show-all"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert!(docks.lock().unwrap().closed.is_empty());
}

/// Asks for the panel menu on `panel`'s header on the tick the steps start.
fn open_panel_menu(app: App, panel: &'static str) -> App {
    app.add_systems(
        TickStage::Systems,
        (move |mut tick: Local<usize>,
               mut out: MessageWriter<ContextMenuEvent>,
               ids: Query<(Entity, &LumenId)>| {
            *tick += 1;
            if *tick != 6 {
                return;
            }
            let entity = ids
                .iter()
                .find(|(_, id)| id.0 == panel)
                .map(|(e, _)| e)
                .expect("the panel is there");
            out.write(ContextMenuEvent {
                entity,
                position: Vec2::new(1200.0, 100.0),
                button: PointerButton::Secondary,
                local: None,
            });
        })
        .before(pixl::workspace::workspace_input),
    )
}

#[test]
fn the_panel_menu_moves_a_panel_to_the_bottom_dock_and_back_up() {
    let (app, docks) = watch(open_panel_menu(editor(), "panel-layers"));
    let out = drive(
        app,
        vec![
            Step::Wait,
            Step::Wait,
            Step::Menu("panel-menu-bottom"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "workspace-bottom");
    let d = docks.lock().unwrap().clone();
    assert_eq!(
        d.panels(Dock::Bottom),
        ["panel-timeline", "panel-preview", "panel-layers"]
    );
    assert!(d.shows(Dock::Bottom));

    let (app, docks) = watch(open_panel_menu(editor(), "panel-history"));
    drive(
        app,
        vec![
            Step::Wait,
            Step::Wait,
            Step::Menu("panel-menu-up"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert_eq!(
        docks.lock().unwrap().panels(Dock::Right),
        [
            "panel-color",
            "panel-palette",
            "panel-layers",
            "panel-history",
            "panel-rig"
        ]
    );
}

#[test]
fn the_dialog_lists_panels_and_saves_a_layout_that_outlives_the_window() {
    let dir = std::env::temp_dir().join(format!("pixl-ws-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = editor().insert_resource(Workspace::load(Store::at(&dir)));
    let (app, docks) = watch(app);
    let out = drive(
        app,
        vec![
            Step::Menu("window-workspace"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Click("ws-dock-left-panel-palette"),
            Step::Wait,
            Step::Text("ws_name", "Mine"),
            Step::Click("ws-save"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "workspace-dialog");
    assert_eq!(docks.lock().unwrap().panels(Dock::Left), ["panel-palette"]);
    let again = Workspace::load(Store::at(&dir));
    assert_eq!(again.live.name, "Mine");
    let mine = again.layout("Mine").expect("the layout was saved");
    assert_eq!(mine.place("panel-palette").unwrap().dock, Dock::Left);

    // A new window opens on the saved arrangement.
    let app = editor().insert_resource(Workspace::load(Store::at(&dir)));
    let (app, docks) = watch(app);
    drive(app, vec![Step::Wait, Step::Wait]);
    assert_eq!(docks.lock().unwrap().panels(Dock::Left), ["panel-palette"]);
    let _ = std::fs::remove_dir_all(dir);
}
