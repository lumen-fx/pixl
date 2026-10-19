//! Guided tutorials: each item under the Help menu's tutorials opens a
//! lesson, a document pixl ships, and a card over the canvas that walks
//! through one workflow. Each step says what to do and where, outlines the
//! control it uses next, and ticks itself off when the editor shows it done,
//! and the next step follows on its own. A step done by a stroke or a drag
//! ticks off when it ends. The start screen lists Getting started as well.
//! The lessons are in [`crate::tutorial::lessons`].
//!
//! Closing the card keeps the lesson's place, and the lesson's Help item
//! shows it again while the lesson is open. Opening another document ends
//! the lesson.

pub mod lessons;

use lumenui::glam::Vec2;
use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_document::{Document, LayerId, Rgba};

use crate::editor::{Editor, Tool};
use crate::files::shipped::Shipped;
use crate::files::{Files, Request};
use crate::home::Home;
use crate::ids::ElementIds;
use crate::panels::escape;
use crate::panels::timeline::Timeline;
use crate::rig::RigUi;
use crate::settings::keymap::KeyCommands;
use crate::tools::toolbox::Toolbox;
use crate::tools::{Gesture, set_if_changed};
use crate::tutorial::lessons::LESSONS;
use crate::view::View;
use crate::workspace::Workspace;

/// The class that outlines the control a step uses.
const TARGET: &str = "tutor-target";

/// Controls that sit below the fold of a scroll, and the scroll's signal:
/// outlining one scrolls to the end, where it is.
const BELOW_FOLD: [(&str, &str); 1] = [("rig-render-keyed", "rig_scroll")];

/// A lesson: the document it opens, the layout it works in, and its steps.
pub struct Lesson {
    /// The command that opens it: its Help menu item, and the id a key
    /// binds.
    pub id: &'static str,
    /// Its name in the Help menu and on its card.
    pub title: &'static str,
    /// The document it works on, opened as an untitled copy.
    pub document: Shipped,
    /// The workspace layout it switches to, which gives its panels room.
    pub layout: &'static str,
    pub steps: &'static [Step],
    /// What the card says once every step is done.
    pub done: &'static str,
}

/// The editor at a moment a step compares with: when the lesson started,
/// and when the current step did.
#[derive(Clone, Debug)]
pub struct Mark {
    pub doc: Document,
    pub fg: Rgba,
    /// The edits there were to undo and to redo.
    pub undo: usize,
    pub redo: usize,
    /// How many times the view had zoomed, and panned.
    pub zooms: u64,
    pub pans: u64,
}

impl Mark {
    fn of(editor: &Editor, motion: &Motion) -> Self {
        Self {
            doc: editor.doc.clone(),
            fg: editor.fg,
            undo: editor.history.undo_labels().count(),
            redo: editor.history.redo_labels().count(),
            zooms: motion.zooms,
            pans: motion.pans,
        }
    }
}

/// How the view has moved: a tick whose zoom changed counts as a zoom, and
/// one whose zoom and stage stayed but whose origin moved counts as a pan.
#[derive(Clone, Copy, Debug, Default)]
struct Motion {
    zooms: u64,
    pans: u64,
    /// The view's zoom, origin and stage on the last tick.
    last: Option<Place>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    zoom: f64,
    origin: (f64, f64),
    stage: (Vec2, Vec2),
}

impl Motion {
    fn watch(&mut self, view: &View) {
        let now = Place {
            zoom: view.zoom,
            origin: view.origin,
            stage: view.stage,
        };
        if let Some(last) = self.last {
            if last.zoom != now.zoom {
                self.zooms += 1;
            } else if last.stage == now.stage && last.origin != now.origin {
                self.pans += 1;
            }
        }
        self.last = Some(now);
    }
}

/// What a step's check looks at.
pub struct Seen<'a> {
    pub editor: &'a Editor,
    pub rig: &'a RigUi,
    pub timeline: &'a Timeline,
    pub signals: &'a Signals<'a>,
    /// The editor when the lesson started, and when the current step did.
    pub start: &'a Mark,
    pub since: &'a Mark,
    /// How many times the view has zoomed, and panned.
    pub zooms: u64,
    pub pans: u64,
}

