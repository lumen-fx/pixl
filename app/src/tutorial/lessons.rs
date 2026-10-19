//! The lessons the tutorial walks through, each over a document pixl ships:
//!
//! - Getting started: pick the pencil and a color, draw, erase, undo and
//!   redo, zoom and pan, save.
//! - Layers: in the timeline's layer rows, add a layer, name it, paint on
//!   it, drag it under another, hide and solo layers, fade it, merge it
//!   down.
//! - Selections: select a box, move what it holds, deselect, lasso and add
//!   to the lasso, paint inside the selection.
//! - Animation: add frames, turn on the onion skin, draw a bounce, time a
//!   frame, play, open the sprite sheet export.
//! - Palettes: pick a swatch, build a ramp, paint and shade along it, turn
//!   the picture to indexed color.
//! - Rig: build a skeleton for a character with none: a body bone, the head
//!   and the arm joined to the body's end, each part's layer bound to its
//!   bone, then a pose on two frames and the frames between rendered and
//!   played.

use pixl_document::rig::{BoneId, Rig};
use pixl_document::{ColorMode, LayerId};

use crate::editor::Tool;
use crate::files::shipped::Shipped;
use crate::panels::layers::List;
use crate::rig::{RigMode, skeletons};
use crate::tutorial::{Lesson, Seen, Step};

/// Every lesson, in the order the Help menu lists them.
pub const LESSONS: [&Lesson; 6] = [&START, &LAYERS, &SELECTIONS, &ANIMATION, &PALETTES, &RIG];

/// The lesson with this command id.
pub fn lesson(id: &str) -> Option<&'static Lesson> {
    LESSONS.into_iter().find(|l| l.id == id)
}

// ---- getting started ----

pub const START: Lesson = Lesson {
    id: "help-start-tutorial",
    title: "Getting started",
    document: Shipped::StartLesson,
    layout: "Paint",
    steps: &START_STEPS,
    done: "Done. The Help menu has more tutorials: layers, selections, animation, palettes and rigging.",
};

/// The red swatch of the default palette.
const RED_SWATCH: &str = "swatch-4";

const START_STEPS: [Step; 9] = [
    Step {
        title: "Pick the pencil",
        text: "Click the pencil at the top of the tool bar on the left, or press B. It draws one pixel at a time.",
        target: |_| Some(Tool::Pencil.button_id().to_string()),
        done: |s| s.editor.tool == Tool::Pencil,
    },
    Step {
        title: "Pick a color",
        text: "Click the red swatch in the Palette panel on the right. The large square in the Color panel shows the color you draw with.",
        target: |_| Some(RED_SWATCH.to_string()),
        done: |s| s.editor.fg != s.since.fg,
    },
    Step {
        title: "Draw",
        text: "Drag on the canvas to color in the heart.",
        target: |s| s.with_tool(Tool::Pencil, None),
        done: |s| s.edited("Pencil"),
    },
    Step {
        title: "Erase",
        text: "Pick the eraser in the tool bar, or press E, and drag over part of what you drew.",
        target: |s| s.with_tool(Tool::Eraser, None),
        done: |s| s.edited("Eraser"),
    },
    Step {
        title: "Undo",
        text: "Press Ctrl+Z to undo the erasing. The History panel lists every edit, and Undo there does the same.",
        target: |_| Some("history-undo".to_string()),
        done: |s| s.editor.history.redo_labels().count() > s.since.redo,
    },
    Step {
        title: "Redo",
        text: "Press Ctrl+Shift+Z or Ctrl+Y to redo it.",
        target: |_| Some("history-redo".to_string()),
        done: |s| s.editor.history.redo_labels().count() < s.since.redo,
    },
    Step {
        title: "Zoom",
        text: "Turn the mouse wheel over the canvas to zoom in and out around the pointer. The + and - keys zoom too, and 0 fits the canvas to the window.",
        target: |_| None,
        done: |s| s.zooms > s.since.zooms,
    },
    Step {
        title: "Pan",
        text: "Hold W, A, S or D to move the view, or drag with the middle mouse button.",
        target: |_| None,
        done: |s| s.pans > s.since.pans,
    },
    Step {
        title: "Save",
        text: "Press Ctrl+S to save. This copy has no file yet, so pixl asks where to keep it.",
        target: |_| None,
        done: |s| s.editor.path.is_some() && !s.editor.is_modified(),
    },
];

// ---- layers ----

pub const LAYERS: Lesson = Lesson {
    id: "help-layers-tutorial",
    title: "Layers tutorial",
    document: Shipped::LayersLesson,
    layout: "Paint",
    steps: &LAYERS_STEPS,
    done: "Done. A layer keeps its pixels apart from the others until you merge it, so try ideas on a new one.",
};

