//! The workspace: which dock each panel sits in (left, right or bottom of
//! the canvas), their order, which are collapsed or hidden, and how wide or
//! tall the docks are; named layouts of all of that, three built in and any
//! number of the user's, kept in `workspace.json` in pixl's data folder.
//!
//! A panel is any element with the class `panel` inside a dock, found in
//! the markup when the window opens; where the markup puts it is its
//! default place. A layout names the panels it places and leaves the rest
//! where the markup put them, so a panel added later joins every layout.

use lumenui::ecs::hierarchy::ChildOf;
use lumenui::lumen_core::input::{ContextMenuEvent, MenuClicked, PointerReleased};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use serde::{Deserialize, Serialize};

use crate::home::Home;
use crate::keymap::KeyCommands;
use crate::panels::escape;
use crate::store::Store;

/// The file the workspace is kept in.
pub const FILE: &str = "workspace.json";

/// The built-in layouts, in the order the Window menu lists them.
pub const BUILT_IN: [&str; 3] = ["Paint", "Animate", "Rig"];

/// The docks around the canvas.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Dock {
    Left,
    Right,
    Bottom,
}

impl Dock {
    pub const ALL: [Dock; 3] = [Dock::Left, Dock::Right, Dock::Bottom];

    /// The id of the dock's element.
    pub fn element_id(self) -> &'static str {
        match self {
            Dock::Left => "dock-left",
            Dock::Right => "dock-right",
            Dock::Bottom => "dock-bottom",
        }
    }

    pub fn from_element_id(id: &str) -> Option<Dock> {
        Dock::ALL.into_iter().find(|d| d.element_id() == id)
    }

    /// The short name the panel list's dock buttons show.
    pub fn short(self) -> &'static str {
        match self {
            Dock::Left => "Left",
            Dock::Right => "Right",
            Dock::Bottom => "Bottom",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Bottom => "bottom",
        }
    }

    fn from_key(key: &str) -> Option<Dock> {
        Dock::ALL.into_iter().find(|d| d.key() == key)
    }
}

/// Where one panel is.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The panel element's id.
    pub panel: String,
    pub dock: Dock,
    /// Only the header shows.
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default = "shown")]
    pub shown: bool,
}

fn shown() -> bool {
    true
}

/// The size of each dock across the canvas: the left and right docks'
/// width, the bottom dock's height. `None` is the theme's size.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Sizes {
    #[serde(default)]
    pub left: Option<f32>,
    #[serde(default)]
    pub right: Option<f32>,
    #[serde(default)]
    pub bottom: Option<f32>,
}

impl Sizes {
    pub fn get(&self, dock: Dock) -> Option<f32> {
        match dock {
            Dock::Left => self.left,
            Dock::Right => self.right,
            Dock::Bottom => self.bottom,
        }
    }

    pub fn set(&mut self, dock: Dock, size: Option<f32>) {
        match dock {
            Dock::Left => self.left = size,
            Dock::Right => self.right = size,
            Dock::Bottom => self.bottom = size,
        }
    }
}

/// A named arrangement of the panels.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub name: String,
    /// The panels this layout places, each dock's in order from the top
    /// (left for the bottom dock).
    #[serde(default)]
    pub panels: Vec<Place>,
    #[serde(default)]
    pub sizes: Sizes,
}

impl Layout {
    /// One of the built-in layouts.
    pub fn built_in(name: &str) -> Option<Layout> {
        let place = |panel: &str, dock: Dock, shown: bool| Place {
            panel: panel.to_string(),
            dock,
            collapsed: false,
            shown,
        };
        let panels = match name {
            // Drawing: color and layers at hand, nothing below the canvas.
            "Paint" => vec![
                place("panel-color", Dock::Right, true),
                place("panel-palette", Dock::Right, true),
                place("panel-layers", Dock::Right, true),
                place("panel-history", Dock::Right, true),
            ],
            // Animation: the frames below the canvas, history out of the way.
            "Animate" => vec![
                place("panel-color", Dock::Right, true),
                place("panel-palette", Dock::Right, true),
                place("panel-layers", Dock::Right, true),
                place("panel-history", Dock::Right, false),
            ],
            // Rigging: layers and history on the right, colors on the left.
            "Rig" => vec![
                place("panel-color", Dock::Left, true),
                place("panel-palette", Dock::Left, true),
                place("panel-layers", Dock::Right, true),
                place("panel-history", Dock::Right, true),
            ],
            _ => return None,
        };
        Some(Layout {
            name: name.to_string(),
            panels,
            sizes: Sizes::default(),
        })
    }