impl Seen<'_> {
    /// The lesson's raster layer with this name.
    pub fn layer(&self, name: &str) -> Option<LayerId> {
        let doc = &self.editor.doc;
        doc.walk()
            .into_iter()
            .find(|(_, l)| l.props.name == name && l.is_raster())
            .map(|(_, l)| l.id)
    }

    /// The first raster layer the lesson's document did not start with.
    pub fn added(&self) -> Option<LayerId> {
        let before = self.start.doc.walk();
        self.editor
            .doc
            .walk()
            .into_iter()
            .filter(|(_, l)| l.is_raster())
            .map(|(_, l)| l.id)
            .find(|id| before.iter().all(|(_, l)| l.id != *id))
    }

    /// Whether `layer` holds pixels on `frame`.
    pub fn painted(&self, layer: LayerId, frame: usize) -> bool {
        self.editor
            .doc
            .cel_raster(layer, frame)
            .ok()
            .flatten()
            .is_some_and(|r| !r.content_bounds().is_empty())
    }

    /// The labels of the edits made since the current step started.
    pub fn edits(&self) -> Vec<&str> {
        self.editor
            .history
            .undo_labels()
            .skip(self.since.undo)
            .collect()
    }

    /// Whether an edit labelled `label` was made since the step started.
    pub fn edited(&self, label: &str) -> bool {
        self.edits().contains(&label)
    }

    /// Whether the signal `name` holds true, as a dialog's open flag does.
    pub fn flag(&self, name: &str) -> bool {
        self.signals.get::<bool>(name) == Some(true)
    }

    /// `then` while `tool` is the tool in use; until then its button.
    pub fn with_tool(&self, tool: Tool, then: Option<String>) -> Option<String> {
        if self.editor.tool == tool {
            return then;
        }
        Some(tool.button_id().to_string())
    }
}

/// One step of a lesson.
pub struct Step {
    /// A few words for the step list.
    pub title: &'static str,
    /// What to do, and where.
    pub text: &'static str,
    /// The id of the control the step uses next, outlined while it is
    /// current.
    pub target: fn(&Seen) -> Option<String>,
    /// Whether the editor shows the step done.
    pub done: fn(&Seen) -> bool,
}

/// The tutorial's state.
#[derive(Resource, Default, Debug)]
pub struct Tutorial {
    /// The lesson running, by its place in [`LESSONS`], and the generation
    /// of the document it runs over.
    pub lesson: Option<(usize, u64)>,
    /// The current step; the lesson's step count once all are done.
    pub step: usize,
    /// Whether the card shows.
    pub open: bool,
    /// The step the list was last written for, and the control outlined.
    shown: Option<usize>,
    target: Option<String>,
    /// The editor when the lesson started and when the step did, and how
    /// the view has moved.
    marks: Option<(Mark, Mark)>,
    motion: Motion,
}

impl Tutorial {
    /// The lesson open in the editor, if one is.
    pub fn running(&self, editor: &Editor) -> Option<&'static Lesson> {
        let (i, generation) = self.lesson?;
        (generation == editor.generation).then(|| LESSONS[i])
    }

    /// The id of the control outlined for the current step.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }
}

/// The step list, each step marked done, current or still to come.
fn steps_markup(steps: &[Step], step: usize) -> String {
    let mut out = String::new();
    for (i, s) in steps.iter().enumerate() {
        let class = match i.cmp(&step) {
            std::cmp::Ordering::Less => "done",
            std::cmp::Ordering::Equal => "current",
            std::cmp::Ordering::Greater => "todo",
        };
        out.push_str(&format!(
            r#"<row id="tutor-step-{i}" class="tutor-step {class}"><tile class="tutor-mark"/><label class="tutor-step-name" text="{}"/></row>"#,
            escape(s.title)
        ));
    }
    out
}

