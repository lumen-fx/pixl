//! The documents open in the window, one tab each above the canvas.
//!
//! The document in front lives in the editor's resources: the [`Editor`],
//! the [`View`], the layer buffers, the timeline, the Rig panel and the rest,
//! so every system works on that one document, and a document behind
//! another one's tab costs nothing per frame. When a tab goes behind, it
//! takes its document's part of each resource into its [`Parked`] state;
//! when it comes back to the front, it puts that part back. What belongs to
//! the window stays where it is: the settings, the palettes library, the
//! brushes, the layouts, and the colors and tool.
//!
//! A document has a [`DocId`] for as long as it is open. Tabs, the command
//! palette and file dialogs name documents by it, or by the generation of
//! their editor, and reach a document behind another one's tab through
//! [`edit_document`].

use std::path::{Path, PathBuf};

use lumenui::ecs::message::Messages;
use lumenui::ecs::system::SystemParam;
use lumenui::ecs::world::World;
use lumenui::lumen_core::input::{
    CloseRequest, ContextMenuEvent, MenuClicked, PointerButton, PointerMoved, PointerPressed,
    PointerReleased,
};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;

use crate::editor::Editor;
use crate::files::{FileDocument, Files, Opening};
use crate::home::Home;
use crate::ids::ElementIds;
use crate::menus::palette::{Entry, Run};
use crate::menus::select::Selections;
use crate::panels::escape;
use crate::panels::preview::PreviewPane;
use crate::panels::timeline::{LayerTree, Timeline};
use crate::rig::RigUi;
use crate::settings::Settings;
use crate::settings::keymap::KeyCommands;
use crate::tools::toolbox::{Toolbox, ToolboxDocument};
use crate::view::aids::Aids;
use crate::view::{Buffers, Layers, Preview, View};

/// Names an open document for as long as it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub u64);

/// What a document keeps while another one is in front: its part of each
/// resource the editor works through.
pub struct Parked {
    editor: Editor,
    file: FileDocument,
    view: View,
    buffers: Buffers,
    timeline: Timeline,
    tree: LayerTree,
    preview: PreviewPane,
    rig: RigUi,
    selections: Selections,
    palette_lock: bool,
    guides: (Vec<i32>, Vec<i32>),
    tools: ToolboxDocument,
}

impl Parked {
    /// The document's editor.
    pub fn editor(&self) -> &Editor {
        &self.editor
    }
}

/// One tab: its document, parked unless it is the one in front.
struct Tab {
    id: DocId,
    parked: Option<Box<Parked>>,
    /// When the tab was last in front, for the buffer budget to let go of
    /// the buffers of the tabs left longest ago first.
    used: u64,
}

/// What a tab bar click, a key, a menu or a file request asks of the tabs.
/// They are carried out in order by [`apply_tabs`], which reaches every
/// resource a document keeps.
pub enum TabAction {
    /// A new document in a tab of its own, in front.
    Open(Box<Opening>),
    /// Brings a tab to the front, closing the start screen.
    Switch(DocId),
    /// Brings the next tab (1) or the one before (-1) to the front.
    Step(i32),
    /// The document in front takes the recovery copy of its file.
    Recover(PathBuf),
    /// Closes tabs, asking first when any has unsaved edits.
    Close(Vec<DocId>),
    /// Closes tabs, edits and all.
    Discard(Vec<DocId>),
    /// Moves a tab to a place in the row.
    Move(DocId, usize),
}

/// What the discard question is about.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ask {
    Close(Vec<DocId>),
    Quit,
}

/// What the tab bar shows of the window.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    changed: u64,
    home: bool,
    generation: u64,
    modified: bool,
    path: Option<PathBuf>,
    name: Option<String>,
}

impl Shown {
    fn of(docs: &Documents, home: &Home, front: &Editor) -> Self {
        Self {
            changed: docs.changed,
            home: home.open,
            generation: front.generation,
            modified: front.is_modified(),
            path: front.path.clone(),
            name: front.name.clone(),
        }
    }

    /// Whether the bar shows what `of` would read now, read without
    /// copying anything.
    fn is(&self, docs: &Documents, home: &Home, front: &Editor) -> bool {
        self.changed == docs.changed
            && self.home == home.open
            && self.generation == front.generation
            && self.modified == front.is_modified()
            && self.path == front.path
            && self.name == front.name
    }
}

