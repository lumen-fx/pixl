//! The workspace: pixl's panels in Lumen's dock around the canvas. The dock
//! itself (`lumenui-dock`) moves panels by dragging their tabs, pops them out
//! into windows of their own, keeps the arrangement between runs and holds
//! the named layouts; the markup declares the panels, the default
//! arrangement and the built-in layouts. This module adds what pixl puts on
//! top: the Window menu's layout commands, the Panels and layouts dialog,
//! folding a panel to its tab, and taking over the arrangement an earlier
//! pixl kept in `workspace.json`.

use lumenui::lumen_core::components::{A11yLabel, DirtyLayout};
use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::lumen_primitives::TabStripButton;
use lumenui::prelude::*;
use lumenui_dock::model::Node;
use lumenui_dock::{Dock, DockCloseButton, DockTab, DockTabCaption, Layout, Persist};
use serde::Deserialize;

use crate::home::Home;
use crate::ids::ElementIds;
use crate::panels::escape;
use crate::settings::keymap::KeyCommands;
use crate::settings::store::Store;

/// The id of the dock the panels sit in.
pub const DOCK: &str = "dock";

/// The panel holding the canvas: always docked, never listed.
pub const CANVAS: &str = "panel-canvas";

/// The built-in layouts, in the order the Window menu lists them. The
/// markup writes them as `<dock-layout>`s.
pub const BUILT_IN: [&str; 4] = ["Paint", "Animate", "Rig", "Tiles"];

/// The file an earlier pixl kept its arrangement in, before the dock: read
/// once, carried over, and removed.
pub const LEGACY_FILE: &str = "workspace.json";

/// The sides of the canvas the dialog moves panels to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 3] = [Edge::Left, Edge::Right, Edge::Bottom];

    fn key(self) -> &'static str {
        match self {
            Edge::Left => "left",
            Edge::Right => "right",
            Edge::Bottom => "bottom",
        }
    }

    fn from_key(key: &str) -> Option<Edge> {
        Edge::ALL.into_iter().find(|e| e.key() == key)
    }

    /// The short name the panel list's buttons show.
    fn short(self) -> &'static str {
        match self {
            Edge::Left => "Left",
            Edge::Right => "Right",
            Edge::Bottom => "Bottom",
        }
    }
}

/// What the rest of pixl reads about the workspace, and the dialog's state.
#[derive(Resource)]
pub struct Workspace {
    /// The named layout the arrangement came from.
    pub layout: String,
    /// Every layout name: the built-in ones, then the user's.
    pub names: Vec<String>,
    /// Where an earlier pixl's `workspace.json` is looked for.
    legacy: Store,
    /// The dock has its first arrangement: the one it kept, the one carried
    /// over from `workspace.json`, or the Paint layout.
    started: bool,
    /// What the panel list and layout list last showed.
    shown_lists: Option<(String, String)>,
}

impl Workspace {
    /// The workspace of the `pixl` program, which carries over the
    /// arrangement in pixl's data folder.
    pub fn user() -> Self {
        Self::with_legacy(Store::user())
    }

    /// A workspace with nothing to carry over.
    pub fn in_memory() -> Self {
        Self::with_legacy(Store::in_memory())
    }

    /// A workspace that carries over the `workspace.json` in `store`.
    pub fn with_legacy(store: Store) -> Self {
        Self {
            layout: String::new(),
            names: BUILT_IN.iter().map(|s| s.to_string()).collect(),
            legacy: store,
            started: false,
            shown_lists: None,
        }
    }
}

/// The layout names in the order pixl lists them: the built-in ones, then
/// the user's in the dock's order.
fn ordered_names(dock: &Dock) -> Vec<String> {
    let mut out: Vec<String> = BUILT_IN.iter().map(|s| s.to_string()).collect();
    out.extend(
        dock.layouts()
            .into_iter()
            .filter(|n| !BUILT_IN.contains(&n.as_str())),
    );
    out
}

