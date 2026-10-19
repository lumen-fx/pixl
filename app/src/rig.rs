//! Skeletons in the editor: the Rig panel, drawing and binding bones (bones
//! mode), posing them (pose mode), the pose library and rendering poses into
//! frames, over pixl-document's rigs.
//!
//! Bones draw on the `rig` canvas of the stage, between the art and the
//! overlay, in colors the theme gives the probe elements of `rig.lmn`. They
//! are never part of the art.
//!
//! Posing on a frame other than the rig's rest frame renders the pose into
//! that frame as it changes, through the same layer buffers as painting, so
//! the preview is the result; the pose is keyed on the frame when the drag
//! ends. Posing on the rest frame moves the bones alone.

use lumenui::ecs::query::Added;
use lumenui::glam::Vec2 as Window;
use lumenui::lumen_core::input::{EditCommitted, ModifiersState, PointerButton};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{Canvas, CanvasSet, Context2d, PixelBuffer};
use pixl_document::history::Transaction;
use pixl_document::rig::{
    Bend, Binding, BoneId, BonePose, Easing, NamedPose, Parts, Pose, PoseKey, PosedLayer,
};
use pixl_document::rig::{Region, RenderOptions, Rig, Vec2, mirror_name, normalize_angle};
use pixl_document::{ColorMode, Document, LayerId, LayerKind, NewLayer, Rgba, Scope};

use crate::editor::{Editor, Tool};
use crate::home::Home;
use crate::ids::ElementIds;
use crate::panels::escape;
use crate::settings::keymap::KeyCommands;
use crate::tools::set_if_changed;
use crate::view::input::{Navigation, Pointer, PointerInput};
use crate::view::{View, ViewColors, parse_hex};

/// Screen pixels around a joint or a pivot that pick it.
const PICK: f64 = 7.0;
/// Screen pixels from a bone's line that pick the bone.
const PICK_BODY: f64 = 5.0;
/// The side of a pose thumbnail, in screen pixels.
const THUMB: u32 = 40;

/// What the stage does with the pointer while the Rig panel has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigMode {
    /// Draw bones, move their joints, bind layers and move pivots.
    Bones,
    /// Turn and move bones, with IK on two-bone chains.
    Pose,
}

/// The Rig panel's state.
#[derive(Resource, Default)]
pub struct RigUi {
    pub mode: Option<RigMode>,
    /// The skeleton layer the panel edits.
    pub skeleton: Option<LayerId>,
    /// The selected bone.
    pub bone: Option<BoneId>,
    /// Dragging the end of a bone with a parent bends the two (pose mode).
    pub ik: bool,
    /// The pose shown and edited.
    pub pose: Pose,
    /// The document state the pose was read for: generation, revision,
    /// skeleton and frame.
    pose_for: Option<(u64, u64, LayerId, usize)>,
    drag: Option<Drag>,
    last_tool: Option<Tool>,
    shown: Shown,
}

impl RigUi {
    /// Hands over the Rig panel's state for the document in front,
    /// leaving a new document's.
    pub fn take_document(&mut self) -> RigUi {
        std::mem::take(self)
    }

    /// Puts back the state of the document coming to the front.
    pub fn restore_document(&mut self, mut saved: RigUi) {
        saved.drag = None;
        saved.shown = Shown::default();
        *self = saved;
    }

    /// Whether the stage's pointer belongs to the rig instead of the tools.
    pub fn captures_pointer(&self) -> bool {
        self.mode.is_some()
    }
}

/// What the panel and the canvas last showed.
#[derive(Default)]
struct Shown {
    bones: Option<String>,
    parents: Option<String>,
    bindings: Option<String>,
    library: Option<String>,
    /// The document generation and revision, the bone and the skeleton the
    /// lists above were written for.
    lists_for: Option<(u64, u64, Option<BoneId>, Option<LayerId>)>,
    thumbs: Option<(u64, u64, usize)>,
    canvas: Option<u64>,
    size: (f32, f32),
    name_for: Option<Option<BoneId>>,
    /// The bone and parent the parent dropdown's signal was set for.
    parent_for: Option<(BoneId, Option<BoneId>)>,
    easing_for: Option<(u64, usize)>,
    range_for: Option<(u64, usize)>,
}

/// A press on the stage in progress.
enum Drag {
    /// A new bone from `head`, following the pointer.
    NewBone {
        head: Vec2,
        parent: Option<BoneId>,
        tail: Vec2,
    },
    /// A joint of a bone's rest pose.
    Joint {
        bone: BoneId,
        tail: bool,
        tx: Option<Transaction>,
    },
    /// A binding's pivot.
    Pivot {
        binding: usize,
        tx: Option<Transaction>,
    },
    /// A bone in a pose.
    Pose {
        bone: BoneId,
        how: Handle,
        start: Vec2,
        before: BonePose,
        /// The parts cut out of the rest frame, when the pose renders into
        /// the frame as it changes.
        parts: Option<Parts>,
        /// What the drag last wrote into the frame.
        written: Option<Vec<PosedLayer>>,
        tx: Option<Transaction>,
    },
}

/// Which part of a posed bone the pointer holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Handle {
    Turn { grab: f64 },
    Move { head: Vec2 },
    Ik { bend: Bend },
}

/// The skeleton layers of a document, in paint order.
pub fn skeletons(doc: &Document) -> Vec<LayerId> {
    doc.walk()
        .into_iter()
        .filter(|(_, l)| matches!(l.kind, LayerKind::Skeleton { .. }))
        .map(|(_, l)| l.id)
        .collect()
}

/// A point on the half-pixel grid: pixel centers and corners, where
/// quarter turns land on whole pixels.
fn snap(p: Vec2) -> Vec2 {
    Vec2::new((p.x * 2.0).round() / 2.0, (p.y * 2.0).round() / 2.0)
}

/// A document position in stage coordinates.
fn to_stage(view: &View, p: Vec2) -> (f64, f64) {
    (
        view.origin.0 + p.x * view.zoom,
        view.origin.1 + p.y * view.zoom,
    )
}

fn doc_point(view: &View, at: Window) -> Vec2 {
    let (x, y) = view.to_doc(view.stage_point(at));
    Vec2::new(x, y)
}

/// Screen distance between two document points.
fn screen_distance(view: &View, a: Vec2, b: Vec2) -> f64 {
    (a - b).length() * view.zoom
}