/// A tab being dragged along the row.
struct Drag {
    id: DocId,
    from: f32,
    moved: bool,
}

/// The documents open in the window.
#[derive(Resource)]
pub struct Documents {
    tabs: Vec<Tab>,
    /// The tab whose document is in the editor's resources. `None` while
    /// no document is open: the start screen shows, over a blank editor.
    front: Option<usize>,
    next_id: u64,
    clock: u64,
    actions: Vec<TabAction>,
    asking: Option<Ask>,
    /// Bumped whenever the tabs change: opened, closed, moved, brought to
    /// the front, or a document behind them edited.
    changed: u64,
    /// What the tab bar was last written for: the tabs' change count, the
    /// start screen, and the front document's generation, unsaved edits,
    /// file and name.
    shown: Option<Shown>,
    drag: Option<Drag>,
    /// The tab a context menu was opened on.
    menu_on: Option<DocId>,
}

impl Default for Documents {
    /// One tab, in front: the document the editor starts with.
    fn default() -> Self {
        Self {
            tabs: vec![Tab {
                id: DocId(1),
                parked: None,
                used: 0,
            }],
            front: Some(0),
            next_id: 2,
            clock: 0,
            actions: Vec::new(),
            asking: None,
            changed: 0,
            shown: None,
            drag: None,
            menu_on: None,
        }
    }
}

impl Documents {
    /// No document open: pixl on its start screen.
    pub fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            front: None,
            ..Self::default()
        }
    }

    /// How many documents are open.
    pub fn count(&self) -> usize {
        self.tabs.len()
    }

    /// The document in front, if one is open.
    pub fn front(&self) -> Option<DocId> {
        self.front.map(|i| self.tabs[i].id)
    }

    /// The open documents in the order of their tabs.
    pub fn ids(&self) -> Vec<DocId> {
        self.tabs.iter().map(|t| t.id).collect()
    }

    /// Asks the tabs for something, done later this tick.
    pub fn act(&mut self, action: TabAction) {
        self.actions.push(action);
    }

    /// Actions waiting for their turn.
    pub fn pending(&self) -> usize {
        self.actions.len()
    }

    /// Each open document with its editor, in tab order: `front` (the
    /// editor resource) for the one in front.
    pub fn editors<'a>(&'a self, front: &'a Editor) -> impl Iterator<Item = (DocId, &'a Editor)> {
        self.tabs.iter().enumerate().map(move |(i, t)| {
            let editor = match (&t.parked, self.front == Some(i)) {
                (_, true) | (None, _) => front,
                (Some(p), false) => &p.editor,
            };
            (t.id, editor)
        })
    }

    /// The open document that shows the file at `path`, if one does.
    pub fn showing(&self, front: &Editor, path: &Path) -> Option<DocId> {
        let wanted = std::fs::canonicalize(path).ok();
        self.editors(front)
            .find(|(_, e)| {
                e.recent_path().is_some_and(|p| {
                    p == path || (wanted.is_some() && std::fs::canonicalize(p).ok() == wanted)
                })
            })
            .map(|(id, _)| id)
    }

    /// The layer buffer memory the documents behind other tabs keep, in
    /// bytes.
    pub fn behind_bytes(&self) -> u64 {
        self.tabs
            .iter()
            .filter_map(|t| t.parked.as_ref())
            .map(|p| p.buffers.bytes())
            .sum()
    }

    fn index(&self, id: DocId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// The documents with unsaved edits among `ids`, by name.
    fn unsaved(&self, front: &Editor, ids: &[DocId]) -> Vec<String> {
        self.editors(front)
            .filter(|(id, e)| ids.contains(id) && e.is_modified())
            .map(|(_, e)| e.name())
            .collect()
    }
}

