//! The workspace: the panels in the dock around the canvas, the built-in
//! layouts, the Panels and layouts dialog, dragging a tab to another place,
//! and the arrangement an earlier pixl kept in `workspace.json` carried
//! over.

pub mod common;

use std::sync::{Arc, Mutex};

use common::drive::{At, Step, drive, editor_over};
use lumenui::glam::Vec2;
use lumenui::lumen_core::components::TextContent;
use lumenui::lumen_core::input::PointerButton;
use lumenui::lumen_core::window::{PrimaryWindow, Window};
use lumenui::lumen_primitives::TabStripButton;
use lumenui::prelude::*;
use lumenui_dock::{Dock, DockCloseButton, DockTab, DockTabCaption};
use pixl::Workspace;
use pixl::settings::store::Store;
use pixl::workspace::{CANVAS, Edge, edge_of, folded};
use pixl_document::{ColorMode, Document};

/// The panels pixl has besides the canvas.
const PANELS: [&str; 9] = [
    "panel-color",
    "panel-palette",
    "panel-layers",
    "panel-rig",
    "panel-history",
    "panel-brushes",
    "panel-tileset",
    "panel-timeline",
    "panel-preview",
];

/// What the window showed at the end.
#[derive(Clone, Debug, Default)]
struct Seen {
    /// The named layout in use.
    layout: String,
    /// The panels on each side of the canvas, in order.
    left: Vec<String>,
    right: Vec<String>,
    bottom: Vec<String>,
    hidden: Vec<String>,
    folded: Vec<String>,
    /// The panels in windows of their own.
    windows: Vec<String>,
    /// Each shown panel's box: left, top, width, height.
    boxes: Vec<(String, [f32; 4])>,
    toolbar: f32,
    /// Each tab's caption and the panel it shows, and each close button's
    /// panel and the panel of the tab it is on.
    captions: Vec<(String, String)>,
    closes: Vec<(String, String)>,
    /// The panels sharing the canvas's stack.
    with_canvas: Vec<String>,
}

impl Seen {
    fn size(&self, panel: &str) -> [f32; 4] {
        self.boxes
            .iter()
            .find(|(p, _)| p == panel)
            .map_or([0.0; 4], |(_, b)| *b)
    }
}

fn title(panel: &str) -> String {
    dom::get_by_id(panel)
        .and_then(|n| n.get_attr("title"))
        .unwrap_or_default()
}

/// Records the workspace every tick; the last record is the end state.
fn watch(app: App) -> (App, Arc<Mutex<Seen>>) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let out = Arc::clone(&seen);
    let app = app.add_systems(
        TickStage::Systems,
        move |dock: Dock,
              ws: Res<Workspace>,
              buttons: Query<(&TabStripButton, &Children)>,
              tabs: Query<&DockTab>,
              captions: Query<&TextContent, With<DockTabCaption>>,
              closes: Query<&DockCloseButton>| {
            let layout = dock.save();
            let mut s = Seen {
                layout: ws.layout.clone(),
                ..Seen::default()
            };
            for p in PANELS {
                if !dock.is_shown(p) {
                    s.hidden.push(p.to_string());
                    continue;
                }
                if dock.window_of(p) > 0 {
                    s.windows.push(p.to_string());
                }
                if folded(&layout, p) {
                    s.folded.push(p.to_string());
                }
                if let Some(r) = dom::get_by_id(p).and_then(|n| n.rect()) {
                    s.boxes
                        .push((p.to_string(), [r.client_x, r.client_y, r.width, r.height]));
                }
            }
            if let Some(area) = layout.docks.get(pixl::workspace::DOCK) {
                for p in area.root.panels() {
                    match edge_of(&layout, p) {
                        Some(Edge::Left) => s.left.push(p.clone()),
                        Some(Edge::Right) => s.right.push(p.clone()),
                        Some(Edge::Bottom) => s.bottom.push(p.clone()),
                        None => {}
                    }
                }
            }
            if let Some(loc) = layout.find(CANVAS)
                && let Some(stack) = layout.stack(&loc.stack)
            {
                s.with_canvas = stack.panels.clone();
            }
            s.toolbar = dom::get_by_id("toolbar")
                .and_then(|n| n.rect())
                .map_or(0.0, |r| r.width);
            for (b, children) in &buttons {
                let Ok(tab) = tabs.get(b.tab) else { continue };
                for &c in children {
                    if let Ok(text) = captions.get(c) {
                        s.captions.push((text.0.clone(), title(&tab.panel)));
                    }
                    if let Ok(close) = closes.get(c) {
                        s.closes.push((close.panel.clone(), tab.panel.clone()));
                    }
                }
            }
            *out.lock().unwrap() = s;
        },
    );
    (app, seen)
}