/// The panels the dialog lists, in the order of their titles.
fn listed_panels(dock: &Dock) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = dock
        .panels()
        .into_iter()
        .filter(|p| p != CANVAS)
        .map(|p| {
            let title = dom::get_by_id(&p)
                .and_then(|n| n.get_attr("title"))
                .unwrap_or_else(|| p.clone());
            (p, title)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Whether `node` holds `panel`.
fn holds(node: &Node, panel: &str) -> bool {
    node.panels().iter().any(|p| *p == panel)
}

/// The side of the canvas `panel` is on in `layout`'s dock, if it is docked
/// there. Every pixl layout is a row of columns around the canvas's column,
/// so a panel in a column before the canvas's is on its left, one after it
/// on its right, and one in the canvas's column below it.
pub fn edge_of(layout: &Layout, panel: &str) -> Option<Edge> {
    let Node::Split(row) = &layout.docks.get(DOCK)?.root else {
        return None;
    };
    let center = row.children.iter().position(|c| holds(&c.node, CANVAS))?;
    let at = row.children.iter().position(|c| holds(&c.node, panel))?;
    if at < center {
        return Some(Edge::Left);
    }
    if at > center {
        return Some(Edge::Right);
    }
    let canvas = layout.find(CANVAS)?.stack;
    (layout.find(panel)?.stack != canvas).then_some(Edge::Bottom)
}

/// The panels docked on `edge` of the canvas, in tree order.
fn panels_on(layout: &Layout, edge: Edge) -> Vec<String> {
    let Some(area) = layout.docks.get(DOCK) else {
        return Vec::new();
    };
    area.root
        .panels()
        .into_iter()
        .filter(|p| edge_of(layout, p) == Some(edge))
        .cloned()
        .collect()
}

/// Moves `panel` to `edge` of the canvas: below the last panel there (to
/// the right of it below the canvas), or along that edge when none is.
/// Shows it if it was hidden.
pub fn move_to_edge(dock: &Dock, panel: &str, edge: Edge) {
    let layout = dock.save();
    if dock.is_shown(panel) && edge_of(&layout, panel) == Some(edge) {
        return;
    }
    let others: Vec<String> = panels_on(&layout, edge)
        .into_iter()
        .filter(|p| p != panel)
        .collect();
    // A panel the dock refuses to move keeps its place.
    let _ = match (others.last(), edge) {
        (Some(last), Edge::Bottom) => dock.move_to(panel, last, "right"),
        (Some(last), _) => dock.move_to(panel, last, "bottom"),
        (None, Edge::Bottom) => dock.move_to(panel, CANVAS, "bottom"),
        (None, side) => dock.move_to(panel, DOCK, side.key()),
    };
}

/// Whether the stack holding `panel` shows only its tab.
pub fn folded(layout: &Layout, panel: &str) -> bool {
    layout
        .find(panel)
        .and_then(|loc| layout.stack(&loc.stack).map(|s| s.collapsed))
        .unwrap_or(false)
}

/// How much of the height of its column each panel takes while it is
/// open, as the markup's layouts give it: the color and palette panels
/// their content's height, the others a share of what is left.
const SIZES: [(&str, Size); 7] = [
    ("panel-color", Size::Basis(400.0)),
    ("panel-palette", Size::Basis(92.0)),
    ("panel-layers", Size::Share(40.0)),
    ("panel-rig", Size::Share(30.0)),
    ("panel-history", Size::Share(23.0)),
    ("panel-brushes", Size::Share(20.0)),
    ("panel-tileset", Size::Share(26.0)),
];

/// A panel's height in its column.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Size {
    /// This many pixels.
    Basis(f32),
    /// This share of the room the fixed panels leave.
    Share(f32),
}

/// The height `panel` takes in its column while open.
fn size_of(panel: &str) -> Size {
    SIZES
        .iter()
        .find(|(p, _)| *p == panel)
        .map_or(Size::Share(20.0), |(_, s)| *s)
}

/// Folds or unfolds the stack `stack` under `node`, giving its room in the
/// split it is in to the stacks beside it while folded. Returns whether it
/// found the stack.
fn fold_in(node: &mut Node, stack: &str, fold: bool, size: Size) -> bool {
    let Node::Split(split) = node else {
        return false;
    };
    for child in &mut split.children {
        if let Node::Stack(s) = &mut child.node
            && s.id == stack
        {
            s.collapsed = fold;
            (child.basis, child.grow) = match (fold, size) {
                (true, _) => (None, 0.0),
                (false, Size::Basis(px)) => (Some(px), 0.0),
                (false, Size::Share(share)) => (None, share),
            };
            return true;
        }
        if fold_in(&mut child.node, stack, fold, size) {
            return true;
        }
    }
    false
}

/// Folds `panel`'s stack to its tab, giving its room to the stacks beside
/// it, or unfolds it to its share again. lumen-dock 0.0.11's own collapse
/// keeps a folded stack's room (lumen-fx/lumen#641), so this sets the
/// layout itself.
pub fn set_folded(dock: &Dock, panel: &str, fold: bool) {
    let mut layout = dock.save();
    let Some(loc) = layout.find(panel) else {
        return;
    };
    let size = size_of(panel);
    let found = layout
        .docks
        .values_mut()
        .any(|area| fold_in(&mut area.root, &loc.stack, fold, size));
    if found {
        dock.restore(&layout);
    }
}

/// Folds `panel`'s stack to its tab, or unfolds it.
pub fn toggle_fold(dock: &Dock, panel: &str) {
    let now = folded(&dock.save(), panel);
    set_folded(dock, panel, !now);
}

/// Shows `panel` unfolded: what a mode that works in a panel does.
pub fn unfold(dock: &Dock, panel: &str) {
    if !dock.is_shown(panel) {
        let _ = dock.show(panel);
    }
    if folded(&dock.save(), panel) {
        set_folded(dock, panel, false);
    }
}

/// Puts the layout called `name` on screen.
pub fn switch(dock: &Dock, name: &str) -> bool {
    dock.apply_layout(name).is_ok()
}

// ---- carrying over workspace.json -------------------------------------------

/// A panel as `workspace.json` placed it.
#[derive(Deserialize)]
struct LegacyPlace {
    panel: String,
    dock: String,
    #[serde(default)]
    collapsed: bool,
    #[serde(default = "yes")]
    shown: bool,
}

fn yes() -> bool {
    true
}

/// The docks' sizes as `workspace.json` kept them.
#[derive(Deserialize, Default)]
struct LegacySizes {
    #[serde(default)]
    left: Option<f32>,
    #[serde(default)]
    right: Option<f32>,
    #[serde(default)]
    bottom: Option<f32>,
}

/// A layout as `workspace.json` kept it.
#[derive(Deserialize)]
struct LegacyLayout {
    name: String,
    #[serde(default)]
    panels: Vec<LegacyPlace>,
    #[serde(default)]
    sizes: LegacySizes,
}

/// What `workspace.json` held.
#[derive(Deserialize)]
struct Legacy {
    live: Option<LegacyLayout>,
    #[serde(default)]
    layouts: Vec<LegacyLayout>,
}

/// Arranges the dock as `old` was: from the built-in layout it names (Paint
/// for one of the user's), each panel shown, hidden, folded and on the side
/// of the canvas it had, and the side columns and the bottom row as wide
/// and tall as they were.
fn arrange_like(dock: &Dock, old: &LegacyLayout) {
    let base = if BUILT_IN.contains(&old.name.as_str()) {
        old.name.as_str()
    } else {
        "Paint"
    };
    let _ = dock.apply_layout(base);
    let known = dock.panels();
    for place in old.panels.iter().filter(|p| known.contains(&p.panel)) {
        if !place.shown {
            if dock.is_shown(&place.panel) {
                let _ = dock.hide(&place.panel);
            }
            continue;
        }
        if let Some(edge) = Edge::from_key(&place.dock) {
            move_to_edge(dock, &place.panel, edge);
        }
        if folded(&dock.save(), &place.panel) != place.collapsed {
            set_folded(dock, &place.panel, place.collapsed);
        }
    }
    let mut layout = dock.save();
    let Some(area) = layout.docks.get_mut(DOCK) else {
        return;
    };
    let Node::Split(row) = &mut area.root else {
        return;
    };
    let Some(center) = row.children.iter().position(|c| holds(&c.node, CANVAS)) else {
        return;
    };
    let last = row.children.len() - 1;
    for (i, child) in row.children.iter_mut().enumerate() {
        let size = if i < center {
            old.sizes.left
        } else if i > center && i == last {
            old.sizes.right
        } else {
            None
        };
        if size.is_some() {
            child.basis = size;
        }
    }
    if let Node::Split(column) = &mut row.children[center].node
        && let Some(bottom) = old.sizes.bottom
    {
        for child in &mut column.children {
            if !holds(&child.node, CANVAS) {
                child.basis = Some(bottom);
            }
        }
    }
    dock.restore(&layout);
}

/// Takes over the arrangement and the user's layouts an earlier pixl kept
/// in `workspace.json`, then removes the file. Returns whether there was
/// one.
fn carry_over(dock: &Dock, store: &Store) -> bool {
    let Some(old) = store.load::<Legacy>(LEGACY_FILE) else {
        return false;
    };
    for layout in old
        .layouts
        .iter()
        .filter(|l| !BUILT_IN.contains(&l.name.as_str()))
    {
        arrange_like(dock, layout);
        dock.save_layout(&layout.name);
    }
    match &old.live {
        Some(live) => arrange_like(dock, live),
        None => {
            let _ = dock.apply_layout("Paint");
        }
    }
    if let Some(path) = store.path(LEGACY_FILE) {
        // The dock keeps the arrangement now; a file left behind would be
        // carried over again.
        let _ = std::fs::remove_file(path);
    }
    true
}

/// Whether the dock kept an arrangement from an earlier run, which it puts
/// back on its own.
fn dock_kept_one(persist: Option<&Persist>) -> bool {
    persist == Some(&Persist::Auto)
        && lumenui::lumen_core::app_paths::data_dir()
            .join(lumenui_dock::FILE)
            .exists()
}

// ---- systems ------------------------------------------------------------------

/// The Window menu's layout commands and the dialog's buttons.
pub fn workspace_input(
    mut ws: ResMut<Workspace>,
    dock: Dock,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    (home, keys): (Res<Home>, Res<KeyCommands>),
    mut signals: Signals,
) {
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    // The Window menu's commands run from their keys too.
    chosen.extend(
        keys.pressed
            .iter()
            .filter(|(id, repeat)| !repeat && id.starts_with("window-"))
            .map(|(id, _)| id.clone()),
    );
    chosen.extend(clicks.read().filter_map(|c| ids.id_of(c.entity)));
    if home.open {
        return;
    }
    for id in chosen {
        run(&mut ws, &dock, &id, &mut signals);
    }
}

/// Carries out one workspace command by its element or menu item id.
pub fn run(ws: &mut Workspace, dock: &Dock, id: &str, signals: &mut Signals) {
    match id {
        "menu-window" => {
            signals.toggle("__menu_open:window-menu");
        }
        "window-workspace" => {
            ws.shown_lists = None;
            signals.set("ws_message", String::new());
            signals.set("ws_open", true);
        }
        "ws-close" => signals.set("ws_open", false),
        "window-show-all" => {
            for (panel, _) in listed_panels(dock) {
                if !dock.is_shown(&panel) {
                    let _ = dock.show(&panel);
                }
            }
        }
        "window-layout-reset" | "ws-reset" => dock.reset(),
        "ws-save" => {
            let name = signals.get::<String>("ws_name").unwrap_or_default();
            let name = name.trim();
            let note = if name.is_empty() {
                "Name the layout first".to_string()
            } else if BUILT_IN.contains(&name) {
                format!("{name} is a built-in layout; pick another name")
            } else {
                dock.save_layout(name);
                format!("Saved {name}")
            };
            ws.shown_lists = None;
            signals.set("ws_message", note);
        }
        "ws-delete" => {
            let name = ws.layout.clone();
            let note = if dock.delete_layout(&name) {
                format!("Deleted {name}")
            } else {
                format!("{name} is built in")
            };
            ws.shown_lists = None;
            signals.set("ws_message", note);
        }
        other => {
            if let Some(name) = other.strip_prefix("window-layout-") {
                let name = BUILT_IN
                    .iter()
                    .find(|b| b.eq_ignore_ascii_case(name))
                    .map_or_else(|| name.to_string(), |b| b.to_string());
                switch(dock, &name);
            } else if let Some(k) = other
                .strip_prefix("ws-layout-")
                .and_then(|k| k.parse::<usize>().ok())
            {
                if let Some(name) = ws.names.get(k).cloned() {
                    switch(dock, &name);
                    signals.set("ws_name", name);
                    ws.shown_lists = None;
                }
            } else if let Some(rest) = other.strip_prefix("ws-") {
                panel_row_command(dock, rest);
                ws.shown_lists = None;
            }
        }
    }
}

/// A click in the dialog's panel list: `show-<panel>`, `fold-<panel>`,
/// `window-<panel>` or `edge-<edge>-<panel>`.
fn panel_row_command(dock: &Dock, rest: &str) {
    if let Some(panel) = rest.strip_prefix("show-") {
        let _ = dock.toggle(panel);
    } else if let Some(panel) = rest.strip_prefix("fold-") {
        if dock.is_shown(panel) {
            toggle_fold(dock, panel);
        }
    } else if let Some(panel) = rest.strip_prefix("window-") {
        let _ = if dock.window_of(panel) > 0 {
            dock.attach(panel)
        } else {
            dock.detach(panel)
        };
    } else if let Some(rest) = rest.strip_prefix("edge-")
        && let Some((edge, panel)) = rest.split_once('-')
        && let Some(edge) = Edge::from_key(edge)
    {
        move_to_edge(dock, panel, edge);
    }
}

/// The dialog's panel list, one row per panel.
fn panel_list_markup(dock: &Dock) -> String {
    let layout = dock.save();
    let mut out = String::new();
    for (p, title) in listed_panels(dock) {
        let shown = dock.is_shown(&p);
        let (state, label) = if shown {
            ("on", "Shown")
        } else {
            ("off", "Hidden")
        };
        out.push_str(&format!(
            r#"<row id="ws-row-{p}" class="ws-row"><label class="ws-panel-name" text="{title}"/><button id="ws-show-{p}" class="ws-chip {state}" text="{label}"/>"#,
            title = escape(&title),
        ));
        let in_window = dock.window_of(&p) > 0;
        let edge = if shown && !in_window {
            edge_of(&layout, &p)
        } else {
            None
        };
        for e in Edge::ALL {
            let active = if edge == Some(e) { " active" } else { "" };
            out.push_str(&format!(
                r#"<button id="ws-edge-{key}-{p}" class="ws-chip{active}" text="{short}"/>"#,
                key = e.key(),
                short = e.short(),
            ));
        }
        let window = if in_window { " active" } else { "" };
        let fold = if shown && folded(&layout, &p) {
            " active"
        } else {
            ""
        };
        out.push_str(&format!(
            r#"<button id="ws-window-{p}" class="ws-chip{window}" text="Window"/><button id="ws-fold-{p}" class="ws-chip{fold}" text="Folded"/></row>"#
        ));
    }
    out
}

/// The dialog's layout list: the built-in layouts, then the user's.
fn layout_list_markup(ws: &Workspace) -> String {
    let mut out = String::new();
    for (k, name) in ws.names.iter().enumerate() {
        let current = if *name == ws.layout { " current" } else { "" };
        let kind = if BUILT_IN.contains(&name.as_str()) {
            "built in"
        } else {
            "yours"
        };
        out.push_str(&format!(
            r#"<row id="ws-layout-{k}" class="ws-layout{current}"><label class="ws-layout-name" text="{name}"/><spacer/><label class="ws-layout-kind" text="{kind}"/></row>"#,
            name = escape(name),
        ));
    }
    out
}

/// Gives the dock its first arrangement once it has read the markup: the
/// one it kept, else the one carried over from `workspace.json`, else the
/// Paint layout. Then keeps the layout name and the names pixl lists in
/// step with the dock, and the dialog's lists current.
pub fn show_workspace(
    mut ws: ResMut<Workspace>,
    dock: Dock,
    signals: Signals,
    (persist, wake): (Option<Res<Persist>>, Option<Res<WakeDeadline>>),
) {
    if !ws.started {
        if !dock.panels().iter().any(|p| p == CANVAS) {
            return;
        }
        ws.started = true;
        if !dock_kept_one(persist.as_deref()) && !carry_over(&dock, &ws.legacy) {
            let _ = dock.apply_layout("Paint");
        }
        crate::repaint_soon(&wake);
    }
    let layout = signals.get::<String>("dock.layout").unwrap_or_default();
    if layout != ws.layout {
        ws.layout = layout;
        ws.shown_lists = None;
    }
    let names = ordered_names(&dock);
    if names != ws.names {
        ws.names = names;
        ws.shown_lists = None;
    }
    if signals.get::<bool>("ws_open") != Some(true) {
        ws.shown_lists = None;
        return;
    }
    let lists = (panel_list_markup(&dock), layout_list_markup(&ws));
    if ws.shown_lists.as_ref() != Some(&lists)
        && let (Some(panels), Some(layouts)) = (
            dom::get_by_id("ws-panel-list"),
            dom::get_by_id("ws-layout-list"),
        )
    {
        panels.set_inner_markup(&lists.0);
        layouts.set_inner_markup(&lists.1);
        ws.shown_lists = Some(lists);
        crate::repaint_soon(&wake);
    }
}

/// Keeps each tab's caption, accessible name and close button on the panel
/// it shows, and takes the close button off a panel that cannot close.
/// lumen-dock 0.0.11 sets them only when a tab's button is made, and moving
/// panels hands buttons to other panels, so a tab read another panel's
/// title and its close button closed that other panel
/// (lumen-fx/lumen#640).
pub fn retitle_reused_tabs(
    buttons: Query<(Entity, &TabStripButton, &Children), Changed<TabStripButton>>,
    tabs: Query<&DockTab>,
    mut captions: Query<&mut TextContent, With<DockTabCaption>>,
    mut closes: Query<&mut DockCloseButton>,
    mut commands: Commands,
    wake: Option<Res<WakeDeadline>>,
) {
    for (button, b, children) in &buttons {
        let Ok(tab) = tabs.get(b.tab) else {
            continue;
        };
        let node = dom::get_by_id(&tab.panel);
        let title = node
            .and_then(|n| n.get_attr("title"))
            .unwrap_or_else(|| tab.panel.clone());
        let closable = node.and_then(|n| n.get_attr("closable")).as_deref() != Some("false");
        let mut changed = false;
        for &child in children {
            if let Ok(mut text) = captions.get_mut(child)
                && text.0 != title
            {
                text.0 = title.clone();
                commands.entity(child).insert(DirtyLayout);
                changed = true;
            }
            if let Ok(mut close) = closes.get_mut(child) {
                if !closable {
                    // A button handed to the canvas keeps no close button.
                    commands.entity(child).despawn();
                    changed = true;
                } else if close.panel != tab.panel {
                    close.panel = tab.panel.clone();
                    changed = true;
                }
            }
        }
        if changed {
            commands.entity(button).insert(A11yLabel(title));
            crate::repaint_soon(&wake);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layout as pixl's are: a column of panels left of the canvas, the
    /// canvas above the timeline, and a column on the right.
    fn layout() -> Layout {
        let json = r#"{
          "lumen-dock": 2,
          "docks": { "dock": { "root": { "split": "row", "id": "r", "children": [
            { "basis": 264, "node": { "stack": "s1", "panels": ["panel-color"], "active": "panel-color" } },
            { "grow": 1, "node": { "split": "column", "id": "c", "children": [
              { "grow": 1, "node": { "stack": "s2", "panels": ["panel-canvas"], "active": "panel-canvas", "center": true } },
              { "basis": 230, "node": { "stack": "s3", "panels": ["panel-timeline"], "active": "panel-timeline" } } ] } },
            { "basis": 264, "node": { "split": "column", "id": "rr", "children": [
              { "node": { "stack": "s4", "panels": ["panel-rig", "panel-history"], "active": "panel-rig", "collapsed": true } },
              { "grow": 1, "node": { "stack": "s5", "panels": ["panel-brushes"], "active": "panel-brushes" } } ] } }
          ] } } }
        }"#;
        Layout::from_json(json, 0).expect("a layout")
    }

    #[test]
    fn a_panel_is_on_the_side_of_the_canvas_its_column_is() {
        let l = layout();
        assert_eq!(edge_of(&l, "panel-color"), Some(Edge::Left));
        assert_eq!(edge_of(&l, "panel-timeline"), Some(Edge::Bottom));
        assert_eq!(edge_of(&l, "panel-history"), Some(Edge::Right));
        assert_eq!(edge_of(&l, CANVAS), None);
        assert_eq!(edge_of(&l, "panel-layers"), None, "not docked");
        assert_eq!(
            panels_on(&l, Edge::Right),
            ["panel-rig", "panel-history", "panel-brushes"]
        );
    }

    #[test]
    fn a_folded_stack_folds_every_panel_in_it() {
        let l = layout();
        assert!(folded(&l, "panel-history"));
        assert!(!folded(&l, "panel-color"));
    }

    #[test]
    fn workspace_json_reads_as_an_earlier_pixl_wrote_it() {
        let text = r#"{"live":{"name":"Rig","panels":[
            {"panel":"panel-color","dock":"left","collapsed":false,"shown":true},
            {"panel":"panel-layers","dock":"right","collapsed":false,"shown":false}],
            "sizes":{"left":null,"right":300.0,"bottom":null}},"layouts":[]}"#;
        let old: Legacy = serde_json::from_str(text).expect("it reads");
        let live = old.live.expect("a live arrangement");
        assert_eq!(live.name, "Rig");
        assert!(!live.panels[1].shown);
        assert_eq!(live.sizes.right, Some(300.0));
    }
}