/// Screen distance from `p` to the segment `a`-`b`.
fn segment_distance(view: &View, p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let len2 = d.x * d.x + d.y * d.y;
    let t = if len2 > 0.0 {
        (((p - a).x * d.x + (p - a).y * d.y) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    screen_distance(view, p, a + d * t)
}

/// Bones where the panel shows them: posed in pose mode, at rest otherwise.
fn shown_bones(rig: &Rig, ui: &RigUi) -> Vec<(BoneId, Vec2, Vec2)> {
    if ui.mode == Some(RigMode::Pose) {
        rig.posed(&ui.pose)
            .into_iter()
            .map(|b| (b.id, b.head, b.tail))
            .collect()
    } else {
        rig.bones()
            .iter()
            .map(|b| (b.id, b.head, b.tail()))
            .collect()
    }
}

/// What a press at `p` lands on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Pivot(usize),
    Head(BoneId),
    Tail(BoneId),
    Body(BoneId),
    Nothing,
}

fn hit(view: &View, rig: &Rig, ui: &RigUi, p: Vec2) -> Hit {
    if ui.mode == Some(RigMode::Bones)
        && let Some(sel) = ui.bone
    {
        for (i, b) in rig.bindings().iter().enumerate() {
            if b.bone == sel && screen_distance(view, p, b.pivot) <= PICK {
                return Hit::Pivot(i);
            }
        }
    }
    let bones = shown_bones(rig, ui);
    // The selected bone first, then the others from the newest.
    let order = bones
        .iter()
        .filter(|b| Some(b.0) == ui.bone)
        .chain(bones.iter().rev().filter(|b| Some(b.0) != ui.bone));
    for &(id, head, tail) in order.clone() {
        if screen_distance(view, p, tail) <= PICK {
            return Hit::Tail(id);
        }
        if screen_distance(view, p, head) <= PICK {
            return Hit::Head(id);
        }
    }
    for &(id, head, tail) in order {
        if segment_distance(view, p, head, tail) <= PICK_BODY {
            return Hit::Body(id);
        }
    }
    Hit::Nothing
}

/// Keeps the panel pointed at a skeleton that exists and the pose read for
/// the frame shown.
fn settle(editor: &Editor, ui: &mut RigUi) {
    let doc = &editor.doc;
    let all = skeletons(doc);
    let active = doc
        .layer(editor.layer)
        .filter(|l| matches!(l.kind, LayerKind::Skeleton { .. }))
        .map(|l| l.id);
    let keep = ui.skeleton.filter(|s| all.contains(s));
    let skeleton = active.or(keep).or(all.first().copied());
    if skeleton != ui.skeleton {
        ui.skeleton = skeleton;
        ui.bone = None;
        ui.pose_for = None;
    }
    let Some(skeleton) = skeleton else {
        ui.mode = None;
        return;
    };
    let rig = doc.rig(skeleton).expect("a skeleton layer");
    if ui.bone.is_some_and(|b| rig.bone(b).is_none()) {
        ui.bone = None;
    }
    let now = (editor.generation, editor.revision, skeleton, editor.frame);
    if ui.drag.is_none() && ui.pose_for != Some(now) {
        ui.pose = rig
            .pose_at(editor.frame)
            .unwrap_or_else(|| rig.pose.clone());
        ui.pose_for = Some(now);
    }
}

/// Runs a rig edit as one undo step that changes no pixels.
fn edit_rig<R>(
    editor: &mut Editor,
    skeleton: LayerId,
    label: &str,
    edit: impl FnOnce(&mut Rig) -> std::result::Result<R, pixl_document::Error>,
) -> Option<R> {
    let done = editor
        .history
        .transact(&mut editor.doc, label, Scope::Images(Vec::new()), |doc| {
            doc.rig_mut(skeleton).and_then(edit)
        });
    match done {
        Ok(r) => {
            editor.touch();
            Some(r)
        }
        Err(e) => {
            editor.message = Some(e.to_string());
            None
        }
    }
}

/// Shows `pose` on the editor's frame: rendered into it when it is not the
/// rest frame. Returns whether pixels were written.
///
/// `last` holds what the previous call wrote during the same drag; only the
/// layers whose posed pixels differ from it are written again, so turning a
/// hand rewrites the hand alone.
fn show_pose(
    editor: &mut Editor,
    skeleton: LayerId,
    parts: &Parts,
    pose: &Pose,
    last: &mut Option<Vec<PosedLayer>>,
) -> bool {
    let frame = editor.frame;
    let Ok(rig) = editor.doc.rig(skeleton) else {
        return false;
    };
    if frame == rig.source_frame() {
        return false;
    }
    let all = parts.render(rig, pose);
    let posed: Vec<PosedLayer> = match last.as_ref() {
        Some(before) => all
            .iter()
            .filter(|p| !before.contains(p))
            .cloned()
            .collect(),
        None => all.clone(),
    };
    if posed.is_empty() {
        return true;
    }
    let changed: Vec<LayerId> = posed.iter().map(|p| p.layer).collect();
    // Which layers show a cel on the frame: a render that only rewrites the
    // bound layers' cels lets the view look at those layers alone.
    let cels = |doc: &Document| -> Vec<bool> {
        doc.walk()
            .iter()
            .map(|(_, l)| doc.cel(l.id, frame).ok().flatten().is_some())
            .collect()
    };
    let before = cels(&editor.doc);
    match editor
        .doc
        .write_posed(skeleton, &posed, frame, RenderOptions::default())
    {
        Ok(_) => {
            if cels(&editor.doc) == before {
                editor.touch_layers(&changed);
            } else {
                editor.touch();
            }
            *last = Some(all);
            true
        }
        Err(e) => {
            editor.message = Some(e.to_string());
            false
        }
    }
}

/// Ends a pose edit: the pose becomes the rig's pose, and the frame's key
/// when it is not the rest frame.
fn keep_pose(editor: &mut Editor, skeleton: LayerId, pose: &Pose) {
    let frame = editor.frame;
    if let Ok(rig) = editor.doc.rig_mut(skeleton) {
        rig.pose = pose.clone();
        if frame != rig.source_frame()
            && let Ok(f) = rig.frame_mut(frame)
        {
            let easing = f.key.as_ref().map_or(Easing::Linear, |k| k.easing);
            f.key = Some(PoseKey {
                pose: pose.clone(),
                easing,
            });
            f.generated = true;
        }
    }
}

/// Applies a whole pose (from the library, mirrored, reset) as one undo
/// step.
fn apply_pose(editor: &mut Editor, ui: &mut RigUi, skeleton: LayerId, pose: Pose, label: &str) {
    let tx = editor.history.begin(&editor.doc, label, Scope::All);
    if let Ok(parts) = Parts::new(&editor.doc, skeleton) {
        show_pose(editor, skeleton, &parts, &pose, &mut None);
    }
    keep_pose(editor, skeleton, &pose);
    editor.history.commit(&editor.doc, tx);
    editor.touch();
    ui.pose = pose;
    ui.pose_for = None;
}

/// The panel's buttons and rows.
pub fn rig_clicks(
    mut editor: ResMut<Editor>,
    mut ui: ResMut<RigUi>,
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    keys: Res<KeyCommands>,
    workspace: Option<ResMut<crate::workspace::Workspace>>,
    signals: Signals,
) {
    let editor = &mut *editor;
    let ui = &mut *ui;
    let was = ui.mode;
    let mut clicked: Vec<String> = clicks.read().filter_map(|c| ids.id_of(c.entity)).collect();
    // The mode keys run what the mode buttons do.
    for id in ["rig-mode-bones", "rig-mode-pose"] {
        if keys.ran(id, false) {
            clicked.push(id.to_string());
        }
    }
    if keys.ran("cancel", false) {
        ui.mode = None;
    }
    // Picking a tool hands the stage back to it.
    if ui.last_tool != Some(editor.tool) {
        if ui.last_tool.is_some() {
            ui.mode = None;
        }
        ui.last_tool = Some(editor.tool);
    }
    if clicked.iter().any(|id| id.starts_with("tool-")) {
        ui.mode = None;
    }
    settle(editor, ui);
    // A mode asked for with no skeleton to work on unfolds the panel, where
    // New skeleton is.
    let mut unfold = false;
    for id in clicked {
        let skeleton = ui.skeleton;
        if skeleton.is_none() && matches!(id.as_str(), "rig-mode-bones" | "rig-mode-pose") {
            editor.message = Some("Make a skeleton first: New skeleton in the Rig panel".into());
            unfold = true;
        }
        match id.as_str() {
            "rig-new" => {
                let at = pixl_document::Placement::default();
                let made =
                    editor
                        .history
                        .transact(&mut editor.doc, "New skeleton", Scope::All, |doc| {
                            doc.add_layer(NewLayer::Skeleton, "Skeleton", at)
                        });
                if let Ok(id) = made {
                    editor.touch();
                    ui.skeleton = Some(id);
                    ui.bone = None;
                    ui.mode = Some(RigMode::Bones);
                }
            }
            "rig-mode-bones" => {
                ui.mode = (ui.mode != Some(RigMode::Bones) && skeleton.is_some())
                    .then_some(RigMode::Bones);
            }
            "rig-mode-pose" => {
                ui.mode =
                    (ui.mode != Some(RigMode::Pose) && skeleton.is_some()).then_some(RigMode::Pose);
                ui.pose_for = None;
            }
            "rig-ik" => ui.ik = !ui.ik,
            _ => {}
        }
        let Some(skeleton) = skeleton else {
            continue;
        };
        match id.as_str() {
            "rig-bone-delete" => {
                if let Some(b) = ui.bone.take() {
                    edit_rig(editor, skeleton, "Delete bone", |rig| rig.remove_bone(b));
                }
            }
            "rig-bind-layer" | "rig-bind-selection" => {
                bind(editor, ui, skeleton, id == "rig-bind-selection");
            }
            "rig-rest-frame" => {
                let frame = editor.frame;
                edit_rig(editor, skeleton, "Rest frame", |rig| {
                    rig.set_source_frame(frame)
                });
            }
            "rig-mirror" => {
                let Ok(rig) = editor.doc.rig(skeleton) else {
                    continue;
                };
                let pose = rig.mirror(&ui.pose);
                apply_pose(editor, ui, skeleton, pose, "Mirror pose");
            }
            "rig-reset" => apply_pose(editor, ui, skeleton, Pose::default(), "Reset pose"),
            "rig-pose-save" => {
                let pose = ui.pose.clone();
                let typed = signals.get::<String>("rig_pose_name").unwrap_or_default();
                edit_rig(editor, skeleton, "Save pose", |rig| {
                    let name = match typed.trim() {
                        "" => format!("Pose {}", rig.library.len() + 1),
                        t => t.to_string(),
                    };
                    rig.library.push(NamedPose { name, pose });
                    Ok(())
                });
            }
            "rig-key" | "rig-unkey" => {
                let frame = editor.frame;
                let pose = ui.pose.clone();
                let easing = signals
                    .get::<String>("rig_easing")
                    .and_then(|n| Easing::from_name(&n))
                    .unwrap_or_default();
                let key = (id == "rig-key").then_some(PoseKey { pose, easing });
                let label = if key.is_some() {
                    "Key pose"
                } else {
                    "Clear key"
                };
                edit_rig(editor, skeleton, label, |rig| {
                    rig.frame_mut(frame)?.key = key;
                    Ok(())
                });
            }
            "rig-render-new" => {
                let at = editor.frame + 1;
                let pose = ui.pose.clone();
                let made =
                    editor
                        .history
                        .transact(&mut editor.doc, "Render pose", Scope::All, |doc| {
                            doc.render_pose_to_new_frame(
                                skeleton,
                                &pose,
                                at,
                                RenderOptions::default(),
                            )
                        });
                match made {
                    Ok(f) => {
                        editor.frame = f;
                        editor.touch();
                    }
                    Err(e) => editor.message = Some(e.to_string()),
                }
            }
            "rig-render-keyed" | "rig-regenerate" => {
                let last = editor.doc.frame_count();
                let number = |name: &str, default: usize| {
                    signals
                        .get::<String>(name)
                        .and_then(|t| t.trim().parse::<usize>().ok())
                        .unwrap_or(default)
                        .clamp(1, last)
                };
                let (from, to) = (number("rig_from", 1), number("rig_to", last));
                let keyed = id == "rig-render-keyed";
                let label = if keyed {
                    "Render keyed poses"
                } else {
                    "Regenerate"
                };
                let done = editor
                    .history
                    .transact(&mut editor.doc, label, Scope::All, |doc| {
                        if keyed {
                            doc.render_keyed(skeleton, from - 1, to - 1, RenderOptions::default())
                        } else {
                            doc.regenerate(skeleton, RenderOptions::default())
                        }
                    });
                match done {
                    Ok((frames, _)) => {
                        editor.message = Some(match frames.len() {
                            0 => "No frame has a keyed pose to render".to_string(),
                            1 => "Rendered 1 frame".to_string(),
                            n => format!("Rendered {n} frames"),
                        });
                        editor.touch();
                    }
                    Err(e) => editor.message = Some(e.to_string()),
                }
            }
            other => {
                if let Some(n) = other.strip_prefix("rig-bone-").and_then(|n| n.parse().ok()) {
                    ui.bone = Some(BoneId(n));
                } else if let Some(i) = other
                    .strip_prefix("rig-unbind-")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    edit_rig(editor, skeleton, "Unbind", |rig| {
                        rig.unbind(i)
                            .map(|_| ())
                            .ok_or(pixl_document::Error::BindingNotFound(i))
                    });
                } else if let Some(k) = other
                    .strip_prefix("rig-pose-del-")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    edit_rig(editor, skeleton, "Delete pose", |rig| {
                        if k < rig.library.len() {
                            rig.library.remove(k);
                        }
                        Ok(())
                    });
                } else if let Some(k) = other
                    .strip_prefix("rig-pose-")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    let pose = editor
                        .doc
                        .rig(skeleton)
                        .ok()
                        .and_then(|r| r.library.get(k))
                        .map(|p| p.pose.clone());
                    if let Some(pose) = pose {
                        apply_pose(editor, ui, skeleton, pose, "Apply pose");
                    }
                }
            }
        }
    }
    // Entering a mode unfolds the panel it works in.
    if (unfold || (ui.mode.is_some() && ui.mode != was))
        && let Some(mut ws) = workspace
        && ws
            .live
            .place("panel-rig")
            .is_some_and(|p| p.collapsed || !p.shown)
    {
        ws.edit(|l| {
            if let Some(p) = l.panels.iter_mut().find(|p| p.panel == "panel-rig") {
                p.collapsed = false;
                p.shown = true;
            }
        });
    }
}