/// The part of the window each document keeps, taken out of the editor's
/// resources, which are left as a newly opened document's. The document's
/// recovery copy is written first when it has edits the copy lacks, since
/// autosave only looks at the document in front.
fn take_front(world: &mut World) -> Parked {
    let editor = {
        let mut front = world.resource_mut::<Editor>();
        let mut blank = Editor::blank();
        blank.take_window_state(&front);
        std::mem::replace(&mut *front, blank)
    };
    let mut file = std::mem::take(&mut world.resource_mut::<Files>().front);
    file.write_now(&editor, world.resource::<Settings>());
    let parked = Parked {
        editor,
        file,
        view: world.resource_mut::<View>().take_document(),
        buffers: world.resource_mut::<Layers>().take_buffers(),
        timeline: world.resource_mut::<Timeline>().take_document(),
        tree: std::mem::take(&mut *world.resource_mut::<LayerTree>()),
        preview: world.resource_mut::<PreviewPane>().take_document(),
        rig: world.resource_mut::<RigUi>().take_document(),
        selections: world.resource_mut::<Selections>().take_document(),
        palette_lock: std::mem::take(
            &mut world.resource_mut::<crate::menus::image::ImageMenu>().lock,
        ),
        guides: world.resource_mut::<Aids>().take_guides(),
        tools: world.resource_mut::<Toolbox>().take_document(),
    };
    forget_front(world);
    parked
}

/// Puts a document's part of the window back into the editor's resources.
fn restore_front(world: &mut World, parked: Parked) {
    {
        let mut front = world.resource_mut::<Editor>();
        let mut editor = parked.editor;
        editor.take_window_state(&front);
        *front = editor;
    }
    world.resource_mut::<Files>().front = parked.file;
    world.resource_mut::<View>().restore_document(parked.view);
    world
        .resource_mut::<Layers>()
        .restore_buffers(parked.buffers);
    world
        .resource_mut::<Timeline>()
        .restore_document(parked.timeline);
    *world.resource_mut::<LayerTree>() = parked.tree;
    world
        .resource_mut::<PreviewPane>()
        .restore_document(parked.preview);
    world.resource_mut::<RigUi>().restore_document(parked.rig);
    world
        .resource_mut::<Selections>()
        .restore_document(parked.selections);
    world.resource_mut::<crate::menus::image::ImageMenu>().lock = parked.palette_lock;
    world.resource_mut::<Aids>().restore_guides(parked.guides);
    world
        .resource_mut::<Toolbox>()
        .restore_document(parked.tools);
    forget_front(world);
}

/// Drops what the tools drew over the document that was in front: the
/// shape preview and the reference box. The stage and the panels see the
/// new document's generation and bring themselves up to date with it.
fn forget_front(world: &mut World) {
    world.resource_mut::<Preview>().clear();
    *world.resource_mut::<crate::tools::reference::ReferenceBox>() =
        crate::tools::reference::ReferenceBox::default();
}

/// Why the document in front cannot change now, if it cannot: something
/// is under way on it that has to be finished or dropped first.
fn busy(world: &World) -> Option<&'static str> {
    if world.resource::<crate::tools::Gesture>().active() {
        return Some("Finish the stroke first");
    }
    if world.resource::<Toolbox>().active() {
        return Some("Finish the shape first, or drop it with Escape");
    }
    if world
        .resource::<crate::tools::transform::FreeTransform>()
        .active()
    {
        return Some("Apply the free transform first, or cancel it with Escape");
    }
    if world.resource::<crate::menus::adjust::Adjust>().active() {
        return Some("Apply or cancel the filter first");
    }
    if world.resource::<crate::menus::image::ImageMenu>().busy() {
        return Some("Wait for the image edit to finish");
    }
    None
}

/// Moves the document in front behind its tab.
fn park_front(world: &mut World) {
    let Some(i) = world.resource::<Documents>().front else {
        return;
    };
    let parked = take_front(world);
    let mut docs = world.resource_mut::<Documents>();
    docs.tabs[i].parked = Some(Box::new(parked));
    docs.front = None;
}

/// Brings tab `i` to the front, closing the start screen.
fn bring(world: &mut World, i: usize) {
    let parked = {
        let mut docs = world.resource_mut::<Documents>();
        docs.clock += 1;
        let clock = docs.clock;
        let tab = &mut docs.tabs[i];
        tab.used = clock;
        tab.parked.take()
    };
    if let Some(parked) = parked {
        restore_front(world, *parked);
    }
    world.resource_mut::<Documents>().front = Some(i);
    world.resource_mut::<Home>().open = false;
    keep_to_budget(world);
}