    /// Every panel of `known` (each with its default dock, in markup order)
    /// placed: as this layout says, else where the markup puts it, shown.
    /// Panels the layout names that are not in the window are left out.
    pub fn resolve(&self, known: &[(String, Dock)]) -> Vec<Place> {
        let mut out: Vec<Place> = self
            .panels
            .iter()
            .filter(|p| known.iter().any(|(id, _)| *id == p.panel))
            .cloned()
            .collect();
        for (id, dock) in known {
            if !out.iter().any(|p| p.panel == *id) {
                out.push(Place {
                    panel: id.clone(),
                    dock: *dock,
                    collapsed: false,
                    shown: true,
                });
            }
        }
        out
    }

    /// This layout with every panel of `known` in it, so later edits have
    /// a place to change.
    pub fn completed(&self, known: &[(String, Dock)]) -> Layout {
        let mut out = self.clone();
        let resolved = self.resolve(known);
        // Keep what the layout says about panels not in the window now.
        let absent: Vec<Place> = self
            .panels
            .iter()
            .filter(|p| !known.iter().any(|(id, _)| *id == p.panel))
            .cloned()
            .collect();
        out.panels = resolved;
        out.panels.extend(absent);
        out
    }

    pub fn place(&self, panel: &str) -> Option<&Place> {
        self.panels.iter().find(|p| p.panel == panel)
    }

    fn place_mut(&mut self, panel: &str) -> Option<&mut Place> {
        self.panels.iter_mut().find(|p| p.panel == panel)
    }

    /// The panels of `dock`, in order.
    pub fn in_dock(&self, dock: Dock) -> Vec<&Place> {
        self.panels.iter().filter(|p| p.dock == dock).collect()
    }

    /// Moves `panel` to the end of `dock`, showing it.
    pub fn move_to(&mut self, panel: &str, dock: Dock) {
        if let Some(i) = self.panels.iter().position(|p| p.panel == panel) {
            let mut place = self.panels.remove(i);
            place.dock = dock;
            place.shown = true;
            self.panels.push(place);
        }
    }

    /// Moves `panel` one step toward the start of its dock (`up`) or the
    /// end.
    pub fn shift(&mut self, panel: &str, up: bool) {
        let Some(i) = self.panels.iter().position(|p| p.panel == panel) else {
            return;
        };
        let dock = self.panels[i].dock;
        let same: Vec<usize> = (0..self.panels.len())
            .filter(|&k| self.panels[k].dock == dock)
            .collect();
        let at = same
            .iter()
            .position(|&k| k == i)
            .expect("the panel is in its dock");
        let other = if up {
            at.checked_sub(1)
        } else {
            Some(at + 1).filter(|&k| k < same.len())
        };
        if let Some(other) = other {
            self.panels.swap(i, same[other]);
        }
    }

    pub fn toggle_collapsed(&mut self, panel: &str) {
        if let Some(p) = self.place_mut(panel) {
            p.collapsed = !p.collapsed;
        }
    }

    pub fn set_shown(&mut self, panel: &str, shown: bool) {
        if let Some(p) = self.place_mut(panel) {
            p.shown = shown;
        }
    }

    /// Whether any panel of `dock` shows.
    pub fn dock_shows(&self, dock: Dock) -> bool {
        self.panels.iter().any(|p| p.dock == dock && p.shown)
    }
}

/// What `workspace.json` holds.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct Saved {
    /// The arrangement in use, with the name of the layout it came from.
    live: Option<Layout>,
    /// The user's own layouts.
    #[serde(default)]
    layouts: Vec<Layout>,
}

/// A panel the window has, as the markup put it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    pub id: String,
    pub dock: Dock,
    /// The text of its header's title.
    pub title: String,
}

/// The workspace in use and the user's layouts.
#[derive(Resource)]
pub struct Workspace {
    store: Store,
    /// The arrangement on screen; its name is the layout it started from.
    pub live: Layout,
    /// The user's own layouts.
    pub layouts: Vec<Layout>,
    /// The panels the window has, found when it opened.
    pub known: Vec<Known>,
    /// The panel the panel menu was opened for.
    menu_panel: Option<String>,
    /// The arrangement must be put on the window.
    dirty: bool,
    /// The docks' sizes at the last pointer release, to tell a gutter drag.
    measured: Sizes,
    /// What the panel list and layout list last showed.
    shown_lists: Option<(String, String)>,
}