/// Opens and closes the card, starts a lesson, ticks off the steps the
/// editor shows done and points at the next one's control.
#[allow(clippy::too_many_arguments)]
pub fn run_tutorial(
    mut tutorial: ResMut<Tutorial>,
    editor: Res<Editor>,
    (rig, timeline, home, view): (Res<RigUi>, Res<Timeline>, Res<Home>, Res<View>),
    (gesture, toolbox): (Res<Gesture>, Res<Toolbox>),
    mut files: ResMut<Files>,
    workspace: Option<ResMut<Workspace>>,
    (mut menu, mut clicks, ids, keys): (
        MessageReader<MenuClicked>,
        MessageReader<ClickEvent>,
        ElementIds,
        Res<KeyCommands>,
    ),
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let t = &mut *tutorial;
    t.motion.watch(&view);
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    for lesson in LESSONS {
        if keys.ran(lesson.id, false) {
            chosen.push(lesson.id.to_string());
        }
    }
    let clicked: Vec<String> = clicks.read().filter_map(|c| ids.id_of(c.entity)).collect();
    // A shipped document just opened: the lesson over it starts.
    if let Some((which, generation)) = files.shipped.take()
        && let Some(i) = LESSONS.iter().position(|l| l.document == which)
    {
        t.lesson = Some((i, generation));
        t.step = 0;
        t.open = true;
        let mark = Mark::of(&editor, &t.motion);
        t.marks = Some((mark.clone(), mark));
        if let Some(mut ws) = workspace
            && ws.live.name != LESSONS[i].layout
        {
            ws.switch(LESSONS[i].layout);
        }
    }
    if home.open {
        // The start screen lists some lessons under the name of their
        // document.
        for id in &clicked {
            if let Some(lesson) = LESSONS
                .into_iter()
                .find(|l| id.strip_prefix("home-") == Some(l.document.slug()))
            {
                files.queued = Some(Request::Shipped(lesson.document));
            }
        }
    } else {
        chosen.extend(clicked);
        for id in chosen {
            if id == "tutor-close" {
                t.open = false;
            } else if let Some(lesson) = LESSONS.into_iter().find(|l| l.id == id) {
                if t.running(&editor).is_some_and(|l| l.id == lesson.id) {
                    t.open = true;
                } else {
                    files.queued = Some(Request::Shipped(lesson.document));
                }
            }
        }
    }
    let running = t.running(&editor);
    let open = running.is_some() && t.open && !home.open;
    // A step ticks off once the stroke or drag that does it ends.
    let busy = gesture.active() || toolbox.active();
    let target = match (running, &mut t.marks) {
        (Some(lesson), Some((start, since))) => loop {
            let seen = Seen {
                editor: &editor,
                rig: &rig,
                timeline: &timeline,
                signals: &signals,
                start,
                since,
                zooms: t.motion.zooms,
                pans: t.motion.pans,
            };
            if busy || t.step >= lesson.steps.len() || !(lesson.steps[t.step].done)(&seen) {
                break lesson
                    .steps
                    .get(t.step)
                    .filter(|_| open)
                    .and_then(|s| (s.target)(&seen));
            }
            // The next step compares with the editor as this one left it.
            t.step += 1;
            *since = Mark::of(&editor, &t.motion);
        },
        _ => None,
    };
    let mut changed = false;
    if signals.get::<bool>("tutor_open") != Some(open) {
        signals.set("tutor_open", open);
        changed = true;
    }
    if target != t.target {
        if let Some(node) = t.target.as_deref().and_then(dom::get_by_id) {
            crate::set_class(node, TARGET, false);
        }
        if let Some((_, scroll)) = BELOW_FOLD
            .iter()
            .find(|(id, _)| Some(*id) == target.as_deref())
        {
            signals.set(scroll, f64::from(u32::MAX));
        }
        t.target = target;
        changed = true;
    }
    if let Some(node) = t.target.as_deref().and_then(dom::get_by_id) {
        crate::set_class(node, TARGET, true);
    }
    let Some(lesson) = running.filter(|_| open) else {
        t.shown = None;
        if changed {
            crate::repaint_soon(&wake);
        }
        return;
    };
    if t.shown != Some(t.step)
        && let Some(list) = dom::get_by_id("tutor-steps")
    {
        list.set_inner_markup(&steps_markup(lesson.steps, t.step));
        t.shown = Some(t.step);
        changed = true;
    }
    set_if_changed(&mut signals, "tutor_title", lesson.title.to_string());
    let text = lesson.steps.get(t.step).map_or(lesson.done, |s| s.text);
    set_if_changed(&mut signals, "tutor_text", text.to_string());
    let count = if t.step < lesson.steps.len() {
        format!("Step {} of {}", t.step + 1, lesson.steps.len())
    } else {
        "All done".to_string()
    };
    set_if_changed(&mut signals, "tutor_count", count);
    if changed {
        crate::repaint_soon(&wake);
    }
}

/// The tutorial's resource and system, after the commands and the panels
/// it watches.
pub fn install(app: App) -> App {
    use lumenui::lumen_core::input::ClickSet;
    use lumenui::lumen_core::signals;
    app.insert_resource(Tutorial::default())
        .insert_signal("tutor_open", false)
        .insert_signal("tutor_title", "")
        .insert_signal("tutor_text", "")
        .insert_signal("tutor_count", "")
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("tutorial::run_tutorial", run_tutorial)
                .after(crate::files::file_commands)
                .after(crate::rig::show_rig)
                .after(crate::panels::timeline::show_timeline)
                .after(crate::panels::layers::show_layers)
                .after(ClickSet::Producers)
                .before(signals::apply_text_bindings)
                .before(signals::apply_checked_bindings)
                .before(signals::apply_value_bindings),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_marks_steps_done_current_and_to_come() {
        let m = steps_markup(crate::tutorial::lessons::RIG.steps, 2);
        assert!(m.contains(r#"id="tutor-step-1" class="tutor-step done""#));
        assert!(m.contains(r#"id="tutor-step-2" class="tutor-step current""#));
        assert!(m.contains(r#"id="tutor-step-3" class="tutor-step todo""#));
    }
}