/// A new document in a tab of its own, at the end of the row, in front.
fn install(world: &mut World, opening: Opening) {
    park_front(world);
    {
        let mut front = world.resource_mut::<Editor>();
        let mut editor = opening.editor;
        editor.take_window_state(&front);
        *front = editor;
    }
    world.resource_mut::<Files>().front = opening.file;
    forget_front(world);
    let i = {
        let mut docs = world.resource_mut::<Documents>();
        let id = DocId(docs.next_id);
        docs.next_id += 1;
        docs.tabs.push(Tab {
            id,
            parked: None,
            used: 0,
        });
        docs.tabs.len() - 1
    };
    bring(world, i);
}

/// Keeps the layer buffers of every open document within the buffer
/// budget of the performance settings: the document in front has what its
/// plan needs, and the documents behind keep theirs while the total fits,
/// so coming back to one draws at once. Past the budget, the buffers of the
/// tabs left longest ago go first; such a document draws anew when it comes
/// back.
fn keep_to_budget(world: &mut World) {
    let (budget, front) = {
        let layers = world.resource::<Layers>();
        let editor = world.resource::<Editor>();
        let doc = &editor.doc;
        // What the front's plan will hold once it has synced.
        let planned = crate::view::plan_with_onion(doc, editor.frame, layers.budget, None)
            .iter()
            .filter(|p| p.reference.is_none())
            .count() as u64
            * u64::from(doc.width())
            * u64::from(doc.height())
            * 4;
        (layers.budget_bytes(), planned.max(layers.buffer_stats().1))
    };
    let mut docs = world.resource_mut::<Documents>();
    let mut behind: Vec<(u64, usize, u64)> = docs
        .tabs
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.parked.as_ref().map(|p| (t.used, i, p.buffers.bytes())))
        .filter(|(_, _, bytes)| *bytes > 0)
        .collect();
    behind.sort();
    let mut total = front + behind.iter().map(|b| b.2).sum::<u64>();
    for (_, i, bytes) in behind {
        if total <= budget {
            break;
        }
        if let Some(p) = docs.tabs[i].parked.as_mut() {
            p.buffers = Buffers::none();
            total -= bytes;
        }
    }
}

/// Closes tabs, edits and all. When the document in front goes, the tab
/// after it comes to the front, or the one before; with none left the
/// start screen shows.
fn discard(world: &mut World, ids: &[DocId]) {
    let front = world.resource::<Documents>().front();
    if front.is_some_and(|f| ids.contains(&f)) {
        if let Some(why) = busy(world) {
            world.resource_mut::<Editor>().message = Some(why.to_string());
            return;
        }
        drop(take_front(world));
        let mut docs = world.resource_mut::<Documents>();
        docs.front = None;
    }
    let next = {
        let mut docs = world.resource_mut::<Documents>();
        let front_id = docs.front();
        // The tab to bring forward if the front one closes: the first one
        // after it that stays, else the last one before it.
        let at = front.and_then(|f| docs.index(f)).unwrap_or(docs.tabs.len());
        let after = docs.tabs[at.min(docs.tabs.len())..]
            .iter()
            .find(|t| !ids.contains(&t.id))
            .map(|t| t.id);
        let before = docs.tabs[..at.min(docs.tabs.len())]
            .iter()
            .rev()
            .find(|t| !ids.contains(&t.id))
            .map(|t| t.id);
        docs.tabs.retain(|t| !ids.contains(&t.id));
        docs.front = front_id.and_then(|f| docs.index(f));
        if docs.front.is_none() {
            after.or(before)
        } else {
            None
        }
    };
    let docs = world.resource::<Documents>();
    let next = next.and_then(|id| docs.index(id));
    let none_left = docs.front.is_none();
    match next {
        Some(i) => bring(world, i),
        None if none_left => world.resource_mut::<Home>().show(false),
        None => {}
    }
}

