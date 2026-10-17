//! The timeline under the canvas: a row per layer and a column per frame,
//! the document's tags above the frames, and playback.
//!
//! The grid is markup the editor writes when the document's frames, layers
//! or the selection change. Moving to another frame only moves the current
//! frame's highlight, so playback does not rebuild the grid. Playback steps
//! the editor's own frame, so the canvas and the preview play together; it
//! moves on only once the canvas holds the frame it showed, so a document
//! too large to keep up plays slower rather than tearing, and the frame
//! rate it reaches is shown beside the controls.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use lumenui::ecs::hierarchy::ChildOf;
use lumenui::ecs::query::Added;
use lumenui::ecs::system::SystemParam;
use lumenui::lumen_core::input::{
    ContextMenuEvent, DoubleClickEvent, EditCommitted, MenuClicked, ModifiersState, PointerButton,
    PointerMoved, PointerPressed, PointerReleased,
};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::history::Transaction;
use pixl_core::{Document, LayerId, LayerKind, LoopDirection, OnionSkin, Rgba, Scope, Tag};

use crate::editor::Editor;
use crate::home::Home;
use crate::keymap::KeyCommands;
use crate::panels::escape;
use crate::tools::{hex, set_if_changed};
use crate::view::{Layers, parse_hex};

/// Groups whose children the layer lists hide.
#[derive(Resource, Default, Clone, Debug)]
pub struct LayerTree {
    pub collapsed: BTreeSet<LayerId>,
}

/// The layers as the timeline and the layers panel list them: top first,
/// each with its depth, leaving out what collapsed groups hold.
pub fn rows<'a>(doc: &'a Document, tree: &LayerTree) -> Vec<(usize, &'a pixl_core::Layer)> {
    fn go<'a>(
        layers: &'a [pixl_core::Layer],
        depth: usize,
        tree: &LayerTree,
        out: &mut Vec<(usize, &'a pixl_core::Layer)>,
    ) {
        for l in layers.iter().rev() {
            out.push((depth, l));
            if !tree.collapsed.contains(&l.id) {
                go(l.children(), depth + 1, tree, out);
            }
        }
    }
    let mut out = Vec::new();
    go(doc.layers(), 0, tree, &mut out);
    out
}

/// How playback goes through the frames when no tag decides it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayMode {
    #[default]
    Loop,
    Once,
    PingPong,
}

impl PlayMode {
    pub fn name(self) -> &'static str {
        match self {
            PlayMode::Loop => "Loop",
            PlayMode::Once => "Play once",
            PlayMode::PingPong => "Ping-pong",
        }
    }

    pub fn from_name(name: &str) -> PlayMode {
        [PlayMode::Loop, PlayMode::Once, PlayMode::PingPong]
            .into_iter()
            .find(|m| m.name() == name)
            .unwrap_or_default()
    }
}

/// Playback in progress.
#[derive(Clone, Debug, Default)]
pub struct Playback {
    pub playing: bool,
    pub mode: PlayMode,
    /// A tag holding the current frame decides the range and direction.
    pub loop_tag: bool,
    /// The frames playback stays in, both included.
    range: (usize, usize),
    /// 1 forward, -1 backward.
    step: i8,
    ping_pong: bool,
    /// Passes left before playback stops, or `None` to go on.
    passes: Option<u32>,
    /// When the current frame's time is up.
    due: Option<Instant>,
    /// When each recent frame was shown, for the frame rate.
    shown: VecDeque<Instant>,
}

impl Playback {
    /// Starts playing from the editor's frame.
    pub fn start(&mut self, editor: &mut Editor, now: Instant) {
        let doc = &editor.doc;
        let last = doc.frame_count().saturating_sub(1);
        let tag = self
            .loop_tag
            .then(|| {
                doc.tags()
                    .iter()
                    .find(|t| (t.from..=t.to).contains(&editor.frame))
            })
            .flatten();
        let (range, direction, repeat) = match tag {
            Some(t) => ((t.from, t.to.min(last)), t.direction, t.repeat),
            None => {
                let (direction, repeat) = match self.mode {
                    PlayMode::Loop => (LoopDirection::Forward, 0),
                    PlayMode::Once => (LoopDirection::Forward, 1),
                    PlayMode::PingPong => (LoopDirection::PingPong, 0),
                };
                ((0, last), direction, repeat)
            }
        };
        self.range = range;
        self.step = if direction == LoopDirection::Reverse {
            -1
        } else {
            1
        };
        self.ping_pong = direction == LoopDirection::PingPong;
        self.passes = (repeat > 0).then_some(repeat);
        if !(range.0..=range.1).contains(&editor.frame) {
            editor.frame = if self.step > 0 { range.0 } else { range.1 };
        }
        self.playing = true;
        self.shown.clear();
        self.shown.push_back(now);
        self.due = Some(now + frame_time(doc, editor.frame));
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.due = None;
    }

    /// The frame after `frame`, or `None` when playback ends there.
    fn next(&mut self, frame: usize) -> Option<usize> {
        let (a, b) = self.range;
        if a == b {
            return self.pass_done().then_some(a);
        }
        let to = frame as i64 + i64::from(self.step);
        if (a as i64..=b as i64).contains(&to) {
            return Some(to as usize);
        }
        if !self.pass_done() {
            return None;
        }
        if self.ping_pong {
            self.step = -self.step;
            Some((frame as i64 + i64::from(self.step)) as usize)
        } else if self.step > 0 {
            Some(a)
        } else {
            Some(b)
        }
    }

    /// Counts a pass through the range; false when it was the last one.
    fn pass_done(&mut self) -> bool {
        match &mut self.passes {
            None => true,
            Some(n) => {
                *n = n.saturating_sub(1);
                *n > 0
            }
        }
    }

    /// The frames per second playback reached over the last second, or over
    /// the last two frames when it shows fewer than that.
    pub fn fps(&self) -> Option<f64> {
        let (first, last) = (self.shown.front()?, self.shown.back()?);
        let span = last.duration_since(*first).as_secs_f64();
        (self.shown.len() >= 2 && span > 0.0).then(|| (self.shown.len() - 1) as f64 / span)
    }
}

/// How long `frame` shows, at least a millisecond.
fn frame_time(doc: &Document, frame: usize) -> Duration {
    let ms = doc.frames().get(frame).map_or(100, |f| f.duration_ms);
    Duration::from_millis(u64::from(ms.max(1)))
}

/// The onion skin as the timeline's controls set it.
#[derive(Clone, Debug, PartialEq)]
pub struct OnionSettings {
    pub on: bool,
    pub before: usize,
    pub after: usize,
    /// Of the nearest ghost, in percent.
    pub opacity: u8,
    pub falloff: u8,
    pub tint: bool,
    pub above: bool,
}

impl Default for OnionSettings {
    fn default() -> Self {
        Self {
            on: false,
            before: 1,
            after: 1,
            opacity: 40,
            falloff: 50,
            tint: true,
            above: false,
        }
    }
}

impl OnionSettings {
    /// The skin pixl-core draws, with the tints the theme gives.
    pub fn skin(&self, tints: (Option<Rgba>, Option<Rgba>)) -> Option<OnionSkin> {
        (self.on && self.before + self.after > 0).then(|| OnionSkin {
            before: self.before,
            after: self.after,
            opacity: (u32::from(self.opacity.min(100)) * 255 / 100) as u8,
            falloff_percent: self.falloff.min(100),
            tint_before: tints.0.filter(|_| self.tint),
            tint_after: tints.1.filter(|_| self.tint),
            above: self.above,
        })
    }
}

/// A selection of cels: the frames and rows between two corners.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CelSelection {
    pub anchor: (LayerId, usize),
    pub focus: (LayerId, usize),
}

