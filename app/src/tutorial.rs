//! The rig tutorial: Help > Rig tutorial opens the rig lesson, a character
//! with no skeleton, and a card over the canvas that walks through building
//! a skeleton for it: a body bone, the head and the arm joined to the body's
//! end, each part's layer bound to its bone, then a pose on two frames and
//! the frames between rendered and played. Each step says what to do and
//! where, outlines the control it uses next, and ticks itself off when the
//! editor shows it done; the next step follows on its own.
//!
//! Closing the card keeps the lesson's place, and Help > Rig tutorial shows
//! it again while the lesson is open. Opening another document ends the
//! lesson.

use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::LayerId;
use pixl_core::rig::{BoneId, Rig};

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

    /// The body bone: the first bone drawn with no parent.
    fn body(&self) -> Option<BoneId> {
        let rig = self.skeleton()?;
        rig.bones()
            .iter()
            .find(|b| b.parent.is_none())
            .map(|b| b.id)
    }

    /// The bones joined to the body, in the order they were drawn: the head,
    /// then the arm.
    fn joined(&self) -> Vec<BoneId> {
        let (Some(rig), Some(body)) = (self.skeleton(), self.body()) else {
            return Vec::new();
        };
        rig.bones()
            .iter()
            .filter(|b| b.parent == Some(body))
            .map(|b| b.id)
            .collect()
    }

    fn head(&self) -> Option<BoneId> {
        self.joined().first().copied()
    }

    fn arm(&self) -> Option<BoneId> {
        self.joined().get(1).copied()
    }

    /// The lesson's layer with this name.
    fn layer(&self, name: &str) -> Option<LayerId> {
        let doc = &self.editor.doc;
        doc.walk()
            .into_iter()
            .find(|(_, l)| l.props.name == name && l.is_raster())
            .map(|(_, l)| l.id)
    }

    /// Whether the layer named `part` is bound to `bone`.
    fn bound(&self, part: &str, bone: Option<BoneId>) -> bool {
        let (Some(rig), Some(layer), Some(bone)) = (self.skeleton(), self.layer(part), bone) else {
            return false;
        };
        rig.bindings()
            .iter()
            .any(|b| b.layer == layer && b.bone == bone)
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

    /// Whether a keyed frame turns `bone`.
    fn keyed_turn(&self, bone: Option<BoneId>) -> bool {
        let (Some(rig), Some(bone)) = (self.skeleton(), bone) else {
            return false;
        };
        self.keyed().iter().any(|f| {
            rig.frames()[*f]
                .key
                .as_ref()
                .is_some_and(|k| k.pose.get(bone).rotation != 0.0)
        })
    }

    /// Whether two keyed frames hold different poses.
    fn two_keys(&self) -> bool {
        let Some(rig) = self.skeleton() else {
            return false;
        };
        let keyed = self.keyed();
        let pose = |f: usize| rig.frames()[f].key.as_ref().map(|k| &k.pose);
        keyed
            .iter()
            .any(|a| keyed.iter().any(|b| pose(*a) != pose(*b)))
    }

    /// `then`, once the Rig panel is in `mode`; until then its button.
    fn in_mode(&self, mode: RigMode, then: Option<String>) -> Option<String> {
        if self.rig.mode == Some(mode) {
            return then;
        }
        Some(
            match mode {
                RigMode::Bones => "rig-mode-bones",
                RigMode::Pose => "rig-mode-pose",
            }
            .to_string(),
        )
    }

    /// The next click that binds the layer named `part` to `bone`: Bones
    /// mode, the bone in the Rig panel's list, the layer in the Layers panel,
    /// then Bind layer.
    fn bind_next(&self, part: &str, bone: Option<BoneId>) -> Option<String> {
        let next = if bone.is_some() && self.rig.bone != bone {
            bone.map(|b| format!("rig-bone-{}", b.0))
        } else if let Some(layer) = self.layer(part).filter(|l| *l != self.editor.layer) {
            Some(format!("layer-row-{}", layer.0))
        } else {
            Some("rig-bind-layer".to_string())
        };
        self.in_mode(RigMode::Bones, next)
    }
}

/// One step of the lesson.
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