/// Binds the active layer, or the selection on it, to the selected bone.
fn bind(editor: &mut Editor, ui: &RigUi, skeleton: LayerId, selection: bool) {
    let Some(bone) = ui.bone else {
        editor.message = Some("Select a bone to bind to".to_string());
        return;
    };
    let layer = editor.layer;
    if !editor.doc.layer(layer).is_some_and(|l| l.is_raster()) {
        editor.message = Some("Bind a raster layer: pick one in the layers panel".to_string());
        return;
    }
    let region = if selection {
        match editor.selection.as_ref().and_then(Region::from_selection) {
            Some(r) => Some(r),
            None => {
                editor.message = Some("Select the part to bind first".to_string());
                return;
            }
        }
    } else {
        None
    };
    let Ok(head) = editor.doc.rig(skeleton).and_then(|r| {
        r.bone(bone)
            .map(|b| b.head)
            .ok_or(pixl_document::Error::BoneNotFound(bone))
    }) else {
        return;
    };
    let label = if selection {
        "Bind selection"
    } else {
        "Bind layer"
    };
    edit_rig(editor, skeleton, label, |rig| {
        rig.bind(Binding {
            layer,
            region,
            bone,
            pivot: snap(head),
        })
    });
}

/// The bone name field, the parent and easing dropdowns.
pub fn rig_controls(
    mut editor: ResMut<Editor>,
    mut ui: ResMut<RigUi>,
    signals: Signals,
    committed: Query<&LumenId, Added<EditCommitted>>,
) {
    let editor = &mut *editor;
    let Some(skeleton) = ui.skeleton else {
        return;
    };
    let Ok(rig) = editor.doc.rig(skeleton) else {
        return;
    };
    if let Some(bone) = ui.bone
        && committed.iter().any(|id| id.0 == "rig-bone-name")
        && let Some(name) = signals.get::<String>("rig_bone_name")
        && rig.bone(bone).is_some_and(|b| b.name != name.trim())
        && !name.trim().is_empty()
    {
        let name = name.trim().to_string();
        edit_rig(editor, skeleton, "Rename bone", |rig| {
            rig.rename_bone(bone, name)
        });
    }
    let Ok(rig) = editor.doc.rig(skeleton) else {
        return;
    };
    if let Some(bone) = ui.bone
        && let Some(wanted) = signals.get::<String>("rig_parent")
        && let Some(b) = rig.bone(bone)
        && ui.shown.parent_for == Some((bone, b.parent))
    {
        let parent = parent_choice(rig, &wanted);
        if parent != Some(b.parent) && parent.is_some() {
            let parent = parent.flatten();
            if edit_rig(editor, skeleton, "Reparent bone", |rig| {
                rig.set_parent(bone, parent)
            })
            .is_none()
            {
                ui.shown.parents = None;
            }
        }
    }
    let Ok(rig) = editor.doc.rig(skeleton) else {
        return;
    };
    let frame = editor.frame;
    if let Some(name) = signals.get::<String>("rig_easing")
        && let Some(easing) = Easing::from_name(&name)
        && let Some(key) = rig.frames().get(frame).and_then(|f| f.key.as_ref())
        && key.easing != easing
        && ui.shown.easing_for == Some((editor.revision, frame))
    {
        edit_rig(editor, skeleton, "Easing", |rig| {
            if let Some(k) = rig.frame_mut(frame)?.key.as_mut() {
                k.easing = easing;
            }
            Ok(())
        });
    }
}