/// A drag in the timeline, from the press to the release.
enum Drag {
    /// Over cels or frame numbers, choosing a range.
    Select { columns: bool },
    /// Selected frames picked up by their numbers.
    MoveFrames {
        from: usize,
        count: usize,
        over: usize,
    },
    /// A tag's first frame, last frame or the whole tag.
    Tag {
        index: usize,
        part: TagPart,
        grabbed: usize,
        original: (usize, usize),
        tx: Option<Box<Transaction>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagPart {
    Start,
    End,
    Whole,
}

/// What a context menu was opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuOn {
    Frames,
    Tag(usize),
}

/// The timeline's state.
#[derive(Resource)]
pub struct Timeline {
    pub selection: Option<CelSelection>,
    pub playback: Playback,
    pub onion: OnionSettings,
    drag: Option<Drag>,
    menu_on: Option<MenuOn>,
    /// The tag the tag dialog edits; `None` while it makes a new one.
    editing_tag: Option<usize>,
    tag_color: Option<Rgba>,
    /// The tag dialog's name, and the ticks left to write it into the
    /// field, which has focus once the dialog shows (lumen-fx/lumen#536).
    tag_name: (String, u8),
    /// What the grid was last brought up to date with, and what it shows.
    grid: Option<u64>,
    shown: Option<GridModel>,
    /// The controls' values as last shown.
    controls: Option<Controls>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            selection: None,
            playback: Playback {
                loop_tag: true,
                ..Playback::default()
            },
            onion: OnionSettings::default(),
            drag: None,
            menu_on: None,
            editing_tag: None,
            tag_color: None,
            tag_name: (String::new(), 0),
            grid: None,
            shown: None,
            controls: None,
        }
    }
}

/// The values the timeline's controls show.
#[derive(Clone, Debug, PartialEq)]
struct Controls {
    mode: PlayMode,
    loop_tag: bool,
    onion: OnionSettings,
    duration: String,
}

impl Timeline {
    /// The selected frames, first and last; the current frame alone when
    /// nothing is selected.
    pub fn frames(&self, editor: &Editor) -> (usize, usize) {
        let last = editor.doc.frame_count().saturating_sub(1);
        match self.selection {
            Some(s) => {
                let (a, b) = (s.anchor.1.min(s.focus.1), s.anchor.1.max(s.focus.1));
                (a.min(last), b.min(last))
            }
            None => (editor.frame, editor.frame),
        }
    }

    /// The selected layers, top first; the active layer alone when nothing
    /// is selected.
    pub fn layers(&self, editor: &Editor, tree: &LayerTree) -> Vec<LayerId> {
        let Some(s) = self.selection else {
            return vec![editor.layer];
        };
        let order: Vec<LayerId> = rows(&editor.doc, tree).iter().map(|(_, l)| l.id).collect();
        let (Some(a), Some(b)) = (
            order.iter().position(|l| *l == s.anchor.0),
            order.iter().position(|l| *l == s.focus.0),
        ) else {
            return vec![editor.layer];
        };
        order[a.min(b)..=a.max(b)].to_vec()
    }

    fn is_selected(
        &self,
        editor: &Editor,
        order: &[LayerId],
        layer: LayerId,
        frame: usize,
    ) -> bool {
        let Some(s) = self.selection else {
            return false;
        };
        let (fa, fb) = self.frames(editor);
        if !(fa..=fb).contains(&frame) {
            return false;
        }
        let pos = |id: LayerId| order.iter().position(|l| *l == id);
        match (pos(s.anchor.0), pos(s.focus.0), pos(layer)) {
            (Some(a), Some(b), Some(p)) => (a.min(b)..=a.max(b)).contains(&p),
            _ => false,
        }
    }
}

// ---- frame edits ----

impl Editor {
    /// Shows another frame.
    pub fn go_to_frame(&mut self, frame: usize) {
        self.frame = frame.min(self.doc.frame_count().saturating_sub(1));
    }

    /// Runs a structural edit over frames as one undo step.
    fn frame_edit<R>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut Document) -> std::result::Result<R, pixl_core::Error>,
    ) -> Option<R> {
        let done = self
            .history
            .transact(&mut self.doc, label, Scope::All, edit);
        match done {
            Ok(r) => {
                self.frame = self.frame.min(self.doc.frame_count().saturating_sub(1));
                self.touch();
                Some(r)
            }
            Err(e) => {
                self.message = Some(e.to_string());
                None
            }
        }
    }

    /// Adds an empty frame after `after`, as long as `after` lasts, and
    /// shows it.
    pub fn new_frame(&mut self, after: usize) {
        let duration = self.doc.frames().get(after).map_or(100, |f| f.duration_ms);
        if self
            .frame_edit("New frame", |doc| doc.insert_frame(after + 1, duration))
            .is_some()
        {
            self.frame = after + 1;
        }
    }

    /// Copies frames `a..=b` (each cel with its own pixels) to just after
    /// them and shows the first copy.
    pub fn duplicate_frames(&mut self, a: usize, b: usize) {
        let n = b - a + 1;
        let done = self.frame_edit("Copy frames", |doc| {
            for k in 0..n {
                let copy = doc.duplicate_frame(a + k)?;
                doc.move_frames(copy, 1, b + 1 + k)?;
            }
            Ok(())
        });
        if done.is_some() {
            self.frame = b + 1;
        }
    }

    /// Deletes frames `a..=b`. A document keeps one frame, so deleting
    /// every frame keeps the first.
    pub fn delete_frames(&mut self, a: usize, b: usize) {
        let a = if b - a + 1 >= self.doc.frame_count() {
            a + 1
        } else {
            a
        };
        if a > b {
            return;
        }
        if self
            .frame_edit("Delete frames", |doc| {
                for _ in a..=b {
                    doc.remove_frame(a)?;
                }
                Ok(())
            })
            .is_some()
        {
            self.go_to_frame(a.min(self.frame));
        }
    }

    /// Moves frames `from..from + count` to start at `to`.
    pub fn move_frames(&mut self, from: usize, count: usize, to: usize) {
        if from == to {
            return;
        }
        let current = self.frame;
        if self
            .frame_edit("Move frames", |doc| doc.move_frames(from, count, to))
            .is_some()
            && (from..from + count).contains(&current)
        {
            self.frame = to + (current - from);
        }
    }

    /// Reverses the order of frames `a..=b`.
    pub fn reverse_frames(&mut self, a: usize, b: usize) {
        if a >= b {
            return;
        }
        self.frame_edit("Reverse frames", |doc| {
            for k in 0..(b - a) {
                doc.move_frames(b, 1, a + k)?;
            }
            Ok(())
        });
    }

    /// Sets how long frames `a..=b` show.
    pub fn set_durations(&mut self, a: usize, b: usize, ms: u32) {
        let same = (a..=b).all(|f| {
            self.doc
                .frames()
                .get(f)
                .is_some_and(|x| x.duration_ms == ms)
        });
        if same {
            return;
        }
        self.frame_edit("Frame duration", |doc| {
            for f in a..=b {
                doc.set_frame_duration(f, ms)?;
            }
            Ok(())
        });
    }

    /// Links the cels of `layers` in frames `a..=b` to the cel in frame `a`.
    pub fn link_cels(&mut self, layers: &[LayerId], a: usize, b: usize) {
        if a >= b {
            return;
        }
        let rasters: Vec<LayerId> = layers
            .iter()
            .copied()
            .filter(|l| self.doc.layer(*l).is_some_and(|l| l.is_raster()))
            .collect();
        self.frame_edit("Link cels", |doc| {
            for l in &rasters {
                for f in a + 1..=b {
                    doc.link_cel(*l, a, f)?;
                }
            }
            Ok(())
        });
    }

    /// Gives each cel of `layers` in frames `a..=b` its own pixels.
    pub fn unlink_cels(&mut self, layers: &[LayerId], a: usize, b: usize) {
        let rasters: Vec<LayerId> = layers
            .iter()
            .copied()
            .filter(|l| self.doc.layer(*l).is_some_and(|l| l.is_raster()))
            .collect();
        self.frame_edit("Unlink cels", |doc| {
            for l in &rasters {
                for f in a..=b {
                    doc.unlink_cel(*l, f)?;
                }
            }
            Ok(())
        });
    }

    /// Empties the cels of `layers` in frames `a..=b`.
    pub fn clear_cels(&mut self, layers: &[LayerId], a: usize, b: usize) {
        let rasters: Vec<LayerId> = layers
            .iter()
            .copied()
            .filter(|l| self.doc.layer(*l).is_some_and(|l| l.is_raster()))
            .collect();
        self.frame_edit("Clear cels", |doc| {
            for l in &rasters {
                for f in a..=b {
                    doc.clear_cel(*l, f)?;
                }
            }
            Ok(())
        });
    }

    /// Tags frames `a..=b` and returns the tag's index.
    pub fn add_tag(&mut self, a: usize, b: usize, color: Rgba) -> Option<usize> {
        let name = format!("Tag {}", self.doc.tags().len() + 1);
        self.frame_edit("New tag", |doc| {
            doc.tags_mut().push(Tag {
                name,
                from: a,
                to: b,
                direction: LoopDirection::Forward,
                repeat: 0,
                color,
            });
            Ok(doc.tags().len() - 1)
        })
    }
}