fn editor() -> App {
    editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap())
}

/// The editor in a smaller window than `lumen.toml` asks for, as a laptop
/// or a tiling window manager gives it.
fn small_editor() -> App {
    editor().add_systems(
        TickStage::Systems,
        |mut windows: Query<&mut Window, With<PrimaryWindow>>| {
            for mut w in &mut windows {
                if w.size != Vec2::new(1200.0, 720.0) {
                    w.size = Vec2::new(1200.0, 720.0);
                }
            }
        },
    )
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// A layout from the Window menu, and the ticks it takes to show.
fn layout(id: &'static str) -> [Step; 3] {
    [Step::Menu(id), Step::Wait, Step::Wait]
}

#[test]
fn paint_is_the_first_layout_with_the_panels_right_and_the_timeline_below() {
    let (app, seen) = watch(editor());
    let out = drive(app, vec![Step::Wait, Step::Wait, Step::Wait]);
    common::save(&out.frame, "workspace-paint");
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.layout, "Paint");
    assert_eq!(
        strs(&s.right),
        [
            "panel-color",
            "panel-palette",
            "panel-rig",
            "panel-history",
            "panel-brushes"
        ]
    );
    assert_eq!(strs(&s.bottom), ["panel-timeline"]);
    assert!(s.left.is_empty());
    assert_eq!(
        strs(&s.hidden),
        ["panel-layers", "panel-tileset", "panel-preview"]
    );
    // The Rig panel shows its tab alone outside the Rig layout.
    assert_eq!(strs(&s.folded), ["panel-rig"]);
}

#[test]
fn the_rig_layout_puts_the_colors_on_the_left() {
    let (app, seen) = watch(editor());
    let out = drive(app, layout("window-layout-rig").to_vec());
    common::save(&out.frame, "workspace-rig");
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.layout, "Rig");
    assert_eq!(strs(&s.left), ["panel-color", "panel-palette"]);
    assert_eq!(strs(&s.right), ["panel-rig", "panel-history"]);
    assert!(s.folded.is_empty(), "the rig is open: {:?}", s.folded);
    assert!(s.size("panel-color")[2] > 200.0, "the left side has room");
}

#[test]
fn every_tab_names_its_own_panel_after_the_layouts_switch() {
    let (app, seen) = watch(editor());
    let mut steps = Vec::new();
    for id in [
        "window-layout-rig",
        "window-layout-paint",
        "window-layout-animate",
        "window-layout-tiles",
    ] {
        steps.extend(layout(id));
    }
    let out = drive(app, steps);
    common::save(&out.frame, "workspace-tiles");
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.layout, "Tiles");
    assert!(s.captions.len() >= 6, "{:?}", s.captions);
    for (caption, panel) in &s.captions {
        assert_eq!(caption, panel, "a tab's caption names its own panel");
    }
    for (closes, panel) in &s.closes {
        assert_eq!(closes, panel, "a tab's close button closes its own panel");
        assert_ne!(panel, CANVAS, "the canvas has no close button");
    }
}

