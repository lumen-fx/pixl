//! The tool bar's arrangement: which tools it shows and in what order, its
//! groups (tools that share one button, the one used last on top, the rest
//! a right-click or a long press away in a flyout), and one or two columns.
//! Window > Customize tool bar edits it; it is kept in `toolbar.json` in
//! pixl's data folder.
//!
//! The tools are the tool bar's buttons as the markup lays them out; that
//! order is the default, and a tool the arrangement does not name joins at
//! the end, so a new tool always shows.

use lumenui::ecs::hierarchy::ChildOf;
use lumenui::lumen_core::input::{ContextMenuEvent, LongPressEvent, MenuClicked};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use serde::{Deserialize, Serialize};

use crate::editor::{Editor, Tool};
use crate::home::Home;
use crate::keymap::KeyCommands;
use crate::panels::escape;
use crate::store::Store;

/// The file the arrangement is kept in.
pub const FILE: &str = "toolbar.json";

/// Tools that share one button.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    /// Button ids, in the flyout's order.
    pub tools: Vec<String>,
    /// The tool on the button.
    pub shown: String,
}

/// What the tool bar shows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Arrangement {
    /// Button ids in order; tools left out follow in the markup's order.
    pub order: Vec<String>,
    pub hidden: Vec<String>,
    pub groups: Vec<Group>,
    /// 1 or 2.
    pub columns: u8,
}

impl Default for Arrangement {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            hidden: Vec::new(),
            groups: Vec::new(),
            columns: 2,
        }
    }
}

/// The groups of the compact arrangement, by button id.
const COMPACT: &[(&str, &[&str])] = &[
    ("Brushes", &["tool-brush", "tool-spray", "tool-dither"]),
    (
        "Retouch",
        &["tool-smudge", "tool-blur", "tool-shade", "tool-replace"],
    ),
    ("Stamps", &["tool-clone", "tool-pattern"]),
    ("Lines", &["tool-line", "tool-curve"]),
    (
        "Shapes",
        &["tool-rect", "tool-ellipse", "tool-polygon", "tool-contour"],
    ),
    ("Fills", &["tool-bucket", "tool-gradient"]),
    (
        "Selections",
        &[
            "tool-select",
            "tool-ellipse-select",
            "tool-lasso",
            "tool-polylasso",
            "tool-wand",
        ],
    ),
    ("Transforms", &["tool-move", "tool-transform", "tool-crop"]),
    ("Guides", &["tool-measure", "tool-slice"]),
];

impl Arrangement {
    /// Every tool in its own button, as the markup lays them out.
    pub fn full() -> Self {
        Self::default()
    }

    /// Related tools grouped, one column.
    pub fn compact(known: &[String]) -> Self {
        let groups = COMPACT
            .iter()
            .map(|(name, tools)| {
                let tools: Vec<String> = tools
                    .iter()
                    .filter(|t| known.iter().any(|k| k == *t))
                    .map(|t| t.to_string())
                    .collect();
                Group {
                    name: name.to_string(),
                    shown: tools.first().cloned().unwrap_or_default(),
                    tools,
                }
            })
            .filter(|g| g.tools.len() > 1)
            .collect();
        Self {
            groups,
            columns: 1,
            ..Self::default()
        }
    }

    /// Every known tool in order: the arrangement's, then the rest.
    pub fn resolved_order(&self, known: &[String]) -> Vec<String> {
        let mut out: Vec<String> = self
            .order
            .iter()
            .filter(|t| known.contains(t))
            .cloned()
            .collect();
        for t in known {
            if !out.contains(t) {
                out.push(t.clone());
            }
        }
        out
    }

    pub fn group_of(&self, tool: &str) -> Option<usize> {
        self.groups
            .iter()
            .position(|g| g.tools.iter().any(|t| t == tool))
    }

