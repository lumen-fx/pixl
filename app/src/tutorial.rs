//! Guided tutorials: each item under the Help menu's tutorials opens a
//! lesson, a document pixl ships, and a card over the canvas that walks
//! through one workflow. Each step says what to do and where, outlines the
//! control it uses next, and ticks itself off when the editor shows it done;
//! the next step follows on its own. The lessons are in [`crate::lessons`].
//!
//! Closing the card keeps the lesson's place, and the lesson's Help item
//! shows it again while the lesson is open. Opening another document ends
//! the lesson.

use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::LayerId;

use crate::editor::Editor;
use crate::files::{Files, Replace};
use crate::home::Home;
use crate::ids::ElementIds;
use crate::keymap::KeyCommands;
use crate::lessons::LESSONS;
use crate::panels::escape;
use crate::rig::RigUi;
use crate::shipped::Shipped;
use crate::timeline::Timeline;
use crate::tools::set_if_changed;
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

/// What a step's check looks at.
pub struct Seen<'a> {
    pub editor: &'a Editor,
    pub rig: &'a RigUi,
    pub timeline: &'a Timeline,
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
    (rig, timeline, home): (Res<RigUi>, Res<Timeline>, Res<Home>),
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
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    for lesson in LESSONS {
        if keys.ran(lesson.id, false) {
            chosen.push(lesson.id.to_string());
        }
    }
    chosen.extend(clicks.read().filter_map(|c| ids.id_of(c.entity)));
    // A shipped document just opened: the lesson over it starts.
    if let Some((which, generation)) = files.shipped.take()
        && let Some(i) = LESSONS.iter().position(|l| l.document == which)
    {
        t.lesson = Some((i, generation));
        t.step = 0;
        t.open = true;
        if let Some(mut ws) = workspace
            && ws.live.name != LESSONS[i].layout
        {
            ws.switch(LESSONS[i].layout);
        }
    }
    if !home.open {
        for id in chosen {
            if id == "tutor-close" {
                t.open = false;
            } else if let Some(lesson) = LESSONS.into_iter().find(|l| l.id == id) {
                if t.running(&editor).is_some_and(|l| l.id == lesson.id) {
                    t.open = true;
                } else {
                    files.queued = Some(Replace::Shipped(lesson.document));
                }
            }
        }
    }
    let running = t.running(&editor);
    let seen = Seen {
        editor: &editor,
        rig: &rig,
        timeline: &timeline,
    };
    if let Some(lesson) = running {
        while t.step < lesson.steps.len() && (lesson.steps[t.step].done)(&seen) {
            t.step += 1;
        }
    }
    let open = running.is_some() && t.open && !home.open;
    let mut changed = false;
    if signals.get::<bool>("tutor_open") != Some(open) {
        signals.set("tutor_open", open);
        changed = true;
    }
    let target = running
        .and_then(|l| l.steps.get(t.step))
        .filter(|_| open)
        .and_then(|s| (s.target)(&seen));
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
                .after(crate::timeline::show_timeline)
                .after(crate::layers::show_layers)
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
        let m = steps_markup(crate::lessons::RIG.steps, 2);
        assert!(m.contains(r#"id="tutor-step-1" class="tutor-step done""#));
        assert!(m.contains(r#"id="tutor-step-2" class="tutor-step current""#));
        assert!(m.contains(r#"id="tutor-step-3" class="tutor-step todo""#));
    }
}