/// The dropdown's text for a parent choice.
fn parent_label(rig: &Rig, parent: Option<BoneId>) -> String {
    match parent.and_then(|p| rig.bone(p)) {
        Some(b) => format!("{} ({})", b.name, b.id.0),
        None => "No parent".to_string(),
    }
}

/// The parent a dropdown text names: `Some(None)` for no parent, `None`
/// for text naming no bone.
fn parent_choice(rig: &Rig, text: &str) -> Option<Option<BoneId>> {
    if text == "No parent" {
        return Some(None);
    }
    rig.bones()
        .iter()
        .find(|b| parent_label(rig, Some(b.id)) == text)
        .map(|b| Some(b.id))
}

/// The stage's pointer in bones and pose mode.
#[allow(clippy::too_many_arguments)]
pub fn use_rig(
    mut editor: ResMut<Editor>,
    mut ui: ResMut<RigUi>,
    view: Res<View>,
    nav: Res<Navigation>,
    modifiers: Res<ModifiersState>,
    hovered: Query<Option<&LumenId>, With<Hovered>>,
    mut pointer: PointerInput,
    home: Res<Home>,
) {
    let editor = &mut *editor;
    let ui = &mut *ui;
    let events = pointer.events();
    if home.open {
        return;
    }
    let (Some(mode), Some(skeleton)) = (ui.mode, ui.skeleton) else {
        return;
    };
    let shift = modifiers.0.shift;
    let over_stage = hovered.iter().all(|id| {
        id.is_some_and(|id| {
            matches!(
                id.0.as_str(),
                "stage" | "checker" | "art" | "rig" | "overlay"
            )
        })
    });
    for event in events {
        match event {
            Pointer::Down { at, button, .. } => {
                if ui.drag.is_some()
                    || nav.space_held
                    || nav.panning.is_some()
                    || !view.contains(at)
                    || !over_stage
                    || button != PointerButton::Primary
                {
                    continue;
                }
                let p = doc_point(&view, at);
                ui.drag = match mode {
                    RigMode::Bones => press_bones(editor, ui, skeleton, &view, p, shift),
                    RigMode::Pose => press_pose(editor, ui, skeleton, &view, p),
                };
            }
            Pointer::Move { at, .. } => {
                let p = doc_point(&view, at);
                move_drag(editor, ui, skeleton, p, shift);
            }
            Pointer::Up { at, button, .. } => {
                if button != PointerButton::Primary {
                    continue;
                }
                let p = doc_point(&view, at);
                move_drag(editor, ui, skeleton, p, shift);
                end_drag(editor, ui, skeleton);
            }
        }
    }
}

fn press_bones(
    editor: &mut Editor,
    ui: &mut RigUi,
    skeleton: LayerId,
    view: &View,
    p: Vec2,
    shift: bool,
) -> Option<Drag> {
    let rig = editor.doc.rig(skeleton).ok()?;
    let begin = |editor: &Editor, label: &str| {
        Some(
            editor
                .history
                .begin(&editor.doc, label, Scope::Images(Vec::new())),
        )
    };
    match hit(view, rig, ui, p) {
        Hit::Pivot(binding) => Some(Drag::Pivot {
            binding,
            tx: begin(editor, "Move pivot"),
        }),
        // The selected bone's end starts a bone joined to it; Shift moves
        // the end instead.
        Hit::Tail(b) if Some(b) == ui.bone && !shift => {
            let head = rig.bone(b)?.tail();
            Some(Drag::NewBone {
                head,
                parent: Some(b),
                tail: head,
            })
        }
        Hit::Tail(b) => {
            ui.bone = Some(b);
            Some(Drag::Joint {
                bone: b,
                tail: true,
                tx: begin(editor, "Move joint"),
            })
        }
        Hit::Head(b) => {
            ui.bone = Some(b);
            Some(Drag::Joint {
                bone: b,
                tail: false,
                tx: begin(editor, "Move joint"),
            })
        }
        Hit::Body(b) => {
            ui.bone = Some(b);
            None
        }
        Hit::Nothing => {
            let head = snap(p);
            Some(Drag::NewBone {
                head,
                parent: ui.bone,
                tail: head,
            })
        }
    }
}