// ---- input ----

/// Where the pointer and the menus land in the timeline.
#[derive(SystemParam)]
pub struct TimelineInput<'w, 's> {
    clicks: MessageReader<'w, 's, ClickEvent>,
    doubles: MessageReader<'w, 's, DoubleClickEvent>,
    menus: MessageReader<'w, 's, ContextMenuEvent>,
    chosen: MessageReader<'w, 's, MenuClicked>,
    pressed: MessageReader<'w, 's, PointerPressed>,
    moved: MessageReader<'w, 's, PointerMoved>,
    released: MessageReader<'w, 's, PointerReleased>,
    modifiers: Res<'w, ModifiersState>,
    ids: Query<'w, 's, &'static LumenId>,
    parents: Query<'w, 's, &'static ChildOf>,
    hovered: Query<'w, 's, Entity, With<Hovered>>,
    committed: Query<'w, 's, &'static LumenId, Added<EditCommitted>>,
    focused: Query<'w, 's, &'static LumenId, With<Focused>>,
}

impl TimelineInput<'_, '_> {
    /// The id of `entity` or its nearest ancestor with one.
    fn id_of(&self, mut entity: Entity) -> Option<String> {
        loop {
            if let Ok(id) = self.ids.get(entity) {
                return Some(id.0.clone());
            }
            entity = self.parents.get(entity).ok()?.parent();
        }
    }

    /// What the hovered element is, as far as the timeline goes.
    fn hovered_spot(&self) -> Hovering {
        let Some(mut e) = self.hovered.iter().next() else {
            return Hovering::Nothing;
        };
        let mut spot = None;
        loop {
            if let Ok(id) = self.ids.get(e) {
                if spot.is_none() {
                    spot = Spot::parse(&id.0);
                }
                if id.0 == "panel-timeline" {
                    return spot.map_or(Hovering::Timeline, Hovering::Spot);
                }
            }
            match self.parents.get(e) {
                Ok(p) => e = p.parent(),
                Err(_) => return Hovering::Elsewhere,
            }
        }
    }
}

/// Where the hovered element is.
enum Hovering {
    Nothing,
    /// In the timeline, on a cel, frame, tag or layer name.
    Spot(Spot),
    /// In the timeline, between its parts.
    Timeline,
    Elsewhere,
}

/// A part of the timeline an id names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Cel(LayerId, usize),
    Frame(usize),
    Tag(usize, usize),
    Layer(LayerId),
}

impl Spot {
    pub fn parse(id: &str) -> Option<Spot> {
        let nums = |rest: &str| -> Option<(u32, usize)> {
            let (a, b) = rest.split_once('-')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        };
        if let Some(rest) = id.strip_prefix("cel-") {
            let (l, f) = nums(rest)?;
            Some(Spot::Cel(LayerId(l), f))
        } else if let Some(rest) = id.strip_prefix("tl-frame-") {
            Some(Spot::Frame(rest.parse().ok()?))
        } else if let Some(rest) = id.strip_prefix("tl-tag-") {
            let (t, f) = nums(rest)?;
            Some(Spot::Tag(t as usize, f))
        } else if let Some(rest) = id.strip_prefix("tl-layer-") {
            Some(Spot::Layer(LayerId(rest.parse().ok()?)))
        } else {
            None
        }
    }
}