    /// Whether `tool` has a button of its own on the bar.
    pub fn shows(&self, tool: &str) -> bool {
        if self.hidden.iter().any(|t| t == tool) {
            return false;
        }
        match self.group_of(tool) {
            Some(g) => {
                let group = &self.groups[g];
                // The shown member, or the first one that is not hidden.
                let top = group
                    .tools
                    .iter()
                    .find(|t| **t == group.shown && !self.hidden.contains(t))
                    .or_else(|| group.tools.iter().find(|t| !self.hidden.contains(t)));
                top.is_some_and(|t| t == tool)
            }
            None => true,
        }
    }

    /// Moves `tool` one place up or down the bar.
    pub fn shift(&mut self, tool: &str, up: bool, known: &[String]) {
        let mut order = self.resolved_order(known);
        let Some(i) = order.iter().position(|t| t == tool) else {
            return;
        };
        let j = if up {
            i.checked_sub(1)
        } else {
            Some(i + 1).filter(|&j| j < order.len())
        };
        if let Some(j) = j {
            order.swap(i, j);
        }
        self.order = order;
    }

    pub fn toggle_hidden(&mut self, tool: &str) {
        if let Some(i) = self.hidden.iter().position(|t| t == tool) {
            self.hidden.remove(i);
        } else {
            self.hidden.push(tool.to_string());
        }
    }

    /// Puts `tool` in the next group (none, then each group in turn).
    pub fn cycle_group(&mut self, tool: &str) {
        let next = match self.group_of(tool) {
            None => (!self.groups.is_empty()).then_some(0),
            Some(g) => (g + 1 < self.groups.len()).then_some(g + 1),
        };
        if let Some(g) = self.group_of(tool) {
            let group = &mut self.groups[g];
            group.tools.retain(|t| t != tool);
            if group.shown == tool {
                group.shown = group.tools.first().cloned().unwrap_or_default();
            }
        }
        if let Some(n) = next {
            let group = &mut self.groups[n];
            group.tools.push(tool.to_string());
            if group.shown.is_empty() {
                group.shown = tool.to_string();
            }
        }
    }

    /// Makes `tool` the one its group's button shows.
    pub fn bring_up(&mut self, tool: &str) -> bool {
        match self.group_of(tool) {
            Some(g) if self.groups[g].shown != tool => {
                self.groups[g].shown = tool.to_string();
                true
            }
            _ => false,
        }
    }
}

/// A tool bar button as the markup made it.
#[derive(Clone, Debug)]
struct Known {
    id: String,
    /// The icon tile's class, such as `icon-pencil`.
    icon: String,
}

/// The arrangement, where it is kept, and the flyout.
#[derive(Resource)]
pub struct Toolbar {
    store: Store,
    pub arrangement: Arrangement,
    known: Vec<Known>,
    dirty: bool,
    /// The group whose flyout is open, and where.
    flyout: Option<(usize, f32, f32)>,
    flyout_shown: Option<String>,
    list_shown: Option<String>,
    seen_tool: Option<Tool>,
}

impl Toolbar {
    pub fn load(store: Store) -> Self {
        let arrangement = store.load(FILE).unwrap_or_default();
        Self {
            store,
            arrangement,
            known: Vec::new(),
            dirty: true,
            flyout: None,
            flyout_shown: None,
            list_shown: None,
            seen_tool: None,
        }
    }

    pub fn user() -> Self {
        Self::load(Store::user())
    }

    pub fn in_memory() -> Self {
        Self::load(Store::in_memory())
    }

    fn known_ids(&self) -> Vec<String> {
        self.known.iter().map(|k| k.id.clone()).collect()
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.list_shown = None;
        let _ = self.store.save(FILE, &self.arrangement);
    }

    fn icon(&self, tool: &str) -> String {
        self.known
            .iter()
            .find(|k| k.id == tool)
            .map_or_else(String::new, |k| k.icon.clone())
    }
}

/// The tool a pressed element belongs to: the button or its icon.
fn tool_button(
    mut entity: Entity,
    ids: &Query<&LumenId>,
    parents: &Query<&ChildOf>,
) -> Option<String> {
    for _ in 0..3 {
        if let Ok(id) = ids.get(entity)
            && Tool::from_button_id(&id.0).is_some()
        {
            return Some(id.0.clone());
        }
        entity = parents.get(entity).ok()?.parent();
    }
    None
}