/// Carries out the tabs' actions, in order. An exclusive system: bringing
/// a document to the front reaches every resource it keeps.
pub fn apply_tabs(world: &mut World) {
    let actions = {
        let mut docs = world.resource_mut::<Documents>();
        if docs.actions.is_empty() {
            return;
        }
        docs.changed += 1;
        std::mem::take(&mut docs.actions)
    };
    for action in actions {
        match action {
            TabAction::Open(opening) => {
                if let Some(why) = busy(world) {
                    world.resource_mut::<Editor>().message = Some(why.to_string());
                    // Nor is its recovery copy on offer.
                    world
                        .resource_mut::<PropertyStore>()
                        .set(PropertyKey::global("recover_open"), false.into());
                    continue;
                }
                install(world, *opening);
            }
            TabAction::Switch(id) => switch(world, id),
            TabAction::Step(by) => {
                let docs = world.resource::<Documents>();
                let n = docs.tabs.len() as i64;
                if n == 0 {
                    continue;
                }
                let at = docs.front.map_or(-1, |i| i as i64);
                let to = if at < 0 && by < 0 {
                    n - 1
                } else {
                    (at + i64::from(by)).rem_euclid(n)
                };
                let id = docs.tabs[to as usize].id;
                switch(world, id);
            }
            TabAction::Recover(path) => {
                let mut editor = world.resource_mut::<Editor>();
                crate::files::recover_path(&mut editor, &path);
            }
            TabAction::Close(ids) => {
                let unsaved = {
                    let docs = world.resource::<Documents>();
                    docs.unsaved(world.resource::<Editor>(), &ids)
                };
                if unsaved.is_empty() {
                    discard(world, &ids);
                } else {
                    world.resource_mut::<Documents>().asking = Some(Ask::Close(ids));
                    ask(world, &unsaved, false);
                }
            }
            TabAction::Discard(ids) => discard(world, &ids),
            TabAction::Move(id, to) => {
                let mut docs = world.resource_mut::<Documents>();
                let Some(from) = docs.index(id) else {
                    continue;
                };
                let front = docs.front();
                let tab = docs.tabs.remove(from);
                let to = to.min(docs.tabs.len());
                docs.tabs.insert(to, tab);
                docs.front = front.and_then(|f| docs.index(f));
            }
        }
    }
}

/// Brings the document `id` to the front.
fn switch(world: &mut World, id: DocId) {
    let docs = world.resource::<Documents>();
    let Some(i) = docs.index(id) else {
        return;
    };
    if docs.front == Some(i) {
        world.resource_mut::<Home>().open = false;
        return;
    }
    if let Some(why) = busy(world) {
        world.resource_mut::<Editor>().message = Some(why.to_string());
        return;
    }
    park_front(world);
    bring(world, i);
}

/// Puts the discard question, naming the documents whose edits would go.
fn ask(world: &mut World, unsaved: &[String], quit: bool) {
    let (title, text, yes) = discard_words(unsaved, quit);
    let mut store = world.resource_mut::<PropertyStore>();
    store.set(PropertyKey::global("discard_title"), title.into());
    store.set(PropertyKey::global("discard_text"), text.into());
    store.set(PropertyKey::global("discard_yes"), yes.into());
    store.set(PropertyKey::global("discard_open"), true.into());
}

/// The discard question's title, text and button for closing or quitting
/// over `unsaved` documents.
pub fn discard_words(unsaved: &[String], quit: bool) -> (String, String, String) {
    let text = match unsaved {
        [one] => format!("{one} has edits that are not saved."),
        many => format!(
            "These documents have edits that are not saved: {}.",
            many.join(", ")
        ),
    };
    if quit {
        (
            "Quit without saving?".into(),
            text,
            "Quit without saving".into(),
        )
    } else {
        (
            "Close without saving?".into(),
            text,
            "Close without saving".into(),
        )
    }
}

/// How the tab bar is used this tick.
#[derive(SystemParam)]
pub struct TabInput<'w, 's> {
    clicks: MessageReader<'w, 's, ClickEvent>,
    menu: MessageReader<'w, 's, MenuClicked>,
    context: MessageReader<'w, 's, ContextMenuEvent>,
    pressed: MessageReader<'w, 's, PointerPressed>,
    moved: MessageReader<'w, 's, PointerMoved>,
    released: MessageReader<'w, 's, PointerReleased>,
    closing: MessageReader<'w, 's, CloseRequest>,
    hovered: Query<'w, 's, Entity, With<Hovered>>,
    ids: ElementIds<'w, 's>,
    keys: Res<'w, KeyCommands>,
}

/// The tab a tab bar element id names: its tab, or its close button.
fn tab_of(id: &str) -> Option<(DocId, bool)> {
    if let Some(n) = id.strip_prefix("tab-close-") {
        return n.parse().ok().map(|n| (DocId(n), true));
    }
    id.strip_prefix("tab-")
        .and_then(|n| n.parse().ok())
        .map(|n| (DocId(n), false))
}