impl Seen<'_> {
    /// The id of the eye on a layer's row.
    fn eye(layer: Option<LayerId>) -> Option<String> {
        layer.map(|l| List::Timeline.eye(l))
    }

    /// `then` while the properties dialog is open: its OK button. Until
    /// then the timeline's Props button, once the lesson's layer is active.
    fn props(&self) -> Option<String> {
        if self.flag("lp_open") {
            return Some("lp-ok".to_string());
        }
        self.on_added(Some("tl-edit-layer".to_string()))
    }

    /// `then` while the layer the lesson added is the active one; until
    /// then its row.
    fn on_added(&self, then: Option<String>) -> Option<String> {
        match self.added() {
            Some(layer) if layer != self.editor.layer => Some(row(layer)),
            _ => then,
        }
    }

    /// Whether `layer` shows.
    fn visible(&self, layer: Option<LayerId>) -> bool {
        layer
            .and_then(|l| self.editor.doc.layer(l))
            .is_some_and(|l| l.props.visible)
    }

    /// The place of `layer` among the layers, bottom first.
    fn place(&self, layer: Option<LayerId>) -> Option<usize> {
        let layer = layer?;
        self.editor
            .doc
            .walk()
            .iter()
            .position(|(_, l)| l.id == layer)
    }
}

const LAYERS_STEPS: [Step; 9] = [
    Step {
        title: "Add a layer",
        text: "The timeline below the canvas has a row for each layer. Click New beside Layer above the rows, and the new layer goes above the active one, Tree.",
        target: |_| Some("tl-new-layer".to_string()),
        done: |s| s.added().is_some(),
    },
    Step {
        title: "Name it",
        text: "Click Props beside New, or double-click the new layer's name, then type Shadow as its name and click OK.",
        target: |s| s.props(),
        done: |s| {
            s.added()
                .and_then(|l| s.editor.doc.layer(l))
                .is_some_and(|l| !l.props.name.starts_with("Layer "))
        },
    },
    Step {
        title: "Paint on it",
        text: "With the pencil, draw a shadow on the grass at the foot of the tree. Only the active layer, its row highlighted, takes the paint.",
        target: |s| s.on_added(s.with_tool(Tool::Pencil, None)),
        done: |s| s.added().is_some_and(|l| s.painted(l, s.editor.frame)),
    },
    Step {
        title: "Move it down",
        text: "Drag your layer's name down onto the lower half of the Tree row to move it under Tree, so the tree covers its shadow.",
        target: |s| s.added().map(row),
        done: |s| match (s.place(s.added()), s.place(s.layer("Tree"))) {
            (Some(layer), Some(tree)) => layer < tree,
            _ => false,
        },
    },
    Step {
        title: "Hide a layer",
        text: "Click the eye at the left of the Tree row to hide the tree.",
        target: |s| Seen::eye(s.layer("Tree")),
        done: |s| s.layer("Tree").is_some() && !s.visible(s.layer("Tree")),
    },
    Step {
        title: "Show one alone",
        text: "Hold Alt and click the eye on your layer's row. Only that layer shows.",
        target: |s| Seen::eye(s.added()),
        done: |s| {
            let only = s.added();
            s.visible(only)
                && s.editor
                    .doc
                    .walk()
                    .iter()
                    .all(|(_, l)| Some(l.id) == only || !l.props.visible)
        },
    },
    Step {
        title: "Show them again",
        text: "Alt+click the same eye to bring back what showed before, then click the Tree row's eye to show the tree.",
        target: |s| {
            let hidden = s
                .editor
                .doc
                .walk()
                .iter()
                .filter(|(_, l)| !l.props.visible)
                .any(|(_, l)| Some(l.id) != s.layer("Tree"));
            Seen::eye(if hidden { s.added() } else { s.layer("Tree") })
        },
        done: |s| s.editor.doc.walk().iter().all(|(_, l)| l.props.visible),
    },
    Step {
        title: "Fade it",
        text: "Open your layer's properties again with Props or a double-click on its name, set Opacity to 50 and click OK.",
        target: |s| s.props(),
        done: |s| {
            s.added()
                .and_then(|l| s.editor.doc.layer(l))
                .is_some_and(|l| l.props.opacity < 255)
        },
    },
    Step {
        title: "Merge it down",
        text: "Open the Layer menu in the menu row and click Merge down. Your layer joins Background, the layer under it.",
        target: |s| s.on_added(Some("menu-layer".to_string())),
        done: |s| s.added().is_none() && s.edited("Merge down"),
    },
];

// ---- selections ----