/// The dialog, the flyout, and the menu item.
#[allow(clippy::too_many_arguments)]
pub fn toolbar_input(
    mut bar: ResMut<Toolbar>,
    mut editor: ResMut<Editor>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    mut context: MessageReader<ContextMenuEvent>,
    mut long: MessageReader<LongPressEvent>,
    ids: Query<&LumenId>,
    parents: Query<&ChildOf>,
    (home, keys): (Res<Home>, Res<KeyCommands>),
    mut signals: Signals,
) {
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    if keys.ran("window-toolbar", false) {
        chosen.push("window-toolbar".into());
    }
    let clicked: Vec<String> = clicks
        .read()
        .filter_map(|c| ids.get(c.entity).ok().map(|id| id.0.clone()))
        .collect();
    let mut asked: Vec<(Entity, Option<(f32, f32)>)> = context
        .read()
        .map(|c| (c.entity, Some((c.position.x, c.position.y))))
        .collect();
    asked.extend(long.read().map(|l| (l.entity, None)));
    if home.open {
        return;
    }
    // A right-click or long press on a grouped button opens its flyout.
    for (entity, at) in asked {
        let Some(tool) = tool_button(entity, &ids, &parents) else {
            continue;
        };
        if let Some(g) = bar.arrangement.group_of(&tool) {
            let (x, y) = at.unwrap_or_else(|| {
                dom::get_by_id(&tool)
                    .and_then(|n| n.rect())
                    .map_or((40.0, 60.0), |r| (r.x + r.width, r.y))
            });
            bar.flyout = Some((g, x, y));
        }
    }
    let flyout_open = bar.flyout.is_some();
    for id in clicked.iter().chain(chosen.iter()) {
        match id.as_str() {
            "window-toolbar" => {
                bar.list_shown = None;
                signals.set("tb_open", true);
            }
            "tb-close" => signals.set("tb_open", false),
            "tb-full" => {
                bar.arrangement = Arrangement::full();
                bar.changed();
            }
            "tb-compact" => {
                let known = bar.known_ids();
                bar.arrangement = Arrangement::compact(&known);
                bar.changed();
            }
            "tb-columns" => {
                bar.arrangement.columns = if bar.arrangement.columns == 1 { 2 } else { 1 };
                bar.changed();
            }
            other => {
                let known = bar.known_ids();
                if let Some(t) = other.strip_prefix("tb-show-") {
                    bar.arrangement.toggle_hidden(t);
                    bar.changed();
                } else if let Some(t) = other.strip_prefix("tb-up-") {
                    bar.arrangement.shift(t, true, &known);
                    bar.changed();
                } else if let Some(t) = other.strip_prefix("tb-down-") {
                    bar.arrangement.shift(t, false, &known);
                    bar.changed();
                } else if let Some(t) = other.strip_prefix("tb-group-") {
                    bar.arrangement.cycle_group(t);
                    bar.changed();
                } else if let Some(t) = other.strip_prefix("flyout-") {
                    if let Some(tool) = Tool::from_button_id(t) {
                        editor.tool = tool;
                    }
                    bar.flyout = None;
                }
            }
        }
    }
    // Any other click closes an open flyout.
    if flyout_open && !clicked.is_empty() {
        bar.flyout = None;
    }
}