/// Clicks, drags and menus of the tab bar, the keys that move between
/// tabs, the open documents in the command palette, the discard question,
/// and the window's close button while documents have unsaved edits.
#[allow(clippy::too_many_arguments)]
pub fn tab_input(
    mut docs: ResMut<Documents>,
    editor: Res<Editor>,
    mut home: ResMut<Home>,
    mut input: TabInput,
    clipboard: Clipboard,
    mut commands: Commands,
    mut signals: Signals,
) {
    let docs = &mut *docs;
    // The window's close button: with unsaved edits, ask once, naming them.
    let close_asked = input.closing.read().any(|c| !c.vetoed);
    if close_asked {
        let unsaved = docs.unsaved(&editor, &docs.ids());
        if !unsaved.is_empty() {
            commands.queue(move |world: &mut World| {
                if let Some(mut close) = world.get_resource_mut::<Messages<CloseRequest>>() {
                    close.write(CloseRequest { vetoed: true });
                }
                world.resource_mut::<Documents>().asking = Some(Ask::Quit);
                ask(world, &unsaved, true);
            });
        }
    }

    let mut chosen: Vec<String> = input.menu.read().map(|m| m.id.clone()).collect();
    let clicked: Vec<Entity> = input.clicks.read().map(|c| c.entity).collect();
    for e in clicked {
        if let Some(id) = input
            .ids
            .up(e)
            .find(|id| id.starts_with("tab-") || id.starts_with("discard-"))
        {
            chosen.push(id.to_string());
        }
    }
    for (id, repeat) in &input.keys.pressed {
        if *repeat {
            continue;
        }
        match id.as_str() {
            "tab-next" => docs.act(TabAction::Step(1)),
            "tab-prev" => docs.act(TabAction::Step(-1)),
            _ => {}
        }
    }
    for id in chosen {
        match id.as_str() {
            "tab-home" => home.show(docs.count() > 0),
            "discard-yes" => {
                signals.set("discard_open", false);
                match docs.asking.take() {
                    Some(Ask::Close(ids)) => docs.act(TabAction::Discard(ids)),
                    // Lumen gives an app no way to close its window, and
                    // the user already asked to.
                    Some(Ask::Quit) => std::process::exit(0),
                    None => {}
                }
            }
            "discard-no" => {
                signals.set("discard_open", false);
                docs.asking = None;
            }
            "tab-menu-close" => {
                if let Some(id) = docs.menu_on.take() {
                    docs.act(TabAction::Close(vec![id]));
                }
            }
            "tab-menu-close-others" => {
                if let Some(id) = docs.menu_on.take() {
                    let others = docs.ids().into_iter().filter(|o| *o != id).collect();
                    docs.act(TabAction::Close(others));
                }
            }
            "tab-menu-close-right" => {
                if let Some(id) = docs.menu_on.take() {
                    let ids = docs.ids();
                    let right = ids
                        .iter()
                        .position(|o| *o == id)
                        .map_or_else(Vec::new, |i| ids[i + 1..].to_vec());
                    docs.act(TabAction::Close(right));
                }
            }
            "tab-menu-copy-path" => {
                if let Some(id) = docs.menu_on.take()
                    && let Some(path) = docs
                        .editors(&editor)
                        .find(|(d, _)| *d == id)
                        .and_then(|(_, e)| e.recent_path().cloned())
                {
                    clipboard.set_text(&path.display().to_string());
                }
            }
            other => match tab_of(other) {
                Some((id, true)) => docs.act(TabAction::Close(vec![id])),
                Some((id, false)) => docs.act(TabAction::Switch(id)),
                None => {}
            },
        }
    }

    // A right click opens the tab's menu; a middle click closes it.
    let menus: Vec<ContextMenuEvent> = input.context.read().cloned().collect();
    for m in menus {
        if let Some((id, _)) = input.ids.up(m.entity).find_map(tab_of) {
            docs.menu_on = Some(id);
            signals.open_menu_at("tab-menu", m.position.x, m.position.y);
        }
    }
    let hovered_tab = input
        .hovered
        .iter()
        .next()
        .and_then(|e| input.ids.up(e).find_map(tab_of))
        .map(|(id, _)| id);
    let presses: Vec<PointerPressed> = input.pressed.read().cloned().collect();
    for p in presses {
        let Some(id) = hovered_tab else {
            continue;
        };
        match p.button {
            PointerButton::Middle => docs.act(TabAction::Close(vec![id])),
            PointerButton::Primary => {
                docs.drag = Some(Drag {
                    id,
                    from: p.position.x,
                    moved: false,
                })
            }
            _ => {}
        }
    }
    // A drag along the row moves the tab to where the pointer is.
    if let Some(at) = input.moved.read().last().map(|m| m.position.x)
        && let Some(drag) = docs.drag.as_mut()
    {
        if (at - drag.from).abs() > 6.0 {
            drag.moved = true;
        }
        if drag.moved {
            let id = drag.id;
            let ids = docs.ids();
            let slot = ids
                .iter()
                .filter(|o| **o != id)
                .take_while(|o| {
                    dom::get_by_id(&format!("tab-{}", o.0))
                        .and_then(|n| n.rect())
                        .is_some_and(|r| at > r.client_x + r.width / 2.0)
                })
                .count();
            if ids.iter().position(|o| *o == id) != Some(slot) {
                docs.act(TabAction::Move(id, slot));
            }
        }
    }
    if input.released.read().count() > 0 {
        docs.drag = None;
    }
}