impl Workspace {
    /// The workspace kept in `store`, or the Paint layout when it keeps
    /// none.
    pub fn load(store: Store) -> Self {
        let saved: Saved = store.load(FILE).unwrap_or_default();
        let live = saved
            .live
            .unwrap_or_else(|| Layout::built_in("Paint").expect("Paint is built in"));
        Self {
            store,
            live,
            layouts: saved.layouts,
            known: Vec::new(),
            menu_panel: None,
            dirty: true,
            measured: Sizes::default(),
            shown_lists: None,
        }
    }

    /// The workspace in pixl's data folder.
    pub fn user() -> Self {
        Self::load(Store::user())
    }

    pub fn in_memory() -> Self {
        Self::load(Store::in_memory())
    }

    fn known_docks(&self) -> Vec<(String, Dock)> {
        self.known.iter().map(|k| (k.id.clone(), k.dock)).collect()
    }

    /// The layout named `name`: the user's own, else a built-in one.
    pub fn layout(&self, name: &str) -> Option<Layout> {
        self.layouts
            .iter()
            .find(|l| l.name == name)
            .cloned()
            .or_else(|| Layout::built_in(name))
    }

    /// Every layout name: the built-in ones, then the user's.
    pub fn names(&self) -> Vec<String> {
        BUILT_IN
            .iter()
            .map(|s| s.to_string())
            .chain(
                self.layouts
                    .iter()
                    .map(|l| l.name.clone())
                    .filter(|n| !BUILT_IN.contains(&n.as_str())),
            )
            .collect()
    }

    /// Puts the layout named `name` on screen.
    pub fn switch(&mut self, name: &str) -> bool {
        let Some(layout) = self.layout(name) else {
            return false;
        };
        self.live = layout.completed(&self.known_docks());
        self.changed();
        true
    }

    /// Puts the live layout back the way its layout has it.
    pub fn reset(&mut self) {
        let name = self.live.name.clone();
        if !self.switch(&name) {
            self.switch("Paint");
        }
    }

    /// Saves the arrangement on screen as the user's layout `name`,
    /// replacing one of that name. A built-in name stays the built-in
    /// layout's.
    pub fn save_as(&mut self, name: &str) -> std::result::Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Name the layout first".to_string());
        }
        if BUILT_IN.contains(&name) {
            return Err(format!("{name} is a built-in layout; pick another name"));
        }
        let mut layout = self.live.clone();
        layout.name = name.to_string();
        match self.layouts.iter_mut().find(|l| l.name == name) {
            Some(l) => *l = layout,
            None => self.layouts.push(layout),
        }
        self.live.name = name.to_string();
        self.persist();
        Ok(())
    }

    /// Deletes the user's layout `name`.
    pub fn delete(&mut self, name: &str) -> bool {
        let before = self.layouts.len();
        self.layouts.retain(|l| l.name != name);
        let gone = self.layouts.len() != before;
        if gone {
            self.persist();
        }
        gone
    }

    /// Edits the live layout, then shows and keeps it.
    pub fn edit(&mut self, f: impl FnOnce(&mut Layout)) {
        f(&mut self.live);
        self.changed();
    }

    /// Shows every panel.
    pub fn show_all(&mut self) {
        self.edit(|l| {
            for p in &mut l.panels {
                p.shown = true;
            }
        });
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.persist();
    }

    fn persist(&self) {
        let saved = Saved {
            live: Some(self.live.clone()),
            layouts: self.layouts.clone(),
        };
        // A workspace that cannot be written still works for this session.
        let _ = self.store.save(FILE, &saved);
    }

    /// The panel's header title.
    pub fn title(&self, panel: &str) -> String {
        self.known
            .iter()
            .find(|k| k.id == panel)
            .map_or_else(|| panel.to_string(), |k| k.title.clone())
    }
}