#[test]
fn a_panel_dropped_on_the_canvas_shares_its_place_and_the_canvas_keeps_no_close_button() {
    // The History tab, dragged by its middle onto the middle of the canvas.
    let drop = |then: &[Step]| {
        let (app, seen) = watch(editor());
        let tab = At::Offset("panel-history", 30.0, -11.0);
        let to = At::Within("panel-canvas", 0.5, 0.5);
        let mut steps = vec![
            Step::Wait,
            Step::Wait,
            Step::Move(tab),
            Step::Wait,
            Step::Down(tab, PointerButton::Primary),
            Step::Move(At::Within("panel-canvas", 0.3, 0.3)),
            Step::Move(to),
            Step::Move(to),
            Step::Up(to, PointerButton::Primary),
            Step::Wait,
            Step::Wait,
        ];
        steps.extend_from_slice(then);
        let out = drive(app, steps);
        (out, seen.lock().unwrap().clone())
    };
    let (out, s) = drop(&[]);
    common::save(&out.frame, "workspace-on-canvas");
    assert_eq!(strs(&s.with_canvas), [CANVAS, "panel-history"]);
    assert_eq!(pixl_count(&out.doc), 0, "the drag painted nothing");
    for (closes, panel) in &s.closes {
        assert_eq!(closes, panel);
        assert_ne!(panel, CANVAS, "the canvas has no close button");
    }
    // A layout switch hands the buttons round again.
    let mut switch = layout("window-layout-rig").to_vec();
    switch.extend(layout("window-layout-paint"));
    let (_, s) = drop(&switch);
    assert!(!s.closes.is_empty());
    for (closes, panel) in &s.closes {
        assert_eq!(closes, panel);
        assert_ne!(panel, CANVAS, "the canvas has no close button");
    }
}

/// How many pixels of the document's first layer are painted.
fn pixl_count(doc: &Document) -> usize {
    let layer = doc.layers()[0].id;
    doc.cel_raster(layer, 0).unwrap().map_or(0, |r| {
        let b = r.content_bounds();
        (b.w * b.h).max(0) as usize
    })
}

#[test]
fn the_docks_and_the_tool_bar_keep_their_size_with_panels_on_each_side() {
    let (app, seen) = watch(small_editor());
    drive(app, vec![Step::Wait, Step::Wait, Step::Wait]);
    let paint = seen.lock().unwrap().clone();
    let (app, seen) = watch(small_editor());
    let out = drive(app, layout("window-layout-rig").to_vec());
    common::save(&out.frame, "workspace-rig-room");
    let rig = seen.lock().unwrap().clone();
    // The canvas gives way, not the tool bar, whose second column of tools
    // would go.
    assert_eq!(rig.toolbar, paint.toolbar, "the tool bar keeps its width");
    assert!(
        (rig.size("panel-color")[2] - paint.size("panel-color")[2]).abs() < 2.0,
        "a side column keeps its width: {} {}",
        rig.size("panel-color")[2],
        paint.size("panel-color")[2]
    );
}

#[test]
fn folding_a_panel_gives_its_room_to_the_others_and_unfolding_takes_it_back() {
    let open = |extra: &[Step]| {
        let (app, seen) = watch(editor());
        let mut steps = vec![Step::Menu("window-workspace"), Step::Wait, Step::Wait];
        steps.extend_from_slice(extra);
        steps.extend([Step::Wait, Step::Wait]);
        drive(app, steps);
        seen.lock().unwrap().clone()
    };
    let before = open(&[]);
    let after = open(&[Step::Click("ws-fold-panel-color")]);
    assert_eq!(strs(&after.folded), ["panel-color", "panel-rig"]);
    assert!(
        after.size("panel-history")[3] > before.size("panel-history")[3] + 100.0,
        "the history got some of the color's room: {:?} {:?}",
        before.size("panel-history"),
        after.size("panel-history")
    );
    assert!(
        after.size("panel-palette")[1] < before.size("panel-palette")[1],
        "the palette moved up under the folded color"
    );
    let back = open(&[
        Step::Click("ws-fold-panel-color"),
        Step::Wait,
        Step::Wait,
        Step::Click("ws-fold-panel-color"),
    ]);
    assert_eq!(strs(&back.folded), ["panel-rig"]);
    assert!((back.size("panel-color")[3] - before.size("panel-color")[3]).abs() < 2.0);
}