pub const SELECTIONS: Lesson = Lesson {
    id: "help-selection-tutorial",
    title: "Selections tutorial",
    document: Shipped::SelectionLesson,
    layout: "Paint",
    steps: &SELECTION_STEPS,
    done: "Done. Every tool keeps to the selection, and the Select menu grows, shrinks, inverts and saves it.",
};

/// The middles of the selection lesson's apple, star and leaf.
pub const APPLE: (i32, i32) = (8, 9);
pub const STAR: (i32, i32) = (23, 9);
pub const LEAF: (i32, i32) = (15, 24);

impl Seen<'_> {
    /// Whether the selection holds the pixel at `at`.
    fn selects(&self, at: (i32, i32)) -> bool {
        self.editor
            .selection
            .as_ref()
            .is_some_and(|sel| sel.contains(at.0, at.1))
    }
}

const SELECTION_STEPS: [Step; 7] = [
    Step {
        title: "Rectangle select",
        text: "Click Rectangle select in the tool bar, or press M.",
        target: |_| Some(Tool::Select.button_id().to_string()),
        done: |s| s.editor.tool == Tool::Select,
    },
    Step {
        title: "Select the apple",
        text: "Drag a box around the red apple at the top left. An outline marks what is selected.",
        target: |s| s.with_tool(Tool::Select, None),
        done: |s| s.selects(APPLE),
    },
    Step {
        title: "Move it",
        text: "Pick Move in the tool bar, or press V, then drag from inside the box to carry the apple to the right.",
        target: |s| s.with_tool(Tool::Move, None),
        done: |s| s.editor.selection.is_some() && s.edited("Move"),
    },
    Step {
        title: "Deselect",
        text: "Press Escape to drop the selection. Ctrl+D does the same.",
        target: |_| None,
        done: |s| s.editor.selection.is_none(),
    },
    Step {
        title: "Lasso",
        text: "Pick the lasso in the tool bar, or press Q, and drag a loop around the star.",
        target: |s| s.with_tool(Tool::Lasso, None),
        done: |s| s.editor.tool == Tool::Lasso && s.selects(STAR),
    },
    Step {
        title: "Add to it",
        text: "Hold Shift and lasso the leaf too. Shift adds to the selection, and Alt takes away from it.",
        target: |s| s.with_tool(Tool::Lasso, None),
        done: |s| s.selects(STAR) && s.selects(LEAF),
    },
    Step {
        title: "Paint inside",
        text: "Pick the pencil and scribble across the star and the leaf. The paint lands only inside the selection.",
        target: |s| s.with_tool(Tool::Pencil, None),
        done: |s| s.edited("Pencil"),
    },
];

// ---- animation ----

pub const ANIMATION: Lesson = Lesson {
    id: "help-animation-tutorial",
    title: "Animation tutorial",
    document: Shipped::AnimationLesson,
    layout: "Animate",
    steps: &ANIMATION_STEPS,
    done: "Done. Export the sheet from this dialog, or close it and pick Export GIF in the File menu for an animated GIF.",
};

const ANIMATION_STEPS: [Step; 8] = [
    Step {
        title: "Add a frame",
        text: "Click New in the timeline below the canvas. It adds an empty frame 2 after this one and shows it.",
        target: |_| Some("tl-new".to_string()),
        done: |s| s.editor.doc.frame_count() > s.start.doc.frame_count(),
    },
    Step {
        title: "Onion skin",
        text: "Tick Onion skin at the right of the timeline. The frames before and after this one show faintly behind it.",
        target: |_| Some("tl-onion".to_string()),
        done: |s| s.timeline.onion.on,
    },
    Step {
        title: "Draw the fall",
        text: "On frame 2, draw the ball again at the bottom of the canvas, a little wider and flatter where it hits the ground.",
        target: |s| (s.editor.frame != 1).then(|| "tl-frame-1".to_string()),
        done: |s| s.layer("Ball").is_some_and(|l| s.painted(l, 1)),
    },
    Step {
        title: "Add another",
        text: "Click New again for frame 3.",
        target: |_| Some("tl-new".to_string()),
        done: |s| s.editor.doc.frame_count() > s.start.doc.frame_count() + 1,
    },
    Step {
        title: "Draw the rise",
        text: "On frame 3, draw the ball round again, halfway back up.",
        target: |s| (s.editor.frame != 2).then(|| "tl-frame-2".to_string()),
        done: |s| s.layer("Ball").is_some_and(|l| s.painted(l, 2)),
    },
    Step {
        title: "Time the hit",
        text: "Click frame 2's number in the timeline, then type 60 in Duration and press Enter. The ball now meets the ground in a blink.",
        target: |s| {
            Some(if s.editor.frame == 1 {
                "tl-duration".to_string()
            } else {
                "tl-frame-1".to_string()
            })
        },
        done: |s| {
            let was = s.start.doc.frames()[0].duration_ms;
            s.editor.doc.frames().iter().any(|f| f.duration_ms != was)
        },
    },
    Step {
        title: "Play",
        text: "Click Play to watch the ball bounce. Click it again to stop.",
        target: |_| Some("tl-play".to_string()),
        done: |s| s.timeline.playback.playing,
    },
    Step {
        title: "Export",
        text: "Open the File menu and click Export sprite sheet. It lays the frames out side by side on one image, ready for a game.",
        target: |_| Some("menu-file".to_string()),
        done: |s| s.flag("sheet_export_open"),
    },
];