/// Finds the window's panels: each `.panel` child of a dock, with its
/// header's title. Empty until the markup is in the window.
fn discover() -> Vec<Known> {
    let mut out = Vec::new();
    for dock in Dock::ALL {
        let Some(node) = dom::get_by_id(dock.element_id()) else {
            continue;
        };
        for child in node.children() {
            if !child.has_class("panel") {
                continue;
            }
            let Some(id) = child.id() else {
                continue;
            };
            let title = child
                .first_child()
                .and_then(|h| {
                    h.children()
                        .into_iter()
                        .find(|n| n.has_class("panel-title"))
                })
                .and_then(|t| t.text())
                .unwrap_or_else(|| id.clone());
            out.push(Known { id, dock, title });
        }
    }
    out
}

/// Gives a panel's header its collapse and close buttons.
fn add_header_tools(panel: &str, header: dom::Node) {
    let tools = dom::spawn("row");
    tools.set_class("panel-tools");
    header.append(tools);
    tools.set_inner_markup(&format!(
        r#"<button id="{panel}-collapse" class="panel-btn collapse" text="-"/><button id="{panel}-close" class="panel-btn close" text="x"/>"#
    ));
}

/// Whether the window shows `layout` (every panel of `known` in its dock,
/// in order, collapsed and shown as it says).
fn on_screen(places: &[Place]) -> bool {
    for dock in Dock::ALL {
        let Some(node) = dom::get_by_id(dock.element_id()) else {
            return false;
        };
        let here: Vec<String> = node
            .children()
            .into_iter()
            .filter(|c| c.has_class("panel"))
            .filter_map(|c| c.id())
            .collect();
        let want: Vec<&str> = places
            .iter()
            .filter(|p| p.dock == dock)
            .map(|p| p.panel.as_str())
            .collect();
        if here != want {
            return false;
        }
    }
    places.iter().all(|p| {
        dom::get_by_id(&p.panel).is_some_and(|n| {
            n.has_class("collapsed") == p.collapsed && n.has_class("closed") == !p.shown
        })
    })
}

/// Puts `places` and `sizes` on the window.
fn put_on_screen(places: &[Place], sizes: &Sizes) {
    for dock in Dock::ALL {
        let Some(node) = dom::get_by_id(dock.element_id()) else {
            continue;
        };
        let here: Vec<String> = node
            .children()
            .into_iter()
            .filter(|c| c.has_class("panel"))
            .filter_map(|c| c.id())
            .collect();
        let want: Vec<&Place> = places.iter().filter(|p| p.dock == dock).collect();
        if here
            .iter()
            .map(String::as_str)
            .ne(want.iter().map(|p| p.panel.as_str()))
        {
            // Appending each in turn leaves them in order.
            for p in &want {
                if let Some(panel) = dom::get_by_id(&p.panel) {
                    panel.move_to(node);
                }
            }
        }
        let shows = want.iter().any(|p| p.shown);
        if node.has_class("empty") == shows {
            if shows {
                node.remove_class("empty");
            } else {
                node.add_class("empty");
            }
        }
        match sizes.get(dock) {
            Some(px) => {
                node.set_style("flex-basis", &format!("{}", px.round()));
            }
            None => {
                node.remove_style("flex-basis");
            }
        }
    }
    for p in places {
        if let Some(panel) = dom::get_by_id(&p.panel) {
            if panel.has_class("collapsed") != p.collapsed {
                panel.toggle_class("collapsed");
            }
            if panel.has_class("closed") == p.shown {
                panel.toggle_class("closed");
            }
            if let Some(button) = dom::get_by_id(&format!("{}-collapse", p.panel)) {
                button.set_text(if p.collapsed { "+" } else { "-" });
            }
        }
    }
    // The gutter beside an empty side dock goes with it.
    let side = |dock: Dock| places.iter().any(|p| p.dock == dock && p.shown);
    if let Some(docks) = dom::get_by_id("docks") {
        let mut class = String::from("docks");
        if !side(Dock::Left) {
            class.push_str(" no-left");
        }
        if !side(Dock::Right) {
            class.push_str(" no-right");
        }
        let mut have = docks.classes();
        have.sort();
        let mut want: Vec<String> = class.split(' ').map(str::to_string).collect();
        want.sort();
        if have != want {
            docks.set_class(&class);
        }
    }
    if let Some(center) = dom::get_by_id("center") {
        let bottom = side(Dock::Bottom);
        if center.has_class("no-bottom") == bottom {
            center.toggle_class("no-bottom");
        }
    }
}