fn press_pose(
    editor: &mut Editor,
    ui: &mut RigUi,
    skeleton: LayerId,
    view: &View,
    p: Vec2,
) -> Option<Drag> {
    let rig = &editor.doc.rig(skeleton).ok()?.clone();
    let (bone, how) = match hit(view, rig, ui, p) {
        Hit::Tail(b) if ui.ik && rig.bone(b)?.parent.is_some() => (
            b,
            Handle::Ik {
                bend: rig.bend_of(&ui.pose, b),
            },
        ),
        Hit::Head(b) => {
            let head = rig.posed(&ui.pose).into_iter().find(|x| x.id == b)?.head;
            (b, Handle::Move { head })
        }
        Hit::Tail(b) | Hit::Body(b) => {
            let head = rig.posed(&ui.pose).into_iter().find(|x| x.id == b)?.head;
            (
                b,
                Handle::Turn {
                    grab: (p - head).angle(),
                },
            )
        }
        Hit::Pivot(_) | Hit::Nothing => {
            ui.bone = None;
            return None;
        }
    };
    ui.bone = Some(bone);
    let rendering = editor.frame != rig.source_frame();
    let parts = if rendering {
        Parts::new(&editor.doc, skeleton).ok()
    } else {
        None
    };
    let scope = if rendering {
        Scope::All
    } else {
        Scope::Images(Vec::new())
    };
    let label = match how {
        Handle::Ik { .. } => "Pose (IK)",
        Handle::Move { .. } => "Move bone",
        Handle::Turn { .. } => "Turn bone",
    };
    Some(Drag::Pose {
        bone,
        how,
        start: p,
        before: ui.pose.get(bone),
        parts,
        written: None,
        tx: Some(editor.history.begin(&editor.doc, label, scope)),
    })
}

fn move_drag(editor: &mut Editor, ui: &mut RigUi, skeleton: LayerId, p: Vec2, shift: bool) {
    let RigUi {
        drag,
        pose: shown_pose,
        shown,
        ..
    } = ui;
    let Some(drag) = drag.as_mut() else {
        return;
    };
    match drag {
        Drag::NewBone { tail, .. } => *tail = snap(p),
        Drag::Joint { bone, tail, .. } => {
            let (bone, tail) = (*bone, *tail);
            let to = snap(p);
            if let Ok(rig) = editor.doc.rig_mut(skeleton) {
                move_joint(rig, bone, tail, to);
                editor.touch();
            }
        }
        Drag::Pivot { binding, .. } => {
            let i = *binding;
            if let Ok(rig) = editor.doc.rig_mut(skeleton) {
                let _ = rig.update_binding(i, |b| b.pivot = snap(p));
                editor.touch();
            }
        }
        Drag::Pose {
            bone,
            how,
            start,
            before,
            parts,
            written,
            ..
        } => {
            let Ok(rig) = editor.doc.rig(skeleton) else {
                return;
            };
            let mut pose = shown_pose.clone();
            match *how {
                Handle::Turn { grab } => {
                    let Some(head) = rig.posed(&pose).into_iter().find(|b| b.id == *bone) else {
                        return;
                    };
                    let mut turn = (p - head.head).angle() - grab;
                    if shift {
                        turn = ((before.rotation + turn) / 15.0).round() * 15.0 - before.rotation;
                    }
                    pose.set(
                        *bone,
                        BonePose {
                            rotation: normalize_angle(before.rotation + turn),
                            ..*before
                        },
                    );
                }
                Handle::Move { head } => {
                    let mut target = head + (p - *start);
                    if !shift {
                        target = Vec2::new(target.x.round(), target.y.round())
                            + Vec2::new(head.x - head.x.round(), head.y - head.y.round());
                    }
                    let _ = rig.move_head(&mut pose, *bone, target);
                }
                Handle::Ik { bend } => {
                    let _ = rig.solve_ik(&mut pose, *bone, p, bend);
                }
            }
            if pose == *shown_pose {
                return;
            }
            *shown_pose = pose;
            if let Some(parts) = parts.as_ref() {
                show_pose(editor, skeleton, parts, shown_pose, written);
            } else {
                // The bones moved; the canvas shows them.
                shown.canvas = None;
            }
        }
    }
}

/// Moves one end of a bone's rest pose; bones joined at that end follow.
fn move_joint(rig: &mut Rig, bone: BoneId, tail: bool, to: Vec2) {
    let Some(b) = rig.bone(bone).cloned() else {
        return;
    };
    if tail {
        let old = b.tail();
        let _ = rig.set_rest(bone, b.head, to);
        let joined: Vec<(BoneId, Vec2)> = rig
            .bones()
            .iter()
            .filter(|c| c.parent == Some(bone) && (c.head - old).length() < 1e-6)
            .map(|c| (c.id, c.tail()))
            .collect();
        for (c, t) in joined {
            let _ = rig.set_rest(c, to, t);
        }
    } else {
        let _ = rig.set_rest(bone, to, b.tail());
    }
}

fn end_drag(editor: &mut Editor, ui: &mut RigUi, skeleton: LayerId) {
    let Some(drag) = ui.drag.take() else {
        return;
    };
    match drag {
        Drag::NewBone { head, parent, tail } => {
            if (tail - head).length() < 0.5 {
                ui.bone = None;
                return;
            }
            let made = edit_rig(editor, skeleton, "Add bone", |rig| {
                let name = rig.unique_name("Bone");
                rig.add_bone(name, parent, head, tail)
            });
            if let Some(b) = made {
                ui.bone = Some(b);
            }
        }
        Drag::Joint { tx, .. } | Drag::Pivot { tx, .. } => {
            if let Some(tx) = tx {
                editor.history.commit(&editor.doc, tx);
                editor.touch();
            }
        }
        Drag::Pose { tx, .. } => {
            let pose = ui.pose.clone();
            keep_pose(editor, skeleton, &pose);
            if let Some(tx) = tx {
                editor.history.commit(&editor.doc, tx);
            }
            editor.touch();
            ui.pose_for = None;
        }
    }
}

// ---- showing ----