// ---- palettes ----

pub const PALETTES: Lesson = Lesson {
    id: "help-palette-tutorial",
    title: "Palettes tutorial",
    document: Shipped::PaletteLesson,
    layout: "Paint",
    steps: &PALETTE_STEPS,
    done: "Done. Image > Sort palette orders the swatches, and Load palette and Save palette share them as .gpl, .pal, .hex and .ase files.",
};

/// The green swatch of the default palette.
const GREEN_SWATCH: usize = 11;

impl Seen<'_> {
    /// How many colors the lesson's palette started with.
    fn first_colors(&self) -> usize {
        self.start.doc.palette().len()
    }

    /// `then` while the dialog with the open flag `open` shows; until then
    /// the Image menu, which opens it.
    fn image_dialog(&self, open: &str, then: &str) -> Option<String> {
        Some(if self.flag(open) { then } else { "menu-image" }.to_string())
    }
}

const PALETTE_STEPS: [Step; 6] = [
    Step {
        title: "Pick a swatch",
        text: "Click the dark green swatch in the Palette panel. A click picks the color you draw with, and a right-click the background color.",
        target: |_| Some(format!("swatch-{GREEN_SWATCH}")),
        done: |s| s.editor.fg != s.since.fg,
    },
    Step {
        title: "Build a ramp",
        text: "Open the Image menu, click Ramp builder and then Add to palette. It adds shades of your color, dark to light, at the end of the palette.",
        target: |s| s.image_dialog("ramp_open", "ramp-ok"),
        done: |s| s.editor.doc.palette().len() > s.since.doc.palette().len(),
    },
    Step {
        title: "Pick from the ramp",
        text: "Click the middle swatch of the new shades, at the end of the palette.",
        target: |s| {
            let added = s
                .editor
                .doc
                .palette()
                .len()
                .saturating_sub(s.first_colors());
            (added > 0).then(|| format!("swatch-{}", s.first_colors() + added / 2))
        },
        done: |s| {
            let colors = s.editor.doc.palette().colors();
            colors
                .iter()
                .skip(s.first_colors())
                .any(|c| *c == s.editor.fg)
        },
    },
    Step {
        title: "Paint a leaf",
        text: "With the pencil, draw a leaf on the bare stem.",
        target: |s| s.with_tool(Tool::Pencil, None),
        done: |s| s.edited("Pencil"),
    },
    Step {
        title: "Shade it",
        text: "Pick Shading in the tool bar, or press O, and drag over half of your leaf. Each pixel steps to the next swatch, lighter here, and a right-click drag steps back.",
        target: |s| s.with_tool(Tool::Shade, None),
        done: |s| s.edited("Shading"),
    },
    Step {
        title: "Indexed color",
        text: "Open the Image menu, click Indexed color and then OK. Every pixel then holds an entry of the palette instead of a color of its own.",
        target: |s| s.image_dialog("indexed_open", "indexed-ok"),
        done: |s| s.editor.doc.mode() == ColorMode::Indexed,
    },
];

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
    /// mode, the bone in the Rig panel's list, the layer's row in the
    /// timeline, then Bind layer.
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

/// The id of a layer's row in the timeline.
fn row(layer: LayerId) -> String {
    List::Timeline.row(layer)
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
        text: "Click Bind layer. It binds the active layer, Front arm (highlighted in the timeline), to the selected bone, the arm.",
        target: |s| s.bind_next("Front arm", s.arm()),
        done: |s| s.bound("Front arm", s.arm()),
    },
    Step {
        title: "Bind the head",
        text: "Click the head bone, then the Head layer's name in the timeline, then Bind layer. The outline shows each next click.",
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
    fn the_help_menu_lists_every_lesson() {
        let menu = include_str!("../parts/help-menu.lmn");
        for l in LESSONS {
            let item = format!(r#"<menuitem id="{}" label="{}"/>"#, l.id, l.title);
            assert!(menu.contains(&item), "{}", l.title);
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