/// The panel an element belongs to, through its ancestors' ids.
fn panel_of(
    mut entity: Entity,
    ids: &Query<&LumenId>,
    parents: &Query<&ChildOf>,
    ws: &Workspace,
) -> Option<String> {
    for _ in 0..64 {
        if let Ok(id) = ids.get(entity)
            && ws.known.iter().any(|k| k.id == id.0)
        {
            return Some(id.0.clone());
        }
        entity = parents.get(entity).ok()?.parent();
    }
    None
}

/// The Window menu, the panel menu, the panel headers' buttons and the
/// Panels and layouts dialog.
#[allow(clippy::too_many_arguments)]
pub fn workspace_input(
    mut ws: ResMut<Workspace>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    mut context: MessageReader<ContextMenuEvent>,
    mut released: MessageReader<PointerReleased>,
    ids: Query<&LumenId>,
    parents: Query<&ChildOf>,
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
    chosen.extend(
        clicks
            .read()
            .filter_map(|c| ids.get(c.entity).ok().map(|id| id.0.clone())),
    );
    for request in context.read() {
        if let Some(panel) = panel_of(request.entity, &ids, &parents, &ws) {
            ws.menu_panel = Some(panel);
            signals.open_menu_at("panel-menu", request.position.x, request.position.y);
        }
    }
    if home.open {
        released.clear();
        return;
    }
    for id in chosen {
        run(&mut ws, &id, &mut signals);
    }
    // A gutter drag ends with a release: a dock whose size changed since
    // the last release was dragged, and keeps its new size.
    if released.read().count() > 0 && !ws.dirty {
        let mut sizes = ws.live.sizes;
        for dock in Dock::ALL {
            if !ws.live.dock_shows(dock) {
                continue;
            }
            let Some(rect) = dom::get_by_id(dock.element_id()).and_then(|n| n.rect()) else {
                continue;
            };
            let size = if dock == Dock::Bottom {
                rect.height
            } else {
                rect.width
            };
            if ws
                .measured
                .get(dock)
                .is_some_and(|m| (m - size).abs() >= 1.0)
            {
                sizes.set(dock, Some(size.round()));
            }
            ws.measured.set(dock, Some(size));
        }
        if sizes != ws.live.sizes {
            ws.live.sizes = sizes;
            ws.persist();
        }
    }
}

/// Carries out one workspace command by its element or menu item id.
pub fn run(ws: &mut Workspace, id: &str, signals: &mut Signals) {
    let menu_panel = ws.menu_panel.clone();
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
        "window-show-all" => ws.show_all(),
        "window-layout-reset" | "ws-reset" => ws.reset(),
        "ws-save" => {
            let name = signals.get::<String>("ws_name").unwrap_or_default();
            let note = match ws.save_as(&name) {
                Ok(()) => format!("Saved {}", name.trim()),
                Err(e) => e,
            };
            ws.shown_lists = None;
            signals.set("ws_message", note);
        }
        "ws-delete" => {
            let name = ws.live.name.clone();
            let note = if ws.delete(&name) {
                format!("Deleted {name}")
            } else {
                format!("{name} is built in")
            };
            ws.shown_lists = None;
            signals.set("ws_message", note);
        }
        "panel-menu-left" | "panel-menu-right" | "panel-menu-bottom" => {
            if let (Some(panel), Some(dock)) = (
                menu_panel,
                id.strip_prefix("panel-menu-").and_then(Dock::from_key),
            ) {
                ws.edit(|l| l.move_to(&panel, dock));
            }
        }
        "panel-menu-up" | "panel-menu-down" => {
            if let Some(panel) = menu_panel {
                let up = id.ends_with("up");
                ws.edit(|l| l.shift(&panel, up));
            }
        }
        "panel-menu-collapse" => {
            if let Some(panel) = menu_panel {
                ws.edit(|l| l.toggle_collapsed(&panel));
            }
        }
        "panel-menu-hide" => {
            if let Some(panel) = menu_panel {
                ws.edit(|l| l.set_shown(&panel, false));
            }
        }
        other => {
            if let Some(name) = other.strip_prefix("window-layout-") {
                let name = BUILT_IN
                    .iter()
                    .find(|b| b.eq_ignore_ascii_case(name))
                    .map_or_else(|| name.to_string(), |b| b.to_string());
                ws.switch(&name);
            } else if let Some(k) = other
                .strip_prefix("ws-layout-")
                .and_then(|k| k.parse::<usize>().ok())
            {
                if let Some(name) = ws.names().get(k).cloned() {
                    ws.switch(&name);
                    signals.set("ws_name", name);
                    ws.shown_lists = None;
                }
            } else if let Some(panel) = other.strip_suffix("-collapse") {
                let panel = panel.to_string();
                ws.edit(|l| l.toggle_collapsed(&panel));
            } else if let Some(panel) = other.strip_suffix("-close") {
                let panel = panel.to_string();
                ws.edit(|l| l.set_shown(&panel, false));
            } else if let Some(rest) = other.strip_prefix("ws-") {
                panel_row_command(ws, rest);
            }
        }
    }
}