/// Clicks, drags, keys, menus and controls of the timeline.
#[allow(clippy::too_many_arguments)]
pub fn timeline_input(
    mut editor: ResMut<Editor>,
    mut timeline: ResMut<Timeline>,
    tree: Res<LayerTree>,
    home: Res<Home>,
    keys: Res<KeyCommands>,
    mut input: TimelineInput,
    mut signals: Signals,
) {
    if home.open {
        input.clicks.clear();
        input.doubles.clear();
        input.menus.clear();
        input.chosen.clear();
        input.pressed.clear();
        input.moved.clear();
        input.released.clear();
        return;
    }
    let editor = &mut *editor;
    let tl = &mut *timeline;
    if tl.tag_name.1 > 0 {
        tl.tag_name.1 -= 1;
        if let Some(field) = dom::get_by_id("tag-name") {
            field.set_text(&tl.tag_name.0);
        }
        if let Some(c) = tl.tag_color {
            mark_tag_color(c);
        }
    }
    let shift = input.modifiers.0.shift;
    let order: Vec<LayerId> = rows(&editor.doc, &tree).iter().map(|(_, l)| l.id).collect();
    let top = order.first().copied().unwrap_or(editor.layer);
    let bottom = order.last().copied().unwrap_or(editor.layer);

    // Presses start a selection or a drag.
    let presses: Vec<PointerPressed> = input.pressed.read().cloned().collect();
    for press in presses {
        if press.button != PointerButton::Primary {
            continue;
        }
        // What the pointer is over decides, unless it is over something
        // else, such as a menu above the timeline.
        let spot = match input.hovered_spot() {
            Hovering::Spot(spot) => Some(spot),
            Hovering::Elsewhere => None,
            Hovering::Timeline | Hovering::Nothing => spot_at(press.position, &order, &editor.doc),
        };
        let Some(spot) = spot else {
            continue;
        };
        match spot {
            Spot::Cel(layer, frame) => {
                editor.layer = layer;
                editor.go_to_frame(frame);
                select_to(tl, editor, (layer, frame), shift);
                tl.drag = Some(Drag::Select { columns: false });
            }
            Spot::Frame(frame) => {
                let (a, b) = tl.frames(editor);
                let whole_columns = tl
                    .selection
                    .is_some_and(|s| order.len() <= 1 || s.anchor.0 != s.focus.0);
                if !shift && whole_columns && (a..=b).contains(&frame) {
                    tl.drag = Some(Drag::MoveFrames {
                        from: a,
                        count: b - a + 1,
                        over: frame,
                    });
                } else {
                    editor.go_to_frame(frame);
                    if shift {
                        let anchor = tl.selection.map_or((top, editor.frame), |s| s.anchor);
                        tl.selection = Some(CelSelection {
                            anchor: (top, anchor.1),
                            focus: (bottom, frame),
                        });
                    } else {
                        tl.selection = Some(CelSelection {
                            anchor: (top, frame),
                            focus: (bottom, frame),
                        });
                    }
                    tl.drag = Some(Drag::Select { columns: true });
                }
            }
            Spot::Tag(index, frame) => {
                let Some(tag) = editor.doc.tags().get(index).cloned() else {
                    continue;
                };
                let part = if frame == tag.to {
                    TagPart::End
                } else if frame == tag.from {
                    TagPart::Start
                } else {
                    TagPart::Whole
                };
                tl.drag = Some(Drag::Tag {
                    index,
                    part,
                    grabbed: frame,
                    original: (tag.from, tag.to),
                    tx: None,
                });
            }
            Spot::Layer(layer) => {
                if editor.doc.layer(layer).is_some() {
                    editor.layer = layer;
                    if shift {
                        select_to(tl, editor, (layer, editor.frame), true);
                    } else {
                        tl.selection = None;
                    }
                }
            }
        }
    }

    // Moves extend the selection or carry the drag. While the button is
    // down the hover stays on the pressed element, so the spot under the
    // pointer comes from where the frames and rows are laid out.
    let last_move = input.moved.read().last().map(|m| m.position);
    let dragged_to = match (&tl.drag, last_move) {
        (Some(_), Some(at)) => spot_at(at, &order, &editor.doc),
        _ => None,
    };
    if let Some(spot) = dragged_to {
        match (&mut tl.drag, spot) {
            (Some(Drag::Select { columns: false }), Spot::Cel(layer, frame)) => {
                if let Some(s) = &mut tl.selection {
                    s.focus = (layer, frame);
                } else {
                    tl.selection = Some(CelSelection {
                        anchor: (editor.layer, editor.frame),
                        focus: (layer, frame),
                    });
                }
            }
            (Some(Drag::Select { columns: true }), Spot::Frame(frame) | Spot::Cel(_, frame)) => {
                if let Some(s) = &mut tl.selection {
                    s.focus = (bottom, frame);
                }
            }
            (Some(Drag::MoveFrames { over, .. }), Spot::Frame(frame) | Spot::Cel(_, frame)) => {
                *over = frame;
            }
            (
                Some(Drag::Tag {
                    index,
                    part,
                    grabbed,
                    original,
                    tx,
                }),
                Spot::Frame(frame) | Spot::Cel(_, frame) | Spot::Tag(_, frame),
            ) => {
                let last = editor.doc.frame_count().saturating_sub(1);
                let (from, to) = *original;
                let range = match part {
                    TagPart::Start => (frame.min(to), to),
                    TagPart::End => (from, frame.max(from).min(last)),
                    TagPart::Whole => {
                        let shift = frame as i64 - *grabbed as i64;
                        let len = to - from;
                        let start = (from as i64 + shift).clamp(0, (last - len) as i64) as usize;
                        (start, start + len)
                    }
                };
                let i = *index;
                let now = editor.doc.tags().get(i).map(|t| (t.from, t.to));
                if now.is_some() && now != Some(range) {
                    if tx.is_none() {
                        *tx = Some(Box::new(editor.history.begin(
                            &editor.doc,
                            "Tag frames",
                            Scope::Images(Vec::new()),
                        )));
                    }
                    if let Some(t) = editor.doc.tags_mut().get_mut(i) {
                        t.from = range.0;
                        t.to = range.1;
                    }
                    editor.touch();
                }
            }
            _ => {}
        }
    }

    // A release ends the drag.
    if input.released.read().count() > 0 {
        match tl.drag.take() {
            Some(Drag::MoveFrames { from, count, over }) => {
                if !(from..from + count).contains(&over) {
                    let n = editor.doc.frame_count();
                    let to = if over > from {
                        (over + 1 - count).min(n - count)
                    } else {
                        over
                    };
                    editor.move_frames(from, count, to);
                    if let Some(s) = &mut tl.selection {
                        s.anchor.1 = to;
                        s.focus.1 = to + count - 1;
                    }
                }
            }
            Some(Drag::Tag {
                tx: Some(tx),
                index,
                ..
            }) => {
                editor.history.commit(&editor.doc, *tx);
                editor.touch();
                select_tag(tl, editor, index, &order);
            }
            Some(Drag::Tag {
                tx: None, index, ..
            }) => {
                // A click on a tag selects its frames.
                select_tag(tl, editor, index, &order);
            }
            _ => {}
        }
    }

    // Buttons.
    let clicked: Vec<String> = {
        let entities: Vec<Entity> = input.clicks.read().map(|c| c.entity).collect();
        entities
            .into_iter()
            .filter_map(|e| input.id_of(e))
            .collect()
    };
    let mut commands: Vec<String> = clicked;
    commands.extend(input.chosen.read().map(|m| m.id.clone()));
    for id in commands {
        command(&id, editor, tl, &tree, &mut signals);
    }

    // Double-clicks on a tag open its dialog.
    let doubles: Vec<Entity> = input.doubles.read().map(|d| d.entity).collect();
    for e in doubles {
        if let Some(Spot::Tag(index, _)) = input.id_of(e).and_then(|id| Spot::parse(&id)) {
            open_tag_dialog(tl, editor, Some(index), &mut signals);
        }
    }

    // Context menus.
    let menus: Vec<ContextMenuEvent> = input.menus.read().cloned().collect();
    for m in menus {
        let Some(spot) = input.id_of(m.entity).and_then(|id| Spot::parse(&id)) else {
            continue;
        };
        match spot {
            Spot::Cel(layer, frame) => {
                if !tl.is_selected(editor, &order, layer, frame) {
                    editor.layer = layer;
                    editor.go_to_frame(frame);
                    tl.selection = None;
                }
                tl.menu_on = Some(MenuOn::Frames);
                signals.open_menu_at("frame-menu", m.position.x, m.position.y);
            }
            Spot::Frame(frame) => {
                let (a, b) = tl.frames(editor);
                if !(a..=b).contains(&frame) {
                    editor.go_to_frame(frame);
                    tl.selection = Some(CelSelection {
                        anchor: (top, frame),
                        focus: (bottom, frame),
                    });
                }
                tl.menu_on = Some(MenuOn::Frames);
                signals.open_menu_at("frame-menu", m.position.x, m.position.y);
            }
            Spot::Tag(index, _) => {
                tl.menu_on = Some(MenuOn::Tag(index));
                signals.open_menu_at("tag-menu", m.position.x, m.position.y);
            }
            Spot::Layer(_) => {}
        }
    }

    // Frame and layer keys, through the keymap.
    let n = editor.doc.frame_count();
    let step = |editor: &mut Editor, tl: &mut Timeline, to: usize| {
        editor.go_to_frame(to);
        tl.selection = None;
    };
    for (id, _) in keys.pressed.clone() {
        match id.as_str() {
            "frame-prev" => step(editor, tl, (editor.frame + n - 1) % n),
            "frame-next" => step(editor, tl, (editor.frame + 1) % n),
            "frame-first" => step(editor, tl, 0),
            "frame-last" => step(editor, tl, n - 1),
            "frame-new" => command("tl-new", editor, tl, &tree, &mut signals),
            "play" => command("tl-play", editor, tl, &tree, &mut signals),
            "layer-above" | "layer-below" => {
                if let Some(pos) = order.iter().position(|l| *l == editor.layer) {
                    let next = if id == "layer-above" {
                        pos.checked_sub(1)
                    } else {
                        Some(pos + 1).filter(|p| *p < order.len())
                    };
                    if let Some(p) = next {
                        editor.layer = order[p];
                        tl.selection = None;
                    }
                }
            }
            _ => {}
        }
    }

    controls(editor, tl, &input, &mut signals);
}

/// The timeline spot at a window position, by where the frame numbers,
/// the tag lanes and the layer rows are laid out.
fn spot_at(at: lumenui::glam::Vec2, order: &[LayerId], doc: &Document) -> Option<Spot> {
    let rect = |id: &str| dom::get_by_id(id).and_then(|n| n.rect());
    let inside = |r: &lumenui::dom::Rect| {
        at.x >= r.client_x
            && at.x < r.client_x + r.width
            && at.y >= r.client_y
            && at.y < r.client_y + r.height
    };
    let rows = |y: f32| {
        order.iter().copied().find(|id| {
            rect(&format!("tl-layer-{}", id.0))
                .is_some_and(|r| y >= r.client_y && y < r.client_y + r.height)
        })
    };
    if rect("tl-names").is_some_and(|r| inside(&r)) {
        return rows(at.y).map(Spot::Layer);
    }
    if !rect("tl-grid").is_some_and(|r| inside(&r)) {
        return None;
    }
    let first = rect("tl-frame-0")?;
    if first.width <= 0.0 {
        return None;
    }
    let frame = ((at.x - first.client_x) / first.width).floor().max(0.0) as usize;
    if at.y >= first.client_y && at.y < first.client_y + first.height {
        return Some(Spot::Frame(frame));
    }
    if let Some(layer) = rows(at.y) {
        return Some(Spot::Cel(layer, frame));
    }
    for (k, lane) in lanes(doc.tags()).iter().enumerate() {
        if rect(&format!("tl-lane-{k}"))
            .is_some_and(|r| at.y >= r.client_y && at.y < r.client_y + r.height)
        {
            return lane
                .iter()
                .find(|&&i| (doc.tags()[i].from..=doc.tags()[i].to).contains(&frame))
                .map(|&i| Spot::Tag(i, frame));
        }
    }
    None
}

/// Extends the selection to `to`, or starts it there.
fn select_to(tl: &mut Timeline, editor: &Editor, to: (LayerId, usize), extend: bool) {
    tl.selection = match (extend, tl.selection) {
        (true, Some(s)) => Some(CelSelection {
            anchor: s.anchor,
            focus: to,
        }),
        (true, None) => Some(CelSelection {
            anchor: (editor.layer, editor.frame),
            focus: to,
        }),
        (false, _) => Some(CelSelection {
            anchor: to,
            focus: to,
        }),
    };
}