/// The lesson's steps, in order.
pub const STEPS: [Step; 14] = [
    Step {
        title: "New skeleton",
        text: "Click New skeleton in the Rig panel on the right. It starts Bones mode, where a drag on the canvas draws a bone.",
        target: |_| Some("rig-new".to_string()),
        done: |s| s.skeleton().is_some(),
    },
    Step {
        title: "Draw the body",
        text: "On the canvas, drag from the hips, where the legs start, up to the neck. This body bone is the one the others join.",
        target: |s| s.in_mode(RigMode::Bones, None),
        done: |s| s.body().is_some(),
    },
    Step {
        title: "Draw the head",
        text: "Drag from the body bone's end at the neck up to the top of the head. A bone started on the selected bone's end joins it there.",
        target: |s| s.in_mode(RigMode::Bones, None),
        done: |s| s.head().is_some(),
    },
    Step {
        title: "Select the body",
        text: "Click the middle of the body bone, or its name in the Rig panel. A new bone joins the selected one.",
        target: |s| {
            s.in_mode(
                RigMode::Bones,
                s.body().map(|b| format!("rig-bone-{}", b.0)),
            )
        },
        done: |s| s.head().is_some() && s.rig.bone.is_some() && s.rig.bone == s.body(),
    },
    Step {
        title: "Draw the arm",
        text: "Drag from the body bone's end out along the arm to the hand. The arm joins the body beside the head.",
        target: |s| s.in_mode(RigMode::Bones, None),
        done: |s| s.arm().is_some(),
    },
    Step {
        title: "Bind the arm",
        text: "Click Bind layer. It binds the active layer, Front arm (highlighted in the Layers panel), to the selected bone, the arm.",
        target: |s| s.bind_next("Front arm", s.arm()),
        done: |s| s.bound("Front arm", s.arm()),
    },
    Step {
        title: "Bind the head",
        text: "Click the head bone, then the Head layer in the Layers panel, then Bind layer. The outline shows each next click.",
        target: |s| s.bind_next("Head", s.head()),
        done: |s| s.bound("Head", s.head()),
    },
    Step {
        title: "Bind the body",
        text: "Bind the body the same way: the body bone, the Body layer, then Bind layer.",
        target: |s| s.bind_next("Body", s.body()),
        done: |s| s.bound("Body", s.body()),
    },
    Step {
        title: "Pose mode",
        text: "Click Pose at the top of the Rig panel. In Pose mode a drag turns a bone instead of drawing one.",
        target: |_| Some("rig-mode-pose".to_string()),
        done: |s| s.rig.mode == Some(RigMode::Pose),
    },
    Step {
        title: "Go to frame 2",
        text: "Click > in the timeline below the canvas. Frame 1 holds the art at rest, so a pose there moves the bones alone.",
        target: |_| Some("tl-next".to_string()),
        done: |s| {
            s.skeleton()
                .is_some_and(|r| s.editor.frame != r.source_frame())
        },
    },
    Step {
        title: "Raise the arm",
        text: "Drag the arm's hand upward. The arm turns at the neck on its own, and letting go keys the pose on this frame.",
        target: |s| s.in_mode(RigMode::Pose, None),
        done: |s| s.keyed_turn(s.arm()),
    },
    Step {
        title: "Turn the body",
        text: "Click >| to go to the last frame, then drag the middle of the body sideways. The head and arm turn with it, since they are joined to it.",
        target: |s| {
            if s.editor.frame + 1 < s.editor.doc.frame_count() {
                return Some("tl-last".to_string());
            }
            s.in_mode(RigMode::Pose, None)
        },
        done: |s| s.keyed_turn(s.body()) && s.two_keys(),
    },
    Step {
        title: "Fill the frames between",
        text: "Click Render keyed near the bottom of the Rig panel. It draws the frames between your keys, easing from one pose to the next.",
        target: |_| Some("rig-render-keyed".to_string()),
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
        text: "Click Play in the timeline to watch the character move. Click it again to stop.",
        target: |_| Some("tl-play".to_string()),
        done: |s| s.playing,
    },
];

/// What the card says once every step is done.
const DONE: &str = "Done: three bones joined at the neck carry the parts. To join a bone to another later, pick its Parent in the Rig panel.";

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
    target: Option<String>,
}

impl Tutorial {
    /// Whether a lesson is open in the editor.
    pub fn running(&self, editor: &Editor) -> bool {
        self.lesson == Some(editor.generation)
    }

    /// The id of the control outlined for the current step.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
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
    let seen = Seen {
        editor: &editor,
        rig: &rig,
        playing: timeline.playback.playing,
    };
    let target = STEPS
        .get(t.step)
        .filter(|_| open)
        .and_then(|s| (s.target)(&seen));
    if target != t.target {
        if let Some(node) = t.target.as_deref().and_then(dom::get_by_id) {
            crate::set_class(node, TARGET, false);
        }
        // Render keyed sits below the fold of the Rig panel's scroll.
        if target.as_deref() == Some("rig-render-keyed") {
            signals.set("rig_scroll", f64::from(u32::MAX));
        }
        t.target = target;
        changed = true;
    }
    if let Some(node) = t.target.as_deref().and_then(dom::get_by_id) {
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