/// A click in the dialog's panel list: `show-<panel>`, `dock-<dock>-<panel>`,
/// `up-<panel>` or `down-<panel>`.
fn panel_row_command(ws: &mut Workspace, rest: &str) {
    if let Some(panel) = rest.strip_prefix("show-") {
        let panel = panel.to_string();
        let shown = ws.live.place(&panel).is_none_or(|p| p.shown);
        ws.edit(|l| l.set_shown(&panel, !shown));
    } else if let Some(rest) = rest.strip_prefix("dock-") {
        if let Some((dock, panel)) = rest.split_once('-')
            && let Some(dock) = Dock::from_key(dock)
        {
            let panel = panel.to_string();
            ws.edit(|l| l.move_to(&panel, dock));
        }
    } else if let Some(panel) = rest.strip_prefix("up-") {
        let panel = panel.to_string();
        ws.edit(|l| l.shift(&panel, true));
    } else if let Some(panel) = rest.strip_prefix("down-") {
        let panel = panel.to_string();
        ws.edit(|l| l.shift(&panel, false));
    }
}

/// The dialog's panel list, one row per panel, dock by dock.
fn panel_list_markup(ws: &Workspace) -> String {
    let mut out = String::new();
    for dock in Dock::ALL {
        for place in ws.live.in_dock(dock) {
            if !ws.known.iter().any(|k| k.id == place.panel) {
                continue;
            }
            let p = &place.panel;
            let (state, label) = if place.shown {
                ("on", "Shown")
            } else {
                ("off", "Hidden")
            };
            out.push_str(&format!(
                r#"<row id="ws-row-{p}" class="ws-row"><label class="ws-panel-name" text="{title}"/><button id="ws-show-{p}" class="ws-chip {state}" text="{label}"/>"#,
                title = escape(&ws.title(p)),
            ));
            for d in Dock::ALL {
                let active = if d == dock { " active" } else { "" };
                out.push_str(&format!(
                    r#"<button id="ws-dock-{key}-{p}" class="ws-chip{active}" text="{short}"/>"#,
                    key = d.key(),
                    short = d.short(),
                ));
            }
            out.push_str(&format!(
                r#"<button id="ws-up-{p}" class="ws-chip" text="Up"/><button id="ws-down-{p}" class="ws-chip" text="Down"/></row>"#
            ));
        }
    }
    out
}