/// Selects every row of a tag's frames and shows its first frame.
fn select_tag(tl: &mut Timeline, editor: &mut Editor, index: usize, order: &[LayerId]) {
    let Some(tag) = editor.doc.tags().get(index) else {
        return;
    };
    let (from, to) = (tag.from, tag.to);
    if let (Some(top), Some(bottom)) = (order.first(), order.last()) {
        tl.selection = Some(CelSelection {
            anchor: (*top, from),
            focus: (*bottom, to),
        });
    }
    editor.go_to_frame(from);
}

/// The color a new tag gets: the theme's first tag color.
fn first_tag_color() -> Rgba {
    dom::get_by_id("probe-tag")
        .and_then(|n| n.computed_style_of("bg"))
        .as_deref()
        .and_then(parse_hex)
        .unwrap_or(Rgba::WHITE)
}

fn open_tag_dialog(
    tl: &mut Timeline,
    editor: &Editor,
    index: Option<usize>,
    signals: &mut Signals,
) {
    let (a, b) = tl.frames(editor);
    let tag = index.and_then(|i| editor.doc.tags().get(i)).cloned();
    tl.editing_tag = index.filter(|_| tag.is_some());
    let tag = tag.unwrap_or_else(|| Tag {
        name: format!("Tag {}", editor.doc.tags().len() + 1),
        from: a,
        to: b,
        direction: LoopDirection::Forward,
        repeat: 0,
        color: first_tag_color(),
    });
    tl.tag_color = Some(tag.color);
    tl.tag_name = (tag.name.clone(), 2);
    signals.set("tag_name", tag.name.clone());
    signals.set("tag_from", (tag.from + 1).to_string());
    signals.set("tag_to", (tag.to + 1).to_string());
    signals.set(
        "tag_direction",
        match tag.direction {
            LoopDirection::Forward => "Forward",
            LoopDirection::Reverse => "Reverse",
            LoopDirection::PingPong => "Ping-pong",
        },
    );
    signals.set("tag_repeat", tag.repeat.to_string());
    mark_tag_color(tag.color);
    signals.set("tag_open", true);
}

/// Rings the tag dialog's color choice that matches `color`.
fn mark_tag_color(color: Rgba) {
    for i in 1..=8 {
        let Some(node) = dom::get_by_id(&format!("tag-color-{i}")) else {
            continue;
        };
        let c = node.computed_style_of("bg").as_deref().and_then(parse_hex);
        crate::set_class(node, "chosen", c == Some(color));
    }
}

/// The tag dialog's fields, applied.
fn apply_tag_dialog(tl: &mut Timeline, editor: &mut Editor, signals: &Signals) {
    let text = |name: &str| signals.get::<String>(name).unwrap_or_default();
    let last = editor.doc.frame_count().saturating_sub(1);
    let frame = |name: &str| {
        text(name)
            .trim()
            .parse::<usize>()
            .ok()
            .map(|f| f.saturating_sub(1).min(last))
    };
    let (Some(from), Some(to)) = (frame("tag_from"), frame("tag_to")) else {
        editor.message = Some("A tag's frames are numbers such as 1 and 4".into());
        return;
    };
    let name = text("tag_name").trim().to_string();
    let tag = Tag {
        name: if name.is_empty() { "Tag".into() } else { name },
        from: from.min(to),
        to: from.max(to),
        direction: match text("tag_direction").as_str() {
            "Reverse" => LoopDirection::Reverse,
            "Ping-pong" => LoopDirection::PingPong,
            _ => LoopDirection::Forward,
        },
        repeat: text("tag_repeat").trim().parse().unwrap_or(0),
        color: tl.tag_color.unwrap_or_else(first_tag_color),
    };
    let index = tl.editing_tag;
    let label = if index.is_some() { "Tag" } else { "New tag" };
    editor.frame_edit(label, |doc| {
        match index.and_then(|i| doc.tags_mut().get_mut(i)) {
            Some(t) => *t = tag,
            None => doc.tags_mut().push(tag),
        }
        Ok(())
    });
}

fn delete_tag(editor: &mut Editor, index: usize) {
    if index < editor.doc.tags().len() {
        editor.frame_edit("Delete tag", |doc| {
            doc.tags_mut().remove(index);
            Ok(())
        });
    }
}

/// A timeline button or menu item.
fn command(
    id: &str,
    editor: &mut Editor,
    tl: &mut Timeline,
    tree: &LayerTree,
    signals: &mut Signals,
) {
    let (a, b) = tl.frames(editor);
    let last = editor.doc.frame_count().saturating_sub(1);
    match id {
        "tl-first" => {
            editor.go_to_frame(0);
            tl.selection = None;
        }
        "tl-last" => {
            editor.go_to_frame(last);
            tl.selection = None;
        }
        "tl-prev" => {
            editor.go_to_frame(if editor.frame == 0 {
                last
            } else {
                editor.frame - 1
            });
            tl.selection = None;
        }
        "tl-next" => {
            editor.go_to_frame(if editor.frame >= last {
                0
            } else {
                editor.frame + 1
            });
            tl.selection = None;
        }
        "tl-play" => {
            if tl.playback.playing {
                tl.playback.stop();
            } else {
                tl.playback.start(editor, Instant::now());
            }
        }
        "tl-new" | "fm-new" => {
            editor.new_frame(b);
            tl.selection = None;
        }
        "tl-duplicate" | "fm-duplicate" => {
            editor.duplicate_frames(a, b);
            if b > a {
                let n = b - a;
                if let Some(s) = &mut tl.selection {
                    s.anchor.1 = b + 1;
                    s.focus.1 = b + 1 + n;
                }
            } else {
                tl.selection = None;
            }
        }
        "tl-delete" | "fm-delete" => {
            editor.delete_frames(a, b);
            tl.selection = None;
        }
        "tl-link" | "fm-link" => {
            let layers = tl.layers(editor, tree);
            editor.link_cels(&layers, a, b);
        }
        "tl-unlink" | "fm-unlink" => {
            let layers = tl.layers(editor, tree);
            editor.unlink_cels(&layers, a, b);
        }
        "fm-clear" => {
            let layers = tl.layers(editor, tree);
            editor.clear_cels(&layers, a, b);
        }
        "fm-reverse" => editor.reverse_frames(a, b),
        "tl-tag" | "fm-tag" => open_tag_dialog(tl, editor, None, signals),
        "tm-edit" => {
            if let Some(MenuOn::Tag(i)) = tl.menu_on {
                open_tag_dialog(tl, editor, Some(i), signals);
            }
        }
        "tm-select" => {
            if let Some(MenuOn::Tag(i)) = tl.menu_on {
                let order: Vec<LayerId> =
                    rows(&editor.doc, tree).iter().map(|(_, l)| l.id).collect();
                select_tag(tl, editor, i, &order);
            }
        }
        "tm-delete" => {
            if let Some(MenuOn::Tag(i)) = tl.menu_on {
                delete_tag(editor, i);
            }
        }
        "tag-ok" => {
            signals.set("tag_open", false);
            apply_tag_dialog(tl, editor, signals);
        }
        "tag-cancel" => signals.set("tag_open", false),
        "tag-delete" => {
            signals.set("tag_open", false);
            if let Some(i) = tl.editing_tag {
                delete_tag(editor, i);
            }
        }
        other => {
            if let Some(i) = other.strip_prefix("tag-color-")
                && let Some(c) = dom::get_by_id(other)
                    .and_then(|n| n.computed_style_of("bg"))
                    .as_deref()
                    .and_then(parse_hex)
                && i.parse::<u8>().is_ok()
            {
                tl.tag_color = Some(c);
                mark_tag_color(c);
            }
        }
    }
}

