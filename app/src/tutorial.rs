//! The rig tutorial: Help > Rig tutorial opens the rig lesson, a character
//! with no skeleton, and a card over the canvas that walks through rigging
//! its arm, posing it on two frames and playing the result. Each step says
//! what to do and where, outlines the control it uses, and ticks itself off
//! when the editor shows it done; the next step follows on its own.
//!
//! Closing the card keeps the lesson's place, and Help > Rig tutorial shows
//! it again while the lesson is open. Opening another document ends the
//! lesson.

use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::rig::Rig;

use crate::editor::Editor;
use crate::files::{Files, Replace};
use crate::home::Home;
use crate::ids::ElementIds;
use crate::keymap::KeyCommands;
use crate::panels::escape;
use crate::rig::{RigMode, RigUi, skeletons};
use crate::shipped::Shipped;
use crate::timeline::Timeline;
use crate::tools::set_if_changed;
use crate::workspace::Workspace;

/// The class that outlines the control a step uses.
const TARGET: &str = "tutor-target";

/// What a step's check looks at.
pub struct Seen<'a> {
    pub editor: &'a Editor,
    pub rig: &'a RigUi,
    pub playing: bool,
}

impl Seen<'_> {
    /// The lesson's rig, once it has a skeleton.
    fn skeleton(&self) -> Option<&Rig> {
        let doc = &self.editor.doc;
        let layer = *skeletons(doc).first()?;
        doc.rig(layer).ok()
    }

    /// The frames other than the rest frame keyed with a pose away from rest.
    fn keyed(&self) -> Vec<usize> {
        let Some(rig) = self.skeleton() else {
            return Vec::new();
        };
        rig.frames()
            .iter()
            .enumerate()
            .filter(|(f, frame)| {
                *f != rig.source_frame() && frame.key.as_ref().is_some_and(|k| !k.pose.is_rest())
            })
            .map(|(f, _)| f)
            .collect()
    }
}

/// One step of the lesson.
pub struct Step {
    /// A few words for the step list.
    pub title: &'static str,
    /// What to do, and where.
    pub text: &'static str,
    /// The id of the control the step uses, outlined while it is current.
    pub target: Option<&'static str>,
    /// Whether the editor shows the step done.
    pub done: fn(&Seen) -> bool,
}

/// The lesson's steps, in order.
pub const STEPS: [Step; 10] = [
    Step {
        title: "New skeleton",
        text: "Click New skeleton in the Rig panel on the right. A skeleton layer holds the bones.",
        target: Some("rig-new"),
        done: |s| s.skeleton().is_some(),
    },
    Step {
        title: "Bones mode",
        text: "Click Bones at the top of the Rig panel. In Bones mode a drag on the canvas draws a bone.",
        target: Some("rig-mode-bones"),
        done: |s| s.rig.mode == Some(RigMode::Bones),
    },
    Step {
        title: "Draw a bone",
        text: "On the canvas, drag from the front arm's shoulder down to its hand. The bone runs from where you press to where you let go.",
        target: None,
        done: |s| s.skeleton().is_some_and(|r| !r.bones().is_empty()),
    },
    Step {
        title: "Bind the arm",
        text: "Pick Front arm in the Layers panel, then click Bind layer in the Rig panel. The arm's pixels now follow the bone.",
        target: Some("rig-bind-layer"),
        done: |s| s.skeleton().is_some_and(|r| !r.bindings().is_empty()),
    },
    Step {
        title: "Pose mode",
        text: "Click Pose at the top of the Rig panel. In Pose mode a drag turns a bone instead of drawing one.",
        target: Some("rig-mode-pose"),
        done: |s| s.rig.mode == Some(RigMode::Pose),
    },
    Step {
        title: "Go to frame 2",
        text: "Click > in the timeline below the canvas. Frame 1 holds the art at rest, so a pose there moves the bones alone.",
        target: Some("tl-next"),
        done: |s| {
            s.skeleton()
                .is_some_and(|r| s.editor.frame != r.source_frame())
        },
    },
    Step {
        title: "Pose and key",
        text: "Drag the bone on the canvas to turn it. The arm turns with it, and letting go keys the pose on this frame.",
        target: None,
        done: |s| !s.keyed().is_empty(),
    },
    Step {
        title: "A second key",
        text: "Click >| in the timeline to go to the last frame, then drag the bone to a different pose.",
        target: Some("tl-last"),
        done: |s| {
            let keyed = s.keyed();
            let Some(rig) = s.skeleton() else {
                return false;
            };
            let pose = |f: usize| rig.frames()[f].key.as_ref().map(|k| &k.pose);
            keyed
                .iter()
                .any(|a| keyed.iter().any(|b| pose(*a) != pose(*b)))
        },
    },
    Step {
        title: "Fill the frames between",
        text: "Click Render keyed near the bottom of the Rig panel. It draws the frames between your keys, easing from one pose to the next.",
        target: Some("rig-render-keyed"),
        done: |s| {
            let keyed = s.keyed();
            let (Some(rig), Some(first), Some(last)) = (s.skeleton(), keyed.first(), keyed.last())
            else {
                return false;
            };
            (*first..=*last).all(|f| f == rig.source_frame() || rig.frames()[f].generated)
        },
    },
    Step {
        title: "Play",
        text: "Click Play in the timeline to watch the arm move. Click it again to stop.",
        target: Some("tl-play"),
        done: |s| s.playing,
    },
];