#[test]
fn a_rig_mode_unfolds_the_rig_panel() {
    let (app, seen) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Wait,
            Step::Click("rig-mode-bones"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert!(seen.lock().unwrap().folded.is_empty(), "New skeleton shows");
    assert_eq!(
        out.message.as_deref(),
        Some("Make a skeleton first: New skeleton in the Rig panel")
    );
}

#[test]
fn the_dialog_shows_hides_moves_and_pops_out_panels() {
    let (app, seen) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Menu("window-workspace"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Click("ws-edge-left-panel-history"),
            Step::Wait,
            Step::Wait,
            Step::Click("ws-show-panel-layers"),
            Step::Wait,
            Step::Click("ws-edge-left-panel-layers"),
            Step::Wait,
            Step::Wait,
            Step::Click("ws-edge-bottom-panel-brushes"),
            Step::Wait,
            Step::Click("ws-show-panel-palette"),
            Step::Wait,
            Step::Wait,
            Step::Click("ws-window-panel-color"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "workspace-dialog");
    let s = seen.lock().unwrap().clone();
    assert_eq!(strs(&s.left), ["panel-history", "panel-layers"]);
    assert_eq!(strs(&s.bottom), ["panel-timeline", "panel-brushes"]);
    assert!(s.hidden.contains(&"panel-palette".to_string()));
    assert_eq!(strs(&s.windows), ["panel-color"]);

    // The Window chip docks it back where it was.
    let (app, seen) = watch(editor());
    drive(
        app,
        vec![
            Step::Menu("window-workspace"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Click("ws-window-panel-color"),
            Step::Wait,
            Step::Wait,
            Step::Click("ws-window-panel-color"),
            Step::Wait,
            Step::Wait,
        ],
    );
    let s = seen.lock().unwrap().clone();
    assert!(s.windows.is_empty());
    assert_eq!(s.right.first().map(String::as_str), Some("panel-color"));
}

#[test]
fn show_all_and_reset_layout() {
    let (app, seen) = watch(editor());
    drive(
        app,
        vec![
            Step::Wait,
            Step::Menu("window-show-all"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert!(seen.lock().unwrap().hidden.is_empty());

    let (app, seen) = watch(editor());
    drive(
        app,
        vec![
            Step::Menu("window-workspace"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Click("ws-edge-left-panel-history"),
            Step::Wait,
            Step::Wait,
            Step::Menu("window-layout-reset"),
            Step::Wait,
            Step::Wait,
        ],
    );
    let s = seen.lock().unwrap().clone();
    assert!(s.left.is_empty(), "{:?}", s.left);
    assert!(s.right.contains(&"panel-history".to_string()));
}

#[test]
fn a_saved_layout_is_listed_and_switched_back_to() {
    let (app, seen) = watch(editor());
    drive(
        app,
        vec![
            Step::Menu("window-workspace"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Click("ws-edge-left-panel-history"),
            Step::Wait,
            Step::Wait,
            Step::Text("ws_name", "Mine"),
            Step::Click("ws-save"),
            Step::Wait,
            Step::Wait,
            Step::Menu("window-layout-paint"),
            Step::Wait,
            Step::Wait,
            // The fifth row: after the four built-in layouts.
            Step::Click("ws-layout-4"),
            Step::Wait,
            Step::Wait,
        ],
    );
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.layout, "Mine");
    assert_eq!(strs(&s.left), ["panel-history"]);
}

#[test]
fn a_tab_dragged_onto_the_timeline_joins_its_stack() {
    let (app, seen) = watch(editor());
    // Its tab is in the strip just above the panel.
    let tab = At::Offset("panel-history", 24.0, -10.0);
    let to = At::Within("panel-timeline", 0.5, 0.5);
    let out = drive(
        app,
        vec![
            Step::Wait,
            Step::Wait,
            Step::Move(tab),
            Step::Down(tab, PointerButton::Primary),
            Step::Move(At::Within("panel-canvas", 0.5, 0.5)),
            Step::Move(to),
            Step::Move(to),
            Step::Up(to, PointerButton::Primary),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "workspace-dragged");
    let s = seen.lock().unwrap().clone();
    assert!(
        s.bottom.contains(&"panel-history".to_string()),
        "{:?} {:?}",
        s.bottom,
        s.right
    );
    assert!(!s.right.contains(&"panel-history".to_string()));
}

#[test]
fn an_arrangement_from_workspace_json_is_carried_over_and_the_file_goes() {
    let dir = std::env::temp_dir().join(format!("pixl-ws-legacy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // As the Rig layout, with the Layers panel shown on the left, the
    // brushes folded on the right, and a wider right side.
    let file = dir.join(pixl::workspace::LEGACY_FILE);
    std::fs::write(
        &file,
        r#"{"live":{"name":"Rig","panels":[
          {"panel":"panel-color","dock":"left","collapsed":false,"shown":true},
          {"panel":"panel-palette","dock":"left","collapsed":false,"shown":true},
          {"panel":"panel-layers","dock":"left","collapsed":false,"shown":true},
          {"panel":"panel-rig","dock":"right","collapsed":false,"shown":true},
          {"panel":"panel-history","dock":"right","collapsed":false,"shown":true},
          {"panel":"panel-brushes","dock":"right","collapsed":true,"shown":true},
          {"panel":"panel-tileset","dock":"right","collapsed":false,"shown":false},
          {"panel":"panel-timeline","dock":"bottom","collapsed":false,"shown":true},
          {"panel":"panel-preview","dock":"bottom","collapsed":false,"shown":false}],
          "sizes":{"left":null,"right":320.0,"bottom":null}},
          "layouts":[{"name":"Mine","panels":[
          {"panel":"panel-history","dock":"left","collapsed":false,"shown":true}]}]}"#,
    )
    .unwrap();
    let app = editor().insert_resource(Workspace::with_legacy(Store::at(&dir)));
    let (app, seen) = watch(app);
    drive(app, vec![Step::Wait, Step::Wait, Step::Wait, Step::Wait]);
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.layout, "Rig");
    assert_eq!(
        strs(&s.left),
        ["panel-color", "panel-palette", "panel-layers"]
    );
    assert_eq!(
        strs(&s.right),
        ["panel-rig", "panel-history", "panel-brushes"]
    );
    assert_eq!(strs(&s.folded), ["panel-brushes"]);
    assert!(
        (s.size("panel-rig")[2] - 320.0).abs() < 4.0,
        "the right side kept its width: {}",
        s.size("panel-rig")[2]
    );
    assert!(!file.exists(), "the dock keeps the arrangement now");
    let _ = std::fs::remove_dir_all(dir);
    assert!(!s.windows.iter().any(|w| w == CANVAS));
}

#[test]
fn a_saved_user_layout_is_carried_over_too() {
    let dir = std::env::temp_dir().join(format!("pixl-ws-mine-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(pixl::workspace::LEGACY_FILE),
        r#"{"live":{"name":"Paint","panels":[]},"layouts":[{"name":"Mine","panels":[
          {"panel":"panel-history","dock":"left","collapsed":false,"shown":true}]}]}"#,
    )
    .unwrap();
    let app = editor().insert_resource(Workspace::with_legacy(Store::at(&dir)));
    let (app, seen) = watch(app);
    drive(
        app,
        vec![
            Step::Wait,
            Step::Wait,
            Step::Menu("window-workspace"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Click("ws-layout-4"),
            Step::Wait,
            Step::Wait,
        ],
    );
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.layout, "Mine");
    assert_eq!(strs(&s.left), ["panel-history"]);
    let _ = std::fs::remove_dir_all(dir);
}