/// The playback mode, the tag loop, the onion skin and the duration field.
fn controls(editor: &mut Editor, tl: &mut Timeline, input: &TimelineInput, signals: &mut Signals) {
    let Some(last) = tl.controls.clone() else {
        return;
    };
    let mode = signals
        .get::<String>("tl_mode")
        .map_or(last.mode, |n| PlayMode::from_name(&n));
    tl.playback.mode = mode;
    tl.playback.loop_tag = signals.get::<bool>("tl_loop_tag").unwrap_or(last.loop_tag);
    let o = &mut tl.onion;
    let flag = |name: &str, was: bool| signals.get::<bool>(name).unwrap_or(was);
    let count = |name: &str, was: usize| {
        signals
            .get::<String>(name)
            .and_then(|t| t.trim().parse::<usize>().ok())
            .map_or(was, |v| v.min(16))
    };
    let percent = |name: &str, was: u8| {
        signals
            .get::<f64>(name)
            .map_or(was, |v| v.round().clamp(0.0, 100.0) as u8)
    };
    o.on = flag("onion_on", last.onion.on);
    o.tint = flag("onion_tint", last.onion.tint);
    o.above = flag("onion_above", last.onion.above);
    o.before = count("onion_before", last.onion.before);
    o.after = count("onion_after", last.onion.after);
    o.opacity = percent("onion_opacity", last.onion.opacity);
    o.falloff = percent("onion_falloff", last.onion.falloff);

    // The duration field applies when its edit is committed, or when it
    // changes without the keyboard in it (its step buttons).
    let typed = signals.get::<String>("tl_duration").unwrap_or_default();
    let focused = input.focused.iter().any(|id| id.0 == "tl-duration");
    let committed = input.committed.iter().any(|id| id.0 == "tl-duration");
    if typed != last.duration && (committed || !focused) {
        match typed.trim().parse::<u32>() {
            Ok(ms) if (1..=60_000).contains(&ms) => {
                let (a, b) = tl.frames(editor);
                editor.set_durations(a, b, ms);
            }
            _ => editor.message = Some("A frame lasts 1 to 60000 ms".into()),
        }
        // Show what the frames hold now.
        tl.controls = None;
    }
}

// ---- playback ----

/// Steps playback: shows the next frame once the current frame's time is
/// up and the canvas has drawn it.
pub fn playback(
    mut editor: ResMut<Editor>,
    mut timeline: ResMut<Timeline>,
    layers: Res<Layers>,
    wake: Option<Res<WakeDeadline>>,
) {
    let tl = &mut *timeline;
    let pb = &mut tl.playback;
    if !pb.playing {
        return;
    }
    let now = Instant::now();
    let Some(due) = pb.due else {
        pb.due = Some(now + frame_time(&editor.doc, editor.frame));
        return;
    };
    // A frame waits for the canvas to finish the last one, but never more
    // than a moment past its time.
    let drawn = layers.pending() == 0 || now >= due + Duration::from_secs(2);
    if now >= due && drawn {
        match pb.next(editor.frame) {
            Some(f) => {
                editor.go_to_frame(f);
                pb.shown.push_back(now);
                while pb
                    .shown
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
                    && pb.shown.len() > 2
                {
                    pb.shown.pop_front();
                }
                // The next frame is due a duration after this one was due,
                // which keeps the document's pace; a frame that showed more
                // than a frame late starts the count again from now.
                let next = due + frame_time(&editor.doc, editor.frame);
                pb.due = Some(if next < now {
                    now + frame_time(&editor.doc, editor.frame)
                } else {
                    next
                });
            }
            None => pb.stop(),
        }
    }
    if let (Some(wake), Some(due)) = (wake, pb.due) {
        wake.request(due.max(now + Duration::from_millis(1)));
    }
}

// ---- what the timeline shows ----

/// The tag lanes: each tag on the first lane where it overlaps no other.
fn lanes(tags: &[Tag]) -> Vec<Vec<usize>> {
    let mut lanes: Vec<Vec<usize>> = Vec::new();
    for (i, t) in tags.iter().enumerate() {
        let free = lanes.iter().position(|lane| {
            lane.iter()
                .all(|&j| tags[j].to < t.from || tags[j].from > t.to)
        });
        match free {
            Some(l) => lanes[l].push(i),
            None => lanes.push(vec![i]),
        }
    }
    lanes
}

/// A cel's marks, as bits of its class list.
mod mark {
    pub const ON: u16 = 1;
    pub const OFF: u16 = 1 << 1;
    pub const LINK_L: u16 = 1 << 2;
    pub const LINK_R: u16 = 1 << 3;
    pub const GROUP: u16 = 1 << 4;
    pub const REFERENCE: u16 = 1 << 5;
    pub const SKELETON: u16 = 1 << 6;
    pub const SEL: u16 = 1 << 7;
    pub const CUR: u16 = 1 << 8;
    pub const GENERATED: u16 = 1 << 9;
    pub const ACTIVE: u16 = 1 << 10;
    pub const HIDDEN: u16 = 1 << 11;
}

/// What the grid shows. Its shape (the tags, the rows and the number of
/// frames) takes new markup to change; the marks of a cel, a frame number or
/// a layer name change by setting that one element's classes, so a stroke,
/// a step to another frame or a new selection restyles only what changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridModel {
    /// The tag lanes' markup, and each row's layer, depth and name.
    shape: (String, Vec<(u32, usize, String)>, usize),
    frames: Vec<u16>,
    names: Vec<u16>,
    /// A row per layer, a mark set per frame.
    cells: Vec<Vec<u16>>,
}

fn cel_class(m: u16) -> String {
    use mark::*;
    let mut c = String::from("cel");
    for (bit, name) in [
        (ON, " on"),
        (OFF, " off"),
        (LINK_L, " link-l"),
        (LINK_R, " link-r"),
        (GROUP, " group"),
        (REFERENCE, " reference"),
        (SKELETON, " skeleton"),
        (GENERATED, " generated"),
        (SEL, " sel"),
        (CUR, " cur"),
    ] {
        if m & bit != 0 {
            c.push_str(name);
        }
    }
    c
}

fn frame_class(m: u16) -> String {
    let mut c = String::from("tl-frame");
    if m & mark::SEL != 0 {
        c.push_str(" sel");
    }
    if m & mark::CUR != 0 {
        c.push_str(" cur");
    }
    if m & mark::GENERATED != 0 {
        c.push_str(" generated");
    }
    c
}

fn name_class(depth: usize, m: u16) -> String {
    let mut c = format!("tl-name depth-{}", depth.min(4));
    if m & mark::ACTIVE != 0 {
        c.push_str(" active");
    }
    if m & mark::HIDDEN != 0 {
        c.push_str(" hidden");
    }
    c
}