/// What the card says once every step is done.
const DONE: &str = "Done: the arm moves on its own. Help > Open rig demo shows a whole character rigged the same way.";

/// The tutorial's state.
#[derive(Resource, Default, Debug)]
pub struct Tutorial {
    /// The generation of the lesson document the tutorial runs over.
    pub lesson: Option<u64>,
    /// The current step; [`STEPS`]'s length once all are done.
    pub step: usize,
    /// Whether the card shows.
    pub open: bool,
    /// The step the list was last written for, and the control outlined.
    shown: Option<usize>,
    target: Option<&'static str>,
}

impl Tutorial {
    /// Whether a lesson is open in the editor.
    pub fn running(&self, editor: &Editor) -> bool {
        self.lesson == Some(editor.generation)
    }
}

/// The step list, each step marked done, current or still to come.
fn steps_markup(step: usize) -> String {
    let mut out = String::new();
    for (i, s) in STEPS.iter().enumerate() {
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

/// Opens and closes the card, starts the lesson, ticks off the steps the
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
    if keys.ran("help-rig-tutorial", false) {
        chosen.push("help-rig-tutorial".to_string());
    }
    chosen.extend(clicks.read().filter_map(|c| ids.id_of(c.entity)));
    if let Some(generation) = files.lesson.take() {
        t.lesson = Some(generation);
        t.step = 0;
        t.open = true;
        // The lesson works in the Rig panel, the layers and the timeline,
        // which the Rig layout gives room.
        if let Some(mut ws) = workspace
            && ws.live.name != "Rig"
        {
            ws.switch("Rig");
        }
    }
    if !home.open {
        for id in chosen {
            match id.as_str() {
                "help-rig-tutorial" if t.running(&editor) => t.open = true,
                "help-rig-tutorial" => files.queued = Some(Replace::Shipped(Shipped::RigLesson)),
                "tutor-close" => t.open = false,
                _ => {}
            }
        }
    }
    let running = t.running(&editor);
    if running {
        let seen = Seen {
            editor: &editor,
            rig: &rig,
            playing: timeline.playback.playing,
        };
        while t.step < STEPS.len() && (STEPS[t.step].done)(&seen) {
            t.step += 1;
        }
    }
    let open = running && t.open && !home.open;
    let mut changed = false;
    if signals.get::<bool>("tutor_open") != Some(open) {
        signals.set("tutor_open", open);
        changed = true;
    }
    let target = STEPS.get(t.step).and_then(|s| s.target).filter(|_| open);
    if target != t.target {
        if let Some(node) = t.target.and_then(dom::get_by_id) {
            crate::set_class(node, TARGET, false);
        }
        t.target = target;
        // Render keyed sits below the fold of the Rig panel's scroll.
        if target == Some("rig-render-keyed") {
            signals.set("rig_scroll", f64::from(u32::MAX));
        }
        changed = true;
    }
    if let Some(node) = target.and_then(dom::get_by_id) {
        crate::set_class(node, TARGET, true);
    }
    if !open {
        t.shown = None;
        if changed {
            crate::repaint_soon(&wake);
        }
        return;
    }
    if t.shown != Some(t.step)
        && let Some(list) = dom::get_by_id("tutor-steps")
    {
        list.set_inner_markup(&steps_markup(t.step));
        t.shown = Some(t.step);
        changed = true;
    }
    let text = STEPS.get(t.step).map_or(DONE, |s| s.text);
    set_if_changed(&mut signals, "tutor_text", text.to_string());
    let count = if t.step < STEPS.len() {
        format!("Step {} of {}", t.step + 1, STEPS.len())
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
        .insert_signal("tutor_text", "")
        .insert_signal("tutor_count", "")
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("tutorial::run_tutorial", run_tutorial)
                .after(crate::files::file_commands)
                .after(crate::rig::show_rig)
                .after(crate::timeline::show_timeline)
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
    fn every_step_says_what_to_do_in_a_sentence_or_two() {
        for s in &STEPS {
            let sentences = s.text.matches(". ").count() + 1;
            assert!((1..=2).contains(&sentences), "{}", s.text);
            assert!(s.text.is_ascii() && s.title.is_ascii());
        }
    }

    #[test]
    fn the_list_marks_steps_done_current_and_to_come() {
        let m = steps_markup(2);
        assert!(m.contains(r#"id="tutor-step-1" class="tutor-step done""#));
        assert!(m.contains(r#"id="tutor-step-2" class="tutor-step current""#));
        assert!(m.contains(r#"id="tutor-step-3" class="tutor-step todo""#));
    }
}