/// Finds the buttons once, keeps the bar as arranged, keeps the active
/// tool's group showing it, and draws the flyout and the dialog's list.
pub fn show_toolbar(
    mut bar: ResMut<Toolbar>,
    editor: Res<Editor>,
    signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    if bar.known.is_empty() {
        let Some(node) = dom::get_by_id("toolbar") else {
            return;
        };
        let known: Vec<Known> = node
            .children()
            .into_iter()
            .filter_map(|b| {
                let id = b.id()?;
                Tool::from_button_id(&id)?;
                let icon = b
                    .first_child()
                    .and_then(|t| t.classes().into_iter().find(|c| c.starts_with("icon-")))
                    .unwrap_or_default();
                Some(Known { id, icon })
            })
            .collect();
        if known.is_empty() {
            return;
        }
        bar.known = known;
        bar.dirty = true;
    }
    // The tool in use shows on its group's button.
    if bar.seen_tool != Some(editor.tool) {
        bar.seen_tool = Some(editor.tool);
        if bar.arrangement.bring_up(editor.tool.button_id()) {
            bar.changed();
        }
    }
    if bar.dirty {
        let known = bar.known_ids();
        let order = bar.arrangement.resolved_order(&known);
        if arranged(&bar.arrangement, &order) {
            bar.dirty = false;
        } else {
            arrange(&bar.arrangement, &order);
            crate::repaint_soon(&wake);
        }
    }
    // The flyout.
    let flyout = bar.flyout.map(|(g, x, y)| {
        let group = &bar.arrangement.groups[g.min(bar.arrangement.groups.len().saturating_sub(1))];
        let mut markup = String::new();
        for t in &group.tools {
            let label = Tool::from_button_id(t).map_or("", |t| t.label());
            markup.push_str(&format!(
                r#"<tooltip text="{label}"><button id="flyout-{t}" class="tool flyout-tool"><tile class="tool-icon {}"/></button></tooltip>"#,
                bar.icon(t)
            ));
        }
        (markup, x, y)
    });
    let key = flyout
        .as_ref()
        .map(|(m, x, y)| format!("{x} {y} {m}"))
        .unwrap_or_default();
    if bar.flyout_shown.as_deref() != Some(key.as_str())
        && let Some(node) = dom::get_by_id("tool-flyout")
    {
        match &flyout {
            Some((markup, x, y)) => {
                node.set_inner_markup(markup);
                node.set_style("inset", &format!("{} auto auto {}", y.round(), x.round()));
                node.add_class("open");
            }
            None => {
                node.remove_class("open");
            }
        }
        bar.flyout_shown = Some(key);
        crate::repaint_soon(&wake);
    }
    if signals.get::<bool>("tb_open") == Some(true) {
        let markup = list_markup(&bar);
        if bar.list_shown.as_deref() != Some(markup.as_str())
            && let Some(list) = dom::get_by_id("tb-list")
        {
            list.set_inner_markup(&markup);
            bar.list_shown = Some(markup);
            crate::repaint_soon(&wake);
        }
    }
}

/// Whether the bar shows `a`: its buttons in `order`, each shown or not,
/// grouped buttons marked, and its column count.
fn arranged(a: &Arrangement, order: &[String]) -> bool {
    let Some(node) = dom::get_by_id("toolbar") else {
        return true;
    };
    let here: Vec<String> = node.children().into_iter().filter_map(|c| c.id()).collect();
    if here != order {
        return false;
    }
    if node.has_class("one-column") != (a.columns == 1) {
        return false;
    }
    order.iter().all(|t| {
        dom::get_by_id(t).is_some_and(|b| {
            b.has_class("tool-off") == !a.shows(t)
                && b.has_class("grouped") == a.group_of(t).is_some()
        })
    })
}

fn arrange(a: &Arrangement, order: &[String]) {
    let Some(node) = dom::get_by_id("toolbar") else {
        return;
    };
    let here: Vec<String> = node.children().into_iter().filter_map(|c| c.id()).collect();
    if here != order {
        for t in order {
            if let Some(b) = dom::get_by_id(t) {
                b.move_to(node);
            }
        }
    }
    if node.has_class("one-column") != (a.columns == 1) {
        node.toggle_class("one-column");
    }
    for t in order {
        if let Some(b) = dom::get_by_id(t) {
            if b.has_class("tool-off") == a.shows(t) {
                b.toggle_class("tool-off");
            }
            if b.has_class("grouped") != a.group_of(t).is_some() {
                b.toggle_class("grouped");
            }
        }
    }
}