fn tags_markup(doc: &Document) -> String {
    let n = doc.frame_count();
    let mut grid = String::new();
    for (k, lane) in lanes(doc.tags()).into_iter().enumerate() {
        grid.push_str(&format!(r#"<row id="tl-lane-{k}" class="tl-row tags">"#));
        let mut f = 0;
        while f < n {
            match lane.iter().find(|&&i| doc.tags()[i].from == f) {
                Some(&i) => {
                    // One band over the tag's frames, a cell per frame to
                    // take the pointer, and the name over them.
                    let t = &doc.tags()[i];
                    let to = t.to.min(n - 1);
                    grid.push_str(&format!(
                        r#"<row class="tag-band" bg="{}">"#,
                        hex(Rgba { a: 255, ..t.color })
                    ));
                    for g in f..=to {
                        grid.push_str(&format!(r#"<tile id="tl-tag-{i}-{g}" class="tag-cell"/>"#));
                    }
                    grid.push_str(&format!(
                        r#"<label class="tag-name" text="{}"/></row>"#,
                        escape(&t.name)
                    ));
                    f = to + 1;
                }
                None => {
                    grid.push_str(r#"<tile class="tag-gap"/>"#);
                    f += 1;
                }
            }
        }
        grid.push_str("</row>");
    }
    grid
}

/// The cels a rig drew: the skeleton's own row and every layer bound to it,
/// on each frame the rig rendered.
fn generated(doc: &Document) -> BTreeSet<(LayerId, usize)> {
    let mut out = BTreeSet::new();
    for (_, layer) in doc.walk() {
        let LayerKind::Skeleton { rig } = &layer.kind else {
            continue;
        };
        let bound: BTreeSet<LayerId> = rig.bindings().iter().map(|b| b.layer).collect();
        for (f, frame) in rig.frames().iter().enumerate() {
            if frame.generated {
                out.insert((layer.id, f));
                out.extend(bound.iter().map(|l| (*l, f)));
            }
        }
    }
    out
}

/// What the grid shows of the document, the selection and the current frame.
pub fn grid_model(editor: &Editor, tl: &Timeline, tree: &LayerTree) -> GridModel {
    use mark::*;
    let doc = &editor.doc;
    let n = doc.frame_count();
    let rows = rows(doc, tree);
    let (sa, sb) = tl.frames(editor);
    // The selected rows, top first, as positions in `rows`.
    let pos = |id: LayerId| rows.iter().position(|(_, l)| l.id == id);
    let sel_rows = tl.selection.and_then(|s| {
        let (a, b) = (pos(s.anchor.0)?, pos(s.focus.0)?);
        Some(a.min(b)..=a.max(b))
    });
    let columns = tl
        .selection
        .is_some_and(|s| rows.len() <= 1 || s.anchor.0 != s.focus.0 || sa != sb);
    let made = generated(doc);
    let frame_made: BTreeSet<usize> = made.iter().map(|(_, f)| *f).collect();
    let frames = (0..n)
        .map(|f| {
            let mut m = 0;
            if columns && (sa..=sb).contains(&f) {
                m |= SEL;
            }
            if f == editor.frame {
                m |= CUR;
            }
            if frame_made.contains(&f) {
                m |= GENERATED;
            }
            m
        })
        .collect();
    let mut names = Vec::with_capacity(rows.len());
    let mut cells = Vec::with_capacity(rows.len());
    for (r, (_, layer)) in rows.iter().enumerate() {
        let mut nm = 0;
        if layer.id == editor.layer {
            nm |= ACTIVE;
        }
        if !layer.props.visible {
            nm |= HIDDEN;
        }
        names.push(nm);
        let selected = tl.selection.is_some() && sel_rows.as_ref().is_some_and(|s| s.contains(&r));
        let row = (0..n)
            .map(|f| {
                let mut m = match &layer.kind {
                    LayerKind::Raster { cels, .. } => {
                        let image = |f: usize| cels.get(f).copied().flatten().map(|c| c.image);
                        match image(f) {
                            Some(img) => {
                                let mut m = ON;
                                if f > 0 && image(f - 1) == Some(img) {
                                    m |= LINK_L;
                                }
                                if image(f + 1) == Some(img) {
                                    m |= LINK_R;
                                }
                                m
                            }
                            None => OFF,
                        }
                    }
                    LayerKind::Group { .. } => GROUP,
                    LayerKind::Reference { .. } => REFERENCE,
                    LayerKind::Skeleton { .. } => SKELETON,
                };
                if selected && (sa..=sb).contains(&f) {
                    m |= SEL;
                }
                if f == editor.frame {
                    m |= CUR;
                }
                if made.contains(&(layer.id, f)) {
                    m |= GENERATED;
                }
                m
            })
            .collect();
        cells.push(row);
    }
    let shape = (
        tags_markup(doc),
        rows.iter()
            .map(|(d, l)| (l.id.0, *d, l.props.name.clone()))
            .collect(),
        n,
    );
    GridModel {
        shape,
        frames,
        names,
        cells,
    }
}

impl GridModel {
    /// The grid's markup and the layer names' markup.
    pub fn markup(&self) -> (String, String) {
        let (tags, rows, n) = &self.shape;
        let lanes = tags.matches("tl-lane-").count();
        let mut names = r#"<row class="tl-spacer"/>"#.repeat(lanes);
        let mut grid = tags.clone();
        names.push_str(r#"<row class="tl-spacer"/>"#);
        grid.push_str(r#"<row class="tl-row frames">"#);
        for f in 0..*n {
            grid.push_str(&format!(
                r#"<tile id="tl-frame-{f}" class="{}"><label class="tl-frame-num" text="{}"/></tile>"#,
                frame_class(self.frames[f]),
                f + 1
            ));
        }
        grid.push_str("</row>");
        for (r, (id, depth, name)) in rows.iter().enumerate() {
            names.push_str(&format!(
                r#"<row id="tl-layer-{id}" class="{}"><label class="tl-name-text" text="{}"/></row>"#,
                name_class(*depth, self.names[r]),
                escape(name)
            ));
            grid.push_str(r#"<row class="tl-row">"#);
            for f in 0..*n {
                grid.push_str(&format!(
                    r#"<tile id="cel-{id}-{f}" class="{}"><tile class="cel-dot"/></tile>"#,
                    cel_class(self.cells[r][f])
                ));
            }
            grid.push_str("</row>");
        }
        (grid, names)
    }

    /// The elements whose classes differ from `old`'s, with their classes,
    /// when the two have the same shape.
    pub fn changes(&self, old: &GridModel) -> Option<Vec<(String, String)>> {
        if self.shape != old.shape {
            return None;
        }
        let mut out = Vec::new();
        for (f, (a, b)) in self.frames.iter().zip(&old.frames).enumerate() {
            if a != b {
                out.push((format!("tl-frame-{f}"), frame_class(*a)));
            }
        }
        for (r, (id, depth, _)) in self.shape.1.iter().enumerate() {
            if self.names[r] != old.names[r] {
                out.push((format!("tl-layer-{id}"), name_class(*depth, self.names[r])));
            }
            for (f, (a, b)) in self.cells[r].iter().zip(&old.cells[r]).enumerate() {
                if a != b {
                    out.push((format!("cel-{id}-{f}"), cel_class(*a)));
                }
            }
        }
        Some(out)
    }
}

/// Keeps the timeline showing the document, the selection and playback.
pub fn show_timeline(
    editor: Res<Editor>,
    mut timeline: ResMut<Timeline>,
    tree: Res<LayerTree>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let tl = &mut *timeline;
    let mut changed = false;
    // The grid is looked at again when what it shows may have changed, and
    // only the elements whose marks differ are written.
    let key = {
        let mut h = DefaultHasher::new();
        (
            editor.generation,
            editor.revision,
            editor.layer.0,
            editor.frame,
        )
            .hash(&mut h);
        editor.doc.frame_count().hash(&mut h);
        tl.selection.hash(&mut h);
        tree.collapsed.hash(&mut h);
        h.finish()
    };
    if tl.grid != Some(key)
        && let (Some(grid_node), Some(names_node)) =
            (dom::get_by_id("tl-grid"), dom::get_by_id("tl-names"))
    {
        let model = grid_model(&editor, tl, &tree);
        match tl.shown.as_ref().and_then(|old| model.changes(old)) {
            Some(edits) => {
                for (id, class) in &edits {
                    if let Some(node) = dom::get_by_id(id) {
                        node.set_class(class);
                    }
                }
                changed |= !edits.is_empty();
            }
            None => {
                let (grid, names) = model.markup();
                grid_node.set_inner_markup(&grid);
                names_node.set_inner_markup(&names);
                changed = true;
            }
        }
        tl.shown = Some(model);
        tl.grid = Some(key);
    }
    let n = editor.doc.frame_count();
    set_if_changed(
        &mut signals,
        "tl_frame_label",
        format!("Frame {} of {n}", editor.frame + 1),
    );
    set_if_changed(
        &mut signals,
        "tl_play_label",
        if tl.playback.playing { "Pause" } else { "Play" }.to_string(),
    );
    let fps = match (tl.playback.playing, tl.playback.fps()) {
        (true, Some(fps)) => format!("{fps:.1} fps"),
        _ => String::new(),
    };
    set_if_changed(&mut signals, "tl_fps", fps);
    // The controls follow the editor when something else moved them.
    let (a, _) = tl.frames(&editor);
    let duration = editor
        .doc
        .frames()
        .get(a)
        .map_or(100, |f| f.duration_ms)
        .to_string();
    let now = Controls {
        mode: tl.playback.mode,
        loop_tag: tl.playback.loop_tag,
        onion: tl.onion.clone(),
        duration,
    };
    if tl.controls.as_ref() != Some(&now) {
        let o = &now.onion;
        signals.set("tl_mode", now.mode.name());
        signals.set("tl_loop_tag", now.loop_tag);
        signals.set("onion_on", o.on);
        signals.set("onion_tint", o.tint);
        signals.set("onion_above", o.above);
        signals.set("onion_before", o.before.to_string());
        signals.set("onion_after", o.after.to_string());
        signals.set("onion_opacity", f64::from(o.opacity));
        signals.set("onion_falloff", f64::from(o.falloff));
        if tl.controls.as_ref().map(|c| &c.duration) != Some(&now.duration) {
            signals.set("tl_duration", now.duration.clone());
        }
        tl.controls = Some(now);
        changed = true;
    }
    if changed {
        crate::repaint_soon(&wake);
    }
}

/// Hands the onion skin to the canvas view, with the theme's tints.
pub fn onion_to_view(
    timeline: Res<Timeline>,
    colors: Res<crate::view::ViewColors>,
    mut layers: ResMut<Layers>,
    mut tints: Local<(Option<Rgba>, Option<Rgba>, u64)>,
) {
    if tints.2 != colors.revision || tints.0.is_none() {
        let probe = |id: &str| {
            dom::get_by_id(id)
                .and_then(|n| n.computed_style_of("bg"))
                .as_deref()
                .and_then(parse_hex)
        };
        *tints = (
            probe("probe-onion-before"),
            probe("probe-onion-after"),
            colors.revision,
        );
    }
    // The skin steps aside while the animation plays: playback shows the
    // frames as they are, and the ghosts of a large document would hold
    // each frame back while they composite.
    let skin = if timeline.playback.playing {
        None
    } else {
        timeline.onion.skin((tints.0, tints.1))
    };
    if layers.onion != skin {
        layers.onion = skin;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_core::ColorMode;

    fn editor(frames: usize) -> Editor {
        let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
        for f in 1..frames {
            doc.insert_frame(f, 100).unwrap();
        }
        Editor::new(doc)
    }

    fn run(pb: &mut Playback, editor: &mut Editor, steps: usize) -> Vec<usize> {
        let mut out = vec![editor.frame];
        for _ in 0..steps {
            match pb.next(editor.frame) {
                Some(f) => {
                    editor.frame = f;
                    out.push(f);
                }
                None => break,
            }
        }
        out
    }

    #[test]
    fn loops_play_once_and_ping_pong() {
        let mut e = editor(3);
        let mut pb = Playback::default();
        pb.start(&mut e, Instant::now());
        assert_eq!(run(&mut pb, &mut e, 5), [0, 1, 2, 0, 1, 2]);
        e.frame = 0;
        pb.mode = PlayMode::Once;
        pb.start(&mut e, Instant::now());
        assert_eq!(run(&mut pb, &mut e, 5), [0, 1, 2]);
        e.frame = 0;
        pb.mode = PlayMode::PingPong;
        pb.start(&mut e, Instant::now());
        assert_eq!(run(&mut pb, &mut e, 6), [0, 1, 2, 1, 0, 1, 2]);
    }

    #[test]
    fn a_tag_decides_range_direction_and_repeats() {
        let mut e = editor(6);
        e.doc.tags_mut().push(Tag {
            name: "t".into(),
            from: 2,
            to: 4,
            direction: LoopDirection::Reverse,
            repeat: 2,
            color: Rgba::WHITE,
        });
        e.frame = 3;
        let mut pb = Playback {
            loop_tag: true,
            ..Playback::default()
        };
        pb.start(&mut e, Instant::now());
        assert_eq!(run(&mut pb, &mut e, 10), [3, 2, 4, 3, 2]);
    }

    #[test]
    fn copied_and_moved_frames_land_after_the_range() {
        let mut e = editor(3);
        let id = e.layer;
        let images: Vec<_> = (0..3).map(|f| e.doc.target_image(id, f).unwrap()).collect();
        e.duplicate_frames(0, 1);
        assert_eq!(e.doc.frame_count(), 5);
        assert_eq!(e.frame, 2);
        assert_eq!(e.doc.cel(id, 4).unwrap().unwrap().image, images[2]);
        e.move_frames(4, 1, 0);
        assert_eq!(e.doc.cel(id, 0).unwrap().unwrap().image, images[2]);
        e.delete_frames(0, 4);
        assert_eq!(e.doc.frame_count(), 1, "the last frame stays");
    }

    #[test]
    fn tags_take_lanes_where_they_overlap() {
        let tag = |from, to| Tag {
            name: String::new(),
            from,
            to,
            direction: LoopDirection::Forward,
            repeat: 0,
            color: Rgba::WHITE,
        };
        let tags = [tag(0, 2), tag(3, 4), tag(1, 3)];
        assert_eq!(lanes(&tags), vec![vec![0, 1], vec![2]]);
    }

    #[test]
    fn ids_name_their_spots() {
        assert_eq!(Spot::parse("cel-3-12"), Some(Spot::Cel(LayerId(3), 12)));
        assert_eq!(Spot::parse("tl-frame-4"), Some(Spot::Frame(4)));
        assert_eq!(Spot::parse("tl-tag-1-5"), Some(Spot::Tag(1, 5)));
        assert_eq!(Spot::parse("tl-layer-7"), Some(Spot::Layer(LayerId(7))));
        assert_eq!(Spot::parse("tl-play"), None);
    }

    #[test]
    fn a_stroke_on_a_cel_with_pixels_writes_nothing() {
        let mut e = editor(3);
        let tl = Timeline::default();
        let tree = LayerTree::default();
        let layer = e.layer;
        let t = e.doc.target(layer, 0).unwrap();
        t.raster
            .fill_rect_raw(&pixl_core::Rect::new(0, 0, 1, 1), Rgba::BLACK.to_u32());
        let before = grid_model(&e, &tl, &tree);
        let t = e.doc.target(layer, 0).unwrap();
        t.raster
            .fill_rect_raw(&pixl_core::Rect::new(1, 1, 1, 1), Rgba::BLACK.to_u32());
        e.touch_pixels();
        let after = grid_model(&e, &tl, &tree);
        assert_eq!(after.changes(&before), Some(Vec::new()));
    }

    #[test]
    fn a_step_to_another_frame_writes_the_two_columns() {
        let mut e = editor(3);
        let tl = Timeline::default();
        let tree = LayerTree::default();
        let before = grid_model(&e, &tl, &tree);
        e.frame = 2;
        let after = grid_model(&e, &tl, &tree);
        let mut ids: Vec<String> = after
            .changes(&before)
            .expect("the same shape")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ids.sort();
        let l = e.layer.0;
        assert_eq!(
            ids,
            [
                format!("cel-{l}-0"),
                format!("cel-{l}-2"),
                "tl-frame-0".into(),
                "tl-frame-2".into()
            ]
        );
        let (grid, _) = after.markup();
        assert!(grid.contains(&format!(r#"id="cel-{l}-2" class="cel off cur""#)));
    }

    #[test]
    fn a_new_frame_or_layer_takes_new_markup() {
        let mut e = editor(2);
        let tl = Timeline::default();
        let tree = LayerTree::default();
        let before = grid_model(&e, &tl, &tree);
        e.doc.insert_frame(2, 100).unwrap();
        assert_eq!(grid_model(&e, &tl, &tree).changes(&before), None);
        let before = grid_model(&e, &tl, &tree);
        e.add_layer();
        assert_eq!(grid_model(&e, &tl, &tree).changes(&before), None);
    }

    #[test]
    fn frames_a_rig_rendered_are_marked_on_its_layers() {
        let mut e = editor(3);
        let art = e.layer;
        let skel = e
            .doc
            .add_layer(
                pixl_core::NewLayer::Skeleton,
                "Rig",
                pixl_core::Placement::default(),
            )
            .unwrap();
        let rig = e.doc.rig_mut(skel).unwrap();
        let bone = rig
            .add_bone(
                "arm",
                None,
                pixl_core::Vec2::new(1.0, 1.0),
                pixl_core::Vec2::new(4.0, 1.0),
            )
            .unwrap();
        rig.bind(pixl_core::rig::Binding {
            layer: art,
            region: None,
            bone,
            pivot: pixl_core::Vec2::new(1.0, 1.0),
        })
        .unwrap();
        rig.frame_mut(2).unwrap().generated = true;
        let model = grid_model(&e, &Timeline::default(), &LayerTree::default());
        let (grid, _) = model.markup();
        for id in [format!("cel-{}-2", art.0), format!("cel-{}-2", skel.0)] {
            let class = grid
                .split(&format!(r#"id="{id}" class=""#))
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap();
            assert!(class.contains("generated"), "{id}: {class}");
        }
        assert!(grid.contains(r#"id="tl-frame-2" class="tl-frame generated""#));
        assert!(!grid.contains(&format!(r#"id="cel-{}-1" class="cel off generated"#, art.0)));
    }
}