/// The bone list, roots first and children under their parent.
fn bones_markup(rig: &Rig, selected: Option<BoneId>, bound: &[BoneId]) -> String {
    fn rows(
        rig: &Rig,
        parent: Option<BoneId>,
        depth: usize,
        selected: Option<BoneId>,
        bound: &[BoneId],
        out: &mut String,
    ) {
        for b in rig.bones().iter().filter(|b| b.parent == parent) {
            if depth > rig.bones().len() {
                return;
            }
            let active = if Some(b.id) == selected {
                " active"
            } else {
                ""
            };
            let parts = bound.iter().filter(|x| **x == b.id).count();
            let meta = match parts {
                0 => String::new(),
                1 => "1 part".to_string(),
                n => format!("{n} parts"),
            };
            let side = if mirror_name(&b.name).is_some() {
                " sided"
            } else {
                ""
            };
            out.push_str(&format!(
                r#"<row id="rig-bone-{id}" class="rig-bone depth-{depth}{active}{side}"><label class="rig-bone-name" text="{name}"/><spacer/><label class="rig-bone-meta" text="{meta}"/></row>"#,
                id = b.id.0,
                depth = depth.min(4),
                name = escape(&b.name),
            ));
            rows(rig, Some(b.id), depth + 1, selected, bound, out);
        }
    }
    let mut out = String::new();
    rows(rig, None, 0, selected, bound, &mut out);
    if out.is_empty() {
        out.push_str(r#"<label class="rig-empty" text="Drag on the canvas to draw a bone"/>"#);
    }
    out
}

fn parents_markup(rig: &Rig, bone: Option<BoneId>) -> String {
    let Some(b) = bone.and_then(|b| rig.bone(b)) else {
        return String::new();
    };
    let mut options = String::from(r#"<option value="No parent"/>"#);
    let below: Vec<BoneId> = rig
        .bones()
        .iter()
        .filter(|o| o.id == b.id || rig.ancestors(o.id).contains(&b.id))
        .map(|o| o.id)
        .collect();
    for o in rig.bones().iter().filter(|o| !below.contains(&o.id)) {
        options.push_str(&format!(
            r#"<option value="{}"/>"#,
            escape(&parent_label(rig, Some(o.id)))
        ));
    }
    format!(
        r#"<label class="option-label" text="Parent"/><dropdown id="rig-parent" class="rig-parent" bind-value="rig_parent">{options}</dropdown>"#
    )
}

fn bindings_markup(doc: &Document, rig: &Rig, bone: Option<BoneId>) -> String {
    let mut out = String::new();
    for (i, b) in rig.bindings().iter().enumerate() {
        if Some(b.bone) != bone {
            continue;
        }
        let layer = doc
            .layer(b.layer)
            .map_or("(deleted layer)".to_string(), |l| l.props.name.clone());
        let what = match &b.region {
            None => "whole layer".to_string(),
            Some(r) => format!("{}x{} region", r.bounds().w, r.bounds().h),
        };
        out.push_str(&format!(
            r#"<row class="rig-binding"><label class="rig-binding-name" text="{}"/><label class="rig-binding-meta" text="{}"/><spacer/><tooltip text="Unbind"><button id="rig-unbind-{i}" class="flat small" text="x"/></tooltip></row>"#,
            escape(&layer),
            escape(&what),
        ));
    }
    out
}

fn library_markup(rig: &Rig) -> String {
    let mut out = String::new();
    for (k, p) in rig.library.iter().enumerate() {
        out.push_str(&format!(
            r#"<row id="rig-pose-{k}" class="rig-pose"><canvas id="rig-thumb-{k}" class="rig-thumb"/><label class="rig-pose-name" text="{}"/><spacer/><tooltip text="Delete pose"><button id="rig-pose-del-{k}" class="flat small" text="x"/></tooltip></row>"#,
            escape(&p.name)
        ));
    }
    if out.is_empty() {
        out.push_str(r#"<label class="rig-empty" text="Saved poses show here"/>"#);
    }
    out
}

fn refresh(slot: &mut Option<String>, container: &str, markup: String) -> bool {
    if slot.as_deref() == Some(markup.as_str()) {
        return false;
    }
    let Some(node) = dom::get_by_id(container) else {
        return false;
    };
    node.set_inner_markup(&markup);
    *slot = Some(markup);
    true
}

/// What a frame is to the rig, for the panel.
fn frame_info(rig: &Rig, frame: usize, frames: usize) -> String {
    let mut parts = vec![format!("Frame {} of {frames}", frame + 1)];
    if frame == rig.source_frame() {
        parts.push("rest art".to_string());
    }
    if let Some(f) = rig.frames().get(frame) {
        if let Some(k) = &f.key {
            parts.push(format!("keyed, {}", k.easing.name().to_lowercase()));
        }
        if f.generated {
            parts.push("generated".to_string());
        }
    }
    parts.join(" - ")
}

/// The panel, its controls and the bones on the stage.
#[allow(clippy::too_many_arguments)]
pub fn show_rig(
    editor: Res<Editor>,
    mut ui: ResMut<RigUi>,
    view: Res<View>,
    colors: Res<ViewColors>,
    canvases: Query<&Canvas>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let ui = &mut *ui;
    let doc = &editor.doc;
    let rig = ui.skeleton.and_then(|s| doc.rig(s).ok());
    let mut changed = false;
    let mode = |m| if ui.mode == Some(m) { "true" } else { "false" };
    let flags = [
        ("rig_none", rig.is_none().to_string()),
        ("rig_some", rig.is_some().to_string()),
        ("rig_bones", mode(RigMode::Bones).to_string()),
        ("rig_posing", mode(RigMode::Pose).to_string()),
        (
            "rig_has_bone",
            (rig.is_some() && ui.bone.is_some()).to_string(),
        ),
    ];
    for (name, v) in flags {
        let want = v == "true";
        if signals.get::<bool>(name) != Some(want) {
            signals.set(name, want);
            changed = true;
        }
    }
    for (id, on) in [
        ("rig-mode-bones", ui.mode == Some(RigMode::Bones)),
        ("rig-mode-pose", ui.mode == Some(RigMode::Pose)),
        ("rig-ik", ui.ik),
    ] {
        if let Some(n) = dom::get_by_id(id) {
            crate::set_class(n, "active", on);
        }
    }
    if let Some(app) = dom::get_by_id("app") {
        crate::set_class(app, "rigging", ui.mode.is_some());
    }
    if let Some(rig) = rig {
        // The lists change only with the document, the bone or the rig.
        let lists = (editor.generation, editor.revision, ui.bone, ui.skeleton);
        if ui.shown.lists_for != Some(lists) && dom::get_by_id("rig-bone-list").is_some() {
            ui.shown.lists_for = Some(lists);
            let bound: Vec<BoneId> = rig.bindings().iter().map(|b| b.bone).collect();
            changed |= refresh(
                &mut ui.shown.bones,
                "rig-bone-list",
                bones_markup(rig, ui.bone, &bound),
            );
            changed |= refresh(
                &mut ui.shown.parents,
                "rig-parent-box",
                parents_markup(rig, ui.bone),
            );
            changed |= refresh(
                &mut ui.shown.bindings,
                "rig-binding-list",
                bindings_markup(doc, rig, ui.bone),
            );
            if refresh(&mut ui.shown.library, "rig-library", library_markup(rig)) {
                changed = true;
                ui.shown.thumbs = None;
            }
        }
        let parent = ui.bone.and_then(|b| rig.bone(b)).map(|b| (b.id, b.parent));
        if let Some((b, p)) = parent
            && ui.shown.parent_for != parent
        {
            signals.set("rig_parent", parent_label(rig, p));
            ui.shown.parent_for = Some((b, p));
            changed = true;
        }

        if ui.shown.name_for != Some(ui.bone) {
            let name = ui
                .bone
                .and_then(|b| rig.bone(b))
                .map_or(String::new(), |b| b.name.clone());
            signals.set("rig_bone_name", name);
            ui.shown.name_for = Some(ui.bone);
            changed = true;
        }
        let frame = editor.frame;
        if ui.shown.easing_for.map(|e| e.1) != Some(frame)
            || ui.shown.easing_for.map(|e| e.0) != Some(editor.revision)
        {
            let easing = rig
                .frames()
                .get(frame)
                .and_then(|f| f.key.as_ref())
                .map_or(Easing::Linear, |k| k.easing);
            set_if_changed(&mut signals, "rig_easing", easing.name().to_string());
            ui.shown.easing_for = Some((editor.revision, frame));
        }
        let frames = doc.frame_count();
        if ui.shown.range_for.map(|r| r.1) != Some(frames) {
            signals.set("rig_from", "1".to_string());
            signals.set("rig_to", frames.to_string());
            ui.shown.range_for = Some((editor.generation, frames));
        }
        set_if_changed(
            &mut signals,
            "rig_frame_info",
            frame_info(rig, frame, frames),
        );
        let source = rig.source_frame() + 1;
        set_if_changed(
            &mut signals,
            "rig_rest_text",
            format!("Rest art: frame {source}"),
        );
        let hint = match ui.mode {
            Some(RigMode::Bones) => {
                "Drag to draw a bone; start on the selected bone's end to join it, or set its Parent below"
            }
            Some(RigMode::Pose) if frame == rig.source_frame() => {
                "Rest frame: the bones move, the art stays"
            }
            Some(RigMode::Pose) if ui.ik => "Drag a bone to turn it, its end to bend",
            Some(RigMode::Pose) => "Drag a bone to turn it, its head to move it",
            None => "Bones draws the rig, Pose poses it",
        };
        set_if_changed(&mut signals, "rig_hint", hint.to_string());
        let thumbs_key = (editor.generation, editor.revision, rig.library.len());
        if ui.shown.thumbs != Some(thumbs_key) {
            draw_thumbs(doc, ui.skeleton.expect("a rig"), rig, &canvases);
            ui.shown.thumbs = Some(thumbs_key);
        }
    }
    changed |= draw_bones(&editor, ui, &view, &colors, rig);
    if changed {
        crate::repaint_soon(&wake);
    }
}

/// The colors bones draw in, from the theme's probes.
struct BoneColors {
    fill: Rgba,
    edge: Rgba,
    active: Rgba,
    pivot: Rgba,
    link: Rgba,
}

fn probe(id: &str) -> Option<Rgba> {
    dom::get_by_id(id)?
        .computed_style_of("bg")
        .as_deref()
        .and_then(parse_hex)
}

fn bone_colors() -> Option<BoneColors> {
    Some(BoneColors {
        fill: probe("probe-bone")?,
        edge: probe("probe-bone-edge")?,
        active: probe("probe-bone-active")?,
        pivot: probe("probe-bone-pivot")?,
        link: probe("probe-bone-link")?,
    })
}

fn color(c: Rgba) -> lumenui_canvas::Rgba {
    lumenui_canvas::Rgba::new(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        f64::from(c.a) / 255.0,
    )
}

/// Feeds what the bones' drawing depends on to `h`.
fn hash_bones(
    h: &mut impl std::hash::Hasher,
    editor: &Editor,
    ui: &RigUi,
    view: &View,
    colors: &ViewColors,
) {
    use std::hash::Hash;
    view.revision.hash(h);
    colors.revision.hash(h);
    editor.generation.hash(h);
    editor.revision.hash(h);
    ui.mode.map(|m| m as u8).hash(h);
    ui.bone.hash(h);
    ui.skeleton.hash(h);
    format!("{:?}", ui.pose).hash(h);
    if let Some(Drag::NewBone { head, tail, .. }) = &ui.drag {
        format!("{head:?}{tail:?}").hash(h);
    }
}

/// Draws the bones on the `rig` canvas when what it shows changed.
fn draw_bones(
    editor: &Editor,
    ui: &mut RigUi,
    view: &View,
    colors: &ViewColors,
    rig: Option<&Rig>,
) -> bool {
    use std::hash::Hasher;
    let size = (view.stage.1.x, view.stage.1.y);
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return false;
    }
    // Bones show while the panel works on them, or with the skeleton layer
    // active. With none to show the canvas is cleared once, not on every
    // change of the view or the document.
    let active_layer = Some(editor.layer) == ui.skeleton;
    let showing = rig.is_some() && (ui.mode.is_some() || active_layer);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if showing {
        hash_bones(&mut h, editor, ui, view, colors);
    }
    let key = if showing { h.finish() | 1 } else { 0 };
    let ctx = Context2d::new("rig");
    if ui.shown.size != size {
        ctx.resize(size.0, size.1);
        ui.shown.size = size;
        ui.shown.canvas = None;
    }
    if ui.shown.canvas == Some(key) {
        return false;
    }
    ui.shown.canvas = Some(key);
    ctx.clear();
    if !showing {
        return true;
    }
    let (Some(rig), Some(c)) = (rig, bone_colors()) else {
        return true;
    };
    let bones = shown_bones(rig, ui);
    let z = view.zoom;
    let width = (z * 0.6).clamp(3.0, 10.0);
    // Links from a parent's end to a child that starts elsewhere.
    ctx.set_line_width(1.0);
    ctx.set_stroke_color(color(c.link));
    for &(id, head, _) in &bones {
        let Some(parent) = rig.bone(id).and_then(|b| b.parent) else {
            continue;
        };
        let Some(&(_, _, ptail)) = bones.iter().find(|b| b.0 == parent) else {
            continue;
        };
        if (ptail - head).length() > 0.25 {
            let (ax, ay) = to_stage(view, ptail);
            let (bx, by) = to_stage(view, head);
            ctx.begin_path();
            ctx.move_to(ax, ay);
            ctx.line_to(bx, by);
            ctx.stroke();
        }
    }
    for &(id, head, tail) in &bones {
        let selected = Some(id) == ui.bone;
        let (hx, hy) = to_stage(view, head);
        let (tx, ty) = to_stage(view, tail);
        let (dx, dy) = (tx - hx, ty - hy);
        let len = (dx * dx + dy * dy).sqrt();
        ctx.set_fill_color(color(if selected { c.active } else { c.fill }));
        ctx.set_stroke_color(color(c.edge));
        ctx.set_line_width(1.0);
        if len > 0.5 {
            // A long diamond, widest a fifth of the way along.
            let (ux, uy) = (dx / len, dy / len);
            let w = width.min(len / 3.0);
            let (mx, my) = (hx + dx * 0.2, hy + dy * 0.2);
            ctx.begin_path();
            ctx.move_to(hx, hy);
            ctx.line_to(mx - uy * w, my + ux * w);
            ctx.line_to(tx, ty);
            ctx.line_to(mx + uy * w, my - ux * w);
            ctx.close_path();
            ctx.fill();
            ctx.stroke();
        }
        for (x, y, r) in [(hx, hy, 3.5), (tx, ty, 2.5)] {
            ctx.begin_path();
            ctx.arc(x, y, r, 0.0, std::f64::consts::TAU);
            ctx.fill();
            ctx.stroke();
        }
    }
    if let Some(Drag::NewBone { head, tail, .. }) = &ui.drag {
        let (hx, hy) = to_stage(view, *head);
        let (tx, ty) = to_stage(view, *tail);
        ctx.set_stroke_color(color(c.active));
        ctx.set_line_width(2.0);
        ctx.begin_path();
        ctx.move_to(hx, hy);
        ctx.line_to(tx, ty);
        ctx.stroke();
    }
    if ui.mode == Some(RigMode::Bones)
        && let Some(sel) = ui.bone
    {
        ctx.set_fill_color(color(c.pivot));
        ctx.set_stroke_color(color(c.edge));
        for b in rig.bindings().iter().filter(|b| b.bone == sel) {
            let (x, y) = to_stage(view, b.pivot);
            let r = 4.5;
            ctx.begin_path();
            ctx.move_to(x, y - r);
            ctx.line_to(x + r, y);
            ctx.line_to(x, y + r);
            ctx.line_to(x - r, y);
            ctx.close_path();
            ctx.fill();
            ctx.stroke();
        }
    }
    true
}

/// Draws each saved pose's thumbnail: the bound parts in that pose.
fn draw_thumbs(doc: &Document, skeleton: LayerId, rig: &Rig, canvases: &Query<&Canvas>) {
    let Ok(parts) = Parts::new(doc, skeleton) else {
        return;
    };
    for (k, saved) in rig.library.iter().enumerate() {
        let id = format!("rig-thumb-{k}");
        if !canvases.iter().any(|c| c.id == id) {
            continue;
        }
        let posed = parts.render(rig, &saved.pose);
        let area = posed.iter().fold(pixl_document::Rect::EMPTY, |a, p| {
            a.union(&pixl_document::Rect::new(
                p.origin.x,
                p.origin.y,
                p.pixels.width as i32,
                p.pixels.height as i32,
            ))
        });
        let ctx = Context2d::new(id);
        ctx.resize(THUMB as f32, THUMB as f32);
        ctx.clear();
        if area.is_empty() {
            continue;
        }
        let (w, h) = (area.w as u32, area.h as u32);
        let mut rgba = vec![Rgba::TRANSPARENT; (w * h) as usize];
        for p in &posed {
            for y in 0..p.pixels.height as i32 {
                for x in 0..p.pixels.width as i32 {
                    let v = p.pixels.get(x, y);
                    if v == p.clear {
                        continue;
                    }
                    let c = match doc.mode() {
                        ColorMode::Rgba => Rgba::from_u32(v),
                        ColorMode::Indexed => doc.palette().get(v as u8),
                    };
                    let i = ((p.origin.y + y - area.y) as u32 * w
                        + (p.origin.x + x - area.x) as u32) as usize;
                    rgba[i] = pixl_document::composite::blend(
                        rgba[i],
                        c,
                        pixl_document::BlendMode::Normal,
                        255,
                    );
                }
            }
        }
        let bytes: Vec<u8> = rgba.iter().flat_map(|c| [c.r, c.g, c.b, c.a]).collect();
        let Ok(buf) = PixelBuffer::new(w, h) else {
            continue;
        };
        let _ = buf.write_rgba8(0, 0, w, h, &bytes);
        let scale = (f64::from(THUMB) / f64::from(w.max(h))).min(4.0);
        let (dw, dh) = (f64::from(w) * scale, f64::from(h) * scale);
        ctx.set_image_smoothing(false);
        ctx.draw_buffer_scaled(
            &buf,
            (f64::from(THUMB) - dw) / 2.0,
            (f64::from(THUMB) - dh) / 2.0,
            dw,
            dh,
        );
    }
}

/// The rig's resource and systems, ordered among the editor's own.
pub fn install(app: App) -> App {
    use lumenui::lumen_core::input::ClickSet;
    use lumenui::lumen_core::signals;
    app.insert_resource(RigUi::default())
        .insert_signal("rig_none", true)
        .insert_signal("rig_some", false)
        .insert_signal("rig_bones", false)
        .insert_signal("rig_posing", false)
        .insert_signal("rig_has_bone", false)
        .insert_signal("rig_bone_name", "")
        .insert_signal("rig_pose_name", "")
        .insert_signal("rig_easing", "Linear")
        .insert_signal("rig_from", "1")
        .insert_signal("rig_to", "1")
        .add_systems(
            TickStage::Systems,
            (
                crate::perf::timed("rig::rig_clicks", rig_clicks),
                crate::perf::timed("rig::rig_controls", rig_controls),
                crate::perf::timed("rig::use_rig", use_rig),
            )
                .chain()
                .after(crate::panels::panel_clicks)
                .after(crate::tools::pick_tool)
                .before(crate::tools::use_tools)
                .after(ClickSet::Producers)
                .after(signals::push_textinput_to_signal)
                .before(signals::apply_text_bindings)
                .before(signals::apply_value_bindings),
        )
        .add_systems(
            TickStage::Systems,
            crate::perf::timed("rig::show_rig", show_rig)
                .after(crate::view::draw_stage)
                .before(signals::apply_text_bindings)
                .before(signals::apply_checked_bindings)
                .before(signals::apply_value_bindings)
                .before(CanvasSet::Encode),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_snap_to_pixel_centers_and_corners() {
        assert_eq!(snap(Vec2::new(3.3, 4.8)), Vec2::new(3.5, 5.0));
        assert_eq!(snap(Vec2::new(-0.2, 0.74)), Vec2::new(0.0, 0.5));
    }

    #[test]
    fn parent_choices_round_trip_through_their_labels() {
        let mut rig = Rig::new(1);
        let a = rig
            .add_bone("arm", None, Vec2::ZERO, Vec2::new(1.0, 0.0))
            .unwrap();
        assert_eq!(
            parent_choice(&rig, &parent_label(&rig, Some(a))),
            Some(Some(a))
        );
        assert_eq!(parent_choice(&rig, "No parent"), Some(None));
        assert_eq!(parent_choice(&rig, "nothing"), None);
    }

    #[test]
    fn frame_info_names_rest_keys_and_generated_frames() {
        let mut rig = Rig::new(3);
        rig.frame_mut(1).unwrap().key = Some(PoseKey::default());
        rig.frame_mut(1).unwrap().generated = true;
        assert_eq!(frame_info(&rig, 0, 3), "Frame 1 of 3 - rest art");
        assert_eq!(
            frame_info(&rig, 1, 3),
            "Frame 2 of 3 - keyed, linear - generated"
        );
    }
}