/// The dialog's layout list: the built-in layouts, then the user's.
fn layout_list_markup(ws: &Workspace) -> String {
    let mut out = String::new();
    for (k, name) in ws.names().iter().enumerate() {
        let current = if *name == ws.live.name {
            " current"
        } else {
            ""
        };
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

/// Finds the panels once the markup is in, puts the live layout on the
/// window whenever it changes (checking on later ticks that it took, since
/// DOM edits land a tick later), and keeps the dialog's lists current.
pub fn show_workspace(
    mut ws: ResMut<Workspace>,
    signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    if ws.known.is_empty() {
        let known = discover();
        if known.is_empty() {
            return;
        }
        for k in &known {
            if let Some(header) = dom::get_by_id(&k.id).and_then(|n| n.first_child()) {
                add_header_tools(&k.id, header);
            }
        }
        ws.known = known;
        let docks = ws.known_docks();
        ws.live = ws.live.completed(&docks);
        ws.dirty = true;
    }
    if ws.dirty {
        let places = ws.live.resolve(&ws.known_docks());
        if on_screen(&places) {
            ws.dirty = false;
            ws.measured = Sizes::default();
        } else {
            put_on_screen(&places, &ws.live.sizes);
            crate::repaint_soon(&wake);
        }
        ws.shown_lists = None;
    }
    if signals.get::<bool>("ws_open") == Some(true) {
        let lists = (panel_list_markup(&ws), layout_list_markup(&ws));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<(String, Dock)> {
        [
            "panel-color",
            "panel-palette",
            "panel-layers",
            "panel-history",
        ]
        .iter()
        .map(|p| (p.to_string(), Dock::Right))
        .collect()
    }

    fn order(l: &Layout, dock: Dock) -> Vec<&str> {
        l.in_dock(dock).iter().map(|p| p.panel.as_str()).collect()
    }

    #[test]
    fn a_panel_the_layout_does_not_name_keeps_its_default_place() {
        let mut k = known();
        k.push(("panel-new".to_string(), Dock::Bottom));
        let rig = Layout::built_in("Rig").unwrap().completed(&k);
        assert_eq!(order(&rig, Dock::Left), ["panel-color", "panel-palette"]);
        assert_eq!(order(&rig, Dock::Bottom), ["panel-new"]);
        assert!(rig.place("panel-new").unwrap().shown);
    }

    #[test]
    fn a_layout_drops_panels_the_window_lacks_but_remembers_them() {
        let mut l = Layout::built_in("Paint").unwrap();
        l.panels.push(Place {
            panel: "panel-gone".to_string(),
            dock: Dock::Left,
            collapsed: true,
            shown: true,
        });
        assert!(l.resolve(&known()).iter().all(|p| p.panel != "panel-gone"));
        assert!(l.completed(&known()).place("panel-gone").is_some());
    }

    #[test]
    fn moving_and_shifting_keep_each_dock_in_order() {
        let mut l = Layout::built_in("Paint").unwrap().completed(&known());
        l.move_to("panel-palette", Dock::Left);
        assert_eq!(order(&l, Dock::Left), ["panel-palette"]);
        assert_eq!(
            order(&l, Dock::Right),
            ["panel-color", "panel-layers", "panel-history"]
        );
        l.shift("panel-history", true);
        assert_eq!(
            order(&l, Dock::Right),
            ["panel-color", "panel-history", "panel-layers"]
        );
        // At the ends a shift does nothing.
        l.shift("panel-color", true);
        l.shift("panel-layers", false);
        assert_eq!(
            order(&l, Dock::Right),
            ["panel-color", "panel-history", "panel-layers"]
        );
    }

    #[test]
    fn moving_a_hidden_panel_shows_it() {
        let mut l = Layout::built_in("Animate").unwrap().completed(&known());
        assert!(!l.place("panel-history").unwrap().shown);
        assert!(!l.dock_shows(Dock::Bottom));
        l.move_to("panel-history", Dock::Bottom);
        assert!(l.place("panel-history").unwrap().shown);
        assert!(l.dock_shows(Dock::Bottom));
    }

    #[test]
    fn layouts_save_switch_reset_and_keep() {
        let dir = std::env::temp_dir().join(format!("pixl-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ws = Workspace::load(Store::at(&dir));
        ws.known = known()
            .into_iter()
            .map(|(id, dock)| Known {
                title: id.clone(),
                id,
                dock,
            })
            .collect();
        assert_eq!(ws.live.name, "Paint");
        assert!(ws.save_as("Paint").is_err(), "built-in names stay built in");
        ws.edit(|l| l.move_to("panel-color", Dock::Bottom));
        ws.live.sizes.right = Some(300.0);
        ws.save_as("Mine").unwrap();
        ws.switch("Rig");
        assert_eq!(ws.live.place("panel-color").unwrap().dock, Dock::Left);
        // The saved layout and the one in use survive a restart.
        let again = Workspace::load(Store::at(&dir));
        assert_eq!(again.live.name, "Rig");
        let mine = again.layout("Mine").unwrap();
        assert_eq!(mine.place("panel-color").unwrap().dock, Dock::Bottom);
        assert_eq!(mine.sizes.right, Some(300.0));
        assert_eq!(again.names(), ["Paint", "Animate", "Rig", "Mine"]);
        // Reset puts the live layout back to what its layout says.
        ws.edit(|l| l.set_shown("panel-layers", false));
        ws.reset();
        assert!(ws.live.place("panel-layers").unwrap().shown);
        assert!(ws.delete("Mine"));
        assert!(!ws.delete("Paint"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