/// The tab bar: Home, then a tab per open document with its name, a mark
/// while it has unsaved edits and its file in the tooltip. Written again
/// only when what it shows changes.
pub fn show_tabs(
    mut docs: ResMut<Documents>,
    editor: Res<Editor>,
    home: Res<Home>,
    wake: Option<Res<WakeDeadline>>,
) {
    if docs
        .shown
        .as_ref()
        .is_some_and(|s| s.is(&docs, &home, &editor))
    {
        return;
    }
    let front = docs.front.filter(|_| !home.open);
    let mut markup = format!(
        r#"<row id="tab-home" class="tab home-tab{}"><label class="tab-name" text="Home"/></row>"#,
        if home.open { " active" } else { "" }
    );
    for (i, (id, e)) in docs.editors(&editor).enumerate() {
        let mut class = String::from("tab");
        if front == Some(i) {
            class.push_str(" active");
        }
        let dirty = if e.is_modified() {
            class.push_str(" dirty");
            r#"<label class="tab-dirty" text="*"/>"#
        } else {
            ""
        };
        let tip = match (&e.path, &e.imported_from) {
            (Some(p), _) => p.display().to_string(),
            (None, Some(p)) => format!("Imported from {}", p.display()),
            (None, None) => "Not saved yet".to_string(),
        };
        markup.push_str(&format!(
            r#"<tooltip text="{tip}"><row id="tab-{n}" class="{class}"><label class="tab-name" text="{name}"/>{dirty}<button id="tab-close-{n}" class="tab-close flat small" text="x"/></row></tooltip>"#,
            tip = escape(&tip),
            n = id.0,
            name = escape(&e.name()),
        ));
    }
    let (Some(bar), Some(list)) = (dom::get_by_id("tabs"), dom::get_by_id("tab-list")) else {
        return;
    };
    crate::set_class(bar, "empty", docs.tabs.is_empty());
    list.set_inner_markup(&markup);
    docs.shown = Some(Shown::of(&docs, &home, &editor));
    crate::repaint_soon(&wake);
}

/// The open documents as command palette entries, in tab order.
pub fn palette_entries(docs: &Documents, front: &Editor) -> Vec<Entry> {
    docs.editors(front)
        .map(|(id, e)| Entry {
            label: e.name(),
            group: "Document".to_string(),
            keys: e
                .recent_path()
                .and_then(|p| p.parent())
                .map_or_else(String::new, |p| p.display().to_string()),
            run: Run::Item(format!("tab-{}", id.0)),
        })
        .collect()
}

/// Runs `edit` on the open document whose editor has `generation`, in
/// front or behind its tab: a file dialog's answer reaches the document
/// it was opened for. Returns whether that document is still open.
pub fn edit_document(world: &mut World, generation: u64, edit: impl FnOnce(&mut Editor)) -> bool {
    {
        let mut front = world.resource_mut::<Editor>();
        if front.generation == generation {
            edit(&mut front);
            return true;
        }
    }
    let mut docs = world.resource_mut::<Documents>();
    docs.changed += 1;
    let parked = docs
        .tabs
        .iter_mut()
        .filter_map(|t| t.parked.as_mut())
        .find(|p| p.editor.generation == generation);
    match parked {
        Some(p) => {
            edit(&mut p.editor);
            true
        }
        None => false,
    }
}