/// The dialog's rows: each tool in order, with its controls.
fn list_markup(bar: &Toolbar) -> String {
    let known = bar.known_ids();
    let a = &bar.arrangement;
    let mut out = String::new();
    for t in a.resolved_order(&known) {
        let label = Tool::from_button_id(&t).map_or("", |t| t.label());
        let hidden = a.hidden.contains(&t);
        let (state, show) = if hidden {
            ("off", "Hidden")
        } else {
            ("on", "Shown")
        };
        let group = a
            .group_of(&t)
            .map_or_else(|| "No group".to_string(), |g| a.groups[g].name.clone());
        out.push_str(&format!(
            r#"<row class="tb-row"><tile class="tb-icon tool-icon {icon}"/><label class="tb-name" text="{label}"/><button id="tb-show-{t}" class="ws-chip {state}" text="{show}"/><button id="tb-group-{t}" class="ws-chip tb-group" text="{group}"/><button id="tb-up-{t}" class="ws-chip" text="Up"/><button id="tb-down-{t}" class="ws-chip" text="Down"/></row>"#,
            icon = bar.icon(&t),
            group = escape(&group),
        ));
    }
    out
}

/// Adds the tool bar's resources and systems.
pub fn install(app: App, bar: Toolbar) -> App {
    use lumenui::lumen_core::input::ClickSet;
    use lumenui::lumen_core::signals;
    use lumenui_canvas::CanvasSet;
    app.insert_resource(bar)
        .insert_signal("tb_open", false)
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("toolbar::toolbar_input", toolbar_input)
                .after(crate::tools::pick_tool)
                .before(crate::tools::use_tools)
                .after(ClickSet::Producers),
        )
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("toolbar::show_toolbar", show_toolbar)
                .after(crate::tools::show_tool)
                .before(signals::apply_text_bindings)
                .before(CanvasSet::Encode),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        Tool::ALL
            .iter()
            .map(|t| t.button_id().to_string())
            .collect()
    }

    #[test]
    fn the_full_bar_shows_every_tool_in_the_markup_order() {
        let a = Arrangement::full();
        assert_eq!(a.resolved_order(&known()), known());
        assert!(known().iter().all(|t| a.shows(t)));
    }

    #[test]
    fn a_group_shows_one_button_and_the_tool_used_comes_up() {
        let mut a = Arrangement::compact(&known());
        assert!(a.shows("tool-rect"));
        assert!(!a.shows("tool-ellipse"));
        assert!(a.shows("tool-pencil"), "an ungrouped tool shows");
        assert!(a.bring_up("tool-ellipse"));
        assert!(a.shows("tool-ellipse") && !a.shows("tool-rect"));
        assert!(!a.bring_up("tool-pencil"));
    }

    #[test]
    fn hiding_the_shown_member_shows_the_next() {
        let mut a = Arrangement::compact(&known());
        a.toggle_hidden("tool-rect");
        assert!(!a.shows("tool-rect"));
        assert!(a.shows("tool-ellipse"));
    }

    #[test]
    fn shifting_and_regrouping() {
        let k = known();
        let mut a = Arrangement::full();
        a.shift("tool-eraser", true, &k);
        assert_eq!(
            &a.resolved_order(&k)[..3],
            ["tool-pencil", "tool-eraser", "tool-brush"]
        );
        let mut c = Arrangement::compact(&k);
        let groups = c.groups.len();
        c.cycle_group("tool-pencil");
        assert_eq!(c.group_of("tool-pencil"), Some(0));
        for _ in 0..groups {
            c.cycle_group("tool-pencil");
        }
        assert_eq!(
            c.group_of("tool-pencil"),
            None,
            "past the last group is none"
        );
    }

    #[test]
    fn a_new_tool_joins_at_the_end() {
        let mut a = Arrangement::full();
        a.order = vec!["tool-eraser".into()];
        let order = a.resolved_order(&known());
        assert_eq!(order[0], "tool-eraser");
        assert_eq!(order.len(), known().len());
    }
}
