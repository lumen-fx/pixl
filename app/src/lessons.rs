//! The lessons the tutorial walks through, each over a document pixl ships.
//!
//! The rig lesson opens a character with no skeleton and builds one for it:
//! a body bone, the head and the arm joined to the body's end, each part's
//! layer bound to its bone, then a pose on two frames and the frames between
//! rendered and played.

use pixl_core::LayerId;
use pixl_core::rig::{BoneId, Rig};

use crate::rig::{RigMode, skeletons};
use crate::shipped::Shipped;
use crate::tutorial::{Lesson, Seen, Step};

/// Every lesson, in the order the Help menu lists them.
pub const LESSONS: [&Lesson; 1] = [&RIG];

/// The lesson with this command id.
pub fn lesson(id: &str) -> Option<&'static Lesson> {
    LESSONS.into_iter().find(|l| l.id == id)
}

// ---- the rig lesson ----

pub const RIG: Lesson = Lesson {
    id: "help-rig-tutorial",
    title: "Rig tutorial",
    document: Shipped::RigLesson,
    layout: "Rig",
    steps: &RIG_STEPS,
    done: "Done: three bones joined at the neck carry the parts. To join a bone to another later, pick its Parent in the Rig panel.",
};

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
            Some(row(layer))
        } else {
            Some("rig-bind-layer".to_string())
        };
        self.in_mode(RigMode::Bones, next)
    }
}

/// The id of a layer's row in the Layers panel.
fn row(layer: LayerId) -> String {
    format!("layer-row-{}", layer.0)
}

const RIG_STEPS: [Step; 14] = [
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
        done: |s| s.timeline.playback.playing,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_says_what_to_do_in_a_sentence_or_two() {
        for lesson in LESSONS {
            for s in lesson.steps {
                let sentences = s.text.matches(". ").count() + 1;
                assert!((1..=2).contains(&sentences), "{}", s.text);
                assert!(s.text.is_ascii() && s.title.is_ascii());
            }
            assert!(lesson.done.is_ascii() && lesson.title.is_ascii());
        }
    }

    #[test]
    fn each_lesson_has_its_own_command_and_document() {
        for (i, a) in LESSONS.iter().enumerate() {
            assert_eq!(lesson(a.id).map(|l| l.title), Some(a.title));
            for b in &LESSONS[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.document, b.document, "{} and {}", a.title, b.title);
            }
        }
    }
}
