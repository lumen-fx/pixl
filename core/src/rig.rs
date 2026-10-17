//! Skeletons and pose-to-frame.
//!
//! A skeleton layer holds a [`Rig`]: a tree of bones in their rest pose, the
//! parts of raster layers bound to them, the pose being edited, a pose
//! library, and poses keyed on frames. Rendering a pose cuts every bound part
//! out of the rig's source frame and rasterizes it at its bone's posed
//! transform with RotSprite ([`Pixels::rotsprite`]'s sampling; multiples of
//! 90 degrees turn exactly, nearest neighbor), into the frame's cels of the
//! same layers. The result is ordinary pixels the artist touches up.
//!
//! Coordinates are document pixels with y down; angles are degrees,
//! clockwise on screen, 0 pointing right. Rest positions are absolute, so
//! re-parenting a bone does not move it. A pose stores, per bone, a rotation
//! about the bone's head and an offset, both relative to the rest pose and
//! applied in the parent's posed space; a bone's posed transform is a rigid
//! motion of the rest-pose canvas.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::{Add, Mul, Sub};
use std::sync::{Arc, OnceLock};

use crate::color::{ColorMode, Rgba};
use crate::composite::blend;
use crate::document::{BlendMode, Document, LayerId, LayerKind};
use crate::error::Error;
use crate::geom::{Point, Rect};
use crate::image::Raster;
use crate::selection::Selection;
use crate::transform::Pixels;

/// A bone's id, unique within its rig.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoneId(pub u32);

/// A point or vector in document pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// The unit vector at `degrees`.
    pub fn from_angle(degrees: f64) -> Self {
        let (s, c) = sin_cos(degrees);
        Self::new(c, s)
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    /// The direction in degrees, in (-180, 180].
    pub fn angle(self) -> f64 {
        normalize_angle(self.y.atan2(self.x).to_degrees())
    }

    /// The vector turned by `degrees`.
    pub fn rotated(self, degrees: f64) -> Self {
        let (s, c) = sin_cos(degrees);
        Self::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }

    /// The center of a pixel.
    pub fn center_of(p: Point) -> Self {
        Self::new(p.x as f64 + 0.5, p.y as f64 + 0.5)
    }

    fn cross(self, o: Vec2) -> f64 {
        self.x * o.y - self.y * o.x
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f64) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}

/// Sine and cosine of `degrees`, exact at multiples of 90 so quarter turns
/// land on whole pixels.
fn sin_cos(degrees: f64) -> (f64, f64) {
    let q = degrees / 90.0;
    if (q - q.round()).abs() < 1e-9 {
        match (q.round() as i64).rem_euclid(4) {
            0 => (0.0, 1.0),
            1 => (1.0, 0.0),
            2 => (0.0, -1.0),
            _ => (-1.0, 0.0),
        }
    } else {
        degrees.to_radians().sin_cos()
    }
}

/// `degrees` brought into (-180, 180].
pub fn normalize_angle(degrees: f64) -> f64 {
    let a = degrees.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// A rigid motion: turn by `angle` about the origin, then move by `offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rigid {
    pub angle: f64,
    pub offset: Vec2,
}

impl Rigid {
    pub const IDENTITY: Rigid = Rigid {
        angle: 0.0,
        offset: Vec2::ZERO,
    };

    pub fn translation(v: Vec2) -> Self {
        Self {
            angle: 0.0,
            offset: v,
        }
    }

    /// A turn by `angle` about `center`.
    pub fn rotation_about(center: Vec2, angle: f64) -> Self {
        Self {
            angle,
            offset: center - center.rotated(angle),
        }
    }

    pub fn apply(&self, p: Vec2) -> Vec2 {
        p.rotated(self.angle) + self.offset
    }

    /// `inner` first, then `self`.
    pub fn after(&self, inner: &Rigid) -> Rigid {
        Rigid {
            angle: self.angle + inner.angle,
            offset: self.apply(inner.offset),
        }
    }

    pub fn inverse(&self) -> Rigid {
        Rigid {
            angle: -self.angle,
            offset: (Vec2::ZERO - self.offset).rotated(-self.angle),
        }
    }
}

/// One bone in its rest pose.
#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub id: BoneId,
    pub name: String,
    pub parent: Option<BoneId>,
    /// Where the bone starts, in document pixels.
    pub head: Vec2,
    pub length: f64,
    /// The bone's direction, in degrees.
    pub angle: f64,
}

impl Bone {
    pub fn tail(&self) -> Vec2 {
        self.head + Vec2::from_angle(self.angle) * self.length
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RegionBits {
    bounds: Rect,
    bits: Vec<bool>,
}

/// A hard-edged set of pixels, the part of a layer one binding moves.
/// Cheap to clone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region(Arc<RegionBits>);

impl Region {
    /// Every pixel of `rect`.
    pub fn rect(rect: Rect) -> Self {
        let n = (rect.w.max(0) * rect.h.max(0)) as usize;
        Self(Arc::new(RegionBits {
            bounds: rect,
            bits: vec![true; n],
        }))
    }

    /// The pixels of a selection, or `None` when nothing is selected.
    pub fn from_selection(sel: &Selection) -> Option<Self> {
        let b = sel.bounds();
        if b.is_empty() {
            return None;
        }
        let bits = b.points().map(|p| sel.contains(p.x, p.y)).collect();
        Some(Self(Arc::new(RegionBits { bounds: b, bits })))
    }

    /// A region from its bounds and one flag per pixel, row by row.
    pub fn from_bits(bounds: Rect, bits: Vec<bool>) -> Option<Self> {
        let n = (bounds.w.max(0) as usize) * (bounds.h.max(0) as usize);
        (bits.len() == n).then(|| Self(Arc::new(RegionBits { bounds, bits })))
    }

    pub fn bounds(&self) -> Rect {
        self.0.bounds
    }

    /// One flag per pixel of [`Region::bounds`], row by row.
    pub fn bits(&self) -> &[bool] {
        &self.0.bits
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        let b = self.0.bounds;
        if x < b.x || y < b.y || x >= b.right() || y >= b.bottom() {
            return false;
        }
        self.0.bits[((y - b.y) * b.w + (x - b.x)) as usize]
    }
}

/// A rigid part: a raster layer, or a region of one, that follows one bone.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub layer: LayerId,
    /// The pixels this binding moves; `None` takes every pixel of the layer
    /// no region binding of the same layer takes.
    pub region: Option<Region>,
    pub bone: BoneId,
    /// The point of the part that stays on whole pixels as it moves, so a
    /// posed part does not shimmer between frames.
    pub pivot: Vec2,
}

/// How one bone differs from its rest pose.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BonePose {
    /// Degrees, about the bone's head.
    pub rotation: f64,
    /// Pixels, in the parent's posed space.
    pub offset: Vec2,
}

impl BonePose {
    fn is_rest(&self) -> bool {
        self.rotation == 0.0 && self.offset == Vec2::ZERO
    }
}

/// A pose of a rig: each bone's change from rest. Bones it does not list
/// stay at rest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pose {
    bones: BTreeMap<BoneId, BonePose>,
}

impl Pose {
    pub fn get(&self, bone: BoneId) -> BonePose {
        self.bones.get(&bone).copied().unwrap_or_default()
    }

    pub fn set(&mut self, bone: BoneId, pose: BonePose) {
        if pose.is_rest() {
            self.bones.remove(&bone);
        } else {
            self.bones.insert(bone, pose);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (BoneId, BonePose)> + '_ {
        self.bones.iter().map(|(k, v)| (*k, *v))
    }

    pub fn is_rest(&self) -> bool {
        self.bones.is_empty()
    }

    /// The pose a fraction `t` of the way from `a` to `b`: angles take the
    /// shorter way round, offsets move in a straight line.
    pub fn lerp(a: &Pose, b: &Pose, t: f64) -> Pose {
        let ids: BTreeSet<BoneId> = a.bones.keys().chain(b.bones.keys()).copied().collect();
        let mut out = Pose::default();
        for id in ids {
            let (pa, pb) = (a.get(id), b.get(id));
            let turn = normalize_angle(pb.rotation - pa.rotation);
            out.set(
                id,
                BonePose {
                    rotation: normalize_angle(pa.rotation + turn * t),
                    offset: pa.offset + (pb.offset - pa.offset) * t,
                },
            );
        }
        out
    }
}

/// A saved pose.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedPose {
    pub name: String,
    pub pose: Pose,
}

/// How a keyed pose moves toward the next key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// Holds the pose until the next key.
    Hold,
}

impl Easing {
    pub const ALL: [Easing; 5] = [
        Easing::Linear,
        Easing::EaseIn,
        Easing::EaseOut,
        Easing::EaseInOut,
        Easing::Hold,
    ];

    /// The eased fraction for a linear fraction `t` in [0, 1].
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
            Easing::Hold => 0.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Easing::Linear => "Linear",
            Easing::EaseIn => "Ease in",
            Easing::EaseOut => "Ease out",
            Easing::EaseInOut => "Ease in-out",
            Easing::Hold => "Hold",
        }
    }

    pub fn from_name(name: &str) -> Option<Easing> {
        Easing::ALL.into_iter().find(|e| e.name() == name)
    }
}

/// A pose keyed on a frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoseKey {
    pub pose: Pose,
    /// How the pose moves toward the next key.
    pub easing: Easing,
}

/// What the rig knows about one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RigFrame {
    pub key: Option<PoseKey>,
    /// The frame's bound cels were rendered from the rig, so rendering again
    /// may overwrite them.
    pub generated: bool,
}

/// Which way a two-bone chain bends: the turn from the upper bone to the
/// lower one, as seen on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bend {
    Clockwise,
    CounterClockwise,
}

/// A bone in a pose, in document pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PosedBone {
    pub id: BoneId,
    pub head: Vec2,
    pub tail: Vec2,
    pub angle: f64,
}

/// A skeleton: bones, bound parts, poses.
#[derive(Clone, Debug, PartialEq)]
pub struct Rig {
    bones: Vec<Bone>,
    bindings: Vec<Binding>,
    /// The pose being edited.
    pub pose: Pose,
    /// Saved poses.
    pub library: Vec<NamedPose>,
    frames: Vec<RigFrame>,
    source_frame: usize,
    next_bone: u32,
}

impl Rig {
    /// An empty rig for a document of `frames` frames, drawn on frame 0.
    pub fn new(frames: usize) -> Self {
        Self {
            bones: Vec::new(),
            bindings: Vec::new(),
            pose: Pose::default(),
            library: Vec::new(),
            frames: vec![RigFrame::default(); frames],
            source_frame: 0,
            next_bone: 1,
        }
    }

    /// Bones in the order they were added.
    pub fn bones(&self) -> &[Bone] {
        &self.bones
    }

    pub fn bone(&self, id: BoneId) -> Option<&Bone> {
        self.bones.iter().find(|b| b.id == id)
    }

    pub fn bone_named(&self, name: &str) -> Option<&Bone> {
        self.bones.iter().find(|b| b.name == name)
    }

    fn bone_index(&self, id: BoneId) -> Result<usize, Error> {
        self.bones
            .iter()
            .position(|b| b.id == id)
            .ok_or(Error::BoneNotFound(id))
    }

    /// The bones holding `id`, innermost first.
    pub fn ancestors(&self, id: BoneId) -> Vec<BoneId> {
        let mut out = Vec::new();
        let mut at = self.bone(id).and_then(|b| b.parent);
        while let Some(p) = at {
            if out.contains(&p) || out.len() > self.bones.len() {
                break;
            }
            out.push(p);
            at = self.bone(p).and_then(|b| b.parent);
        }
        out
    }

    /// Adds a bone from `head` to `tail` and returns its id.
    pub fn add_bone(
        &mut self,
        name: impl Into<String>,
        parent: Option<BoneId>,
        head: Vec2,
        tail: Vec2,
    ) -> Result<BoneId, Error> {
        if let Some(p) = parent {
            self.bone_index(p)?;
        }
        let id = BoneId(self.next_bone);
        self.next_bone += 1;
        let d = tail - head;
        self.bones.push(Bone {
            id,
            name: name.into(),
            parent,
            head,
            length: d.length(),
            angle: if d.length() > 0.0 { d.angle() } else { 0.0 },
        });
        Ok(id)
    }

    /// A name no bone has yet: `base`, else `base 2`, `base 3`...
    pub fn unique_name(&self, base: &str) -> String {
        if self.bone_named(base).is_none() {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|n| self.bone_named(n).is_none())
            .expect("a free name exists")
    }

    /// Deletes a bone. Its children move to its parent and keep their rest
    /// pose; parts bound to it are unbound.
    pub fn remove_bone(&mut self, id: BoneId) -> Result<(), Error> {
        let i = self.bone_index(id)?;
        let parent = self.bones[i].parent;
        self.bones.remove(i);
        for b in &mut self.bones {
            if b.parent == Some(id) {
                b.parent = parent;
            }
        }
        self.bindings.retain(|b| b.bone != id);
        self.pose.bones.remove(&id);
        for p in &mut self.library {
            p.pose.bones.remove(&id);
        }
        for f in &mut self.frames {
            if let Some(k) = &mut f.key {
                k.pose.bones.remove(&id);
            }
        }
        Ok(())
    }

    pub fn rename_bone(&mut self, id: BoneId, name: impl Into<String>) -> Result<(), Error> {
        let i = self.bone_index(id)?;
        self.bones[i].name = name.into();
        Ok(())
    }

    /// Moves a bone under another (or to the root). Its rest pose stays put.
    pub fn set_parent(&mut self, id: BoneId, parent: Option<BoneId>) -> Result<(), Error> {
        let i = self.bone_index(id)?;
        if let Some(p) = parent {
            self.bone_index(p)?;
            if p == id || self.ancestors(p).contains(&id) {
                return Err(Error::CyclicBone(id));
            }
        }
        self.bones[i].parent = parent;
        Ok(())
    }

    /// Sets a bone's rest pose from its head and tail.
    pub fn set_rest(&mut self, id: BoneId, head: Vec2, tail: Vec2) -> Result<(), Error> {
        let i = self.bone_index(id)?;
        let d = tail - head;
        let b = &mut self.bones[i];
        b.head = head;
        b.length = d.length();
        if d.length() > 0.0 {
            b.angle = d.angle();
        }
        Ok(())
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Binds a part to a bone and returns the binding's index. A whole-layer
    /// binding replaces the layer's earlier whole-layer binding.
    pub fn bind(&mut self, binding: Binding) -> Result<usize, Error> {
        self.bone_index(binding.bone)?;
        if binding.region.is_none()
            && let Some(i) = self
                .bindings
                .iter()
                .position(|b| b.layer == binding.layer && b.region.is_none())
        {
            self.bindings[i] = binding;
            return Ok(i);
        }
        self.bindings.push(binding);
        Ok(self.bindings.len() - 1)
    }

    pub fn unbind(&mut self, index: usize) -> Option<Binding> {
        (index < self.bindings.len()).then(|| self.bindings.remove(index))
    }

    /// Edits a binding (its bone, pivot or region).
    pub fn update_binding(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut Binding),
    ) -> Result<(), Error> {
        let mut b = self
            .bindings
            .get(index)
            .cloned()
            .ok_or(Error::BindingNotFound(index))?;
        edit(&mut b);
        self.bone_index(b.bone)?;
        self.bindings[index] = b;
        Ok(())
    }

    pub(crate) fn retain_layers(&mut self, keep: impl Fn(LayerId) -> bool) {
        self.bindings.retain(|b| keep(b.layer));
    }

    /// One entry per document frame.
    pub fn frames(&self) -> &[RigFrame] {
        &self.frames
    }

    pub fn frame_mut(&mut self, frame: usize) -> Result<&mut RigFrame, Error> {
        self.frames
            .get_mut(frame)
            .ok_or(Error::FrameOutOfRange(frame))
    }

    /// The frame holding the art in rest pose, the one renders read.
    pub fn source_frame(&self) -> usize {
        self.source_frame
    }

    pub fn set_source_frame(&mut self, frame: usize) -> Result<(), Error> {
        if frame >= self.frames.len() {
            return Err(Error::FrameOutOfRange(frame));
        }
        self.source_frame = frame;
        Ok(())
    }

    pub(crate) fn frame_inserted(&mut self, at: usize) {
        self.frames.insert(at, RigFrame::default());
        if self.source_frame >= at {
            self.source_frame += 1;
        }
    }

    pub(crate) fn frame_duplicated(&mut self, from: usize, to: usize) {
        self.frames[to] = self.frames[from].clone();
    }

    /// Frames moved: `order[new]` is the old index of each frame.
    pub(crate) fn frames_reordered(&mut self, order: &[usize]) {
        let old = std::mem::take(&mut self.frames);
        self.frames = order.iter().map(|&o| old[o].clone()).collect();
        if let Some(new) = order.iter().position(|&o| o == self.source_frame) {
            self.source_frame = new;
        }
    }

    pub(crate) fn frame_removed(&mut self, at: usize) {
        self.frames.remove(at);
        if self.source_frame > at || self.source_frame >= self.frames.len() {
            self.source_frame = self.source_frame.saturating_sub(1);
        }
    }

    // ---- posing ----

    /// Every bone's posed transform, a rigid motion of the rest canvas.
    pub fn world(&self, pose: &Pose) -> BTreeMap<BoneId, Rigid> {
        let mut out = BTreeMap::new();
        for b in &self.bones {
            self.world_of(b.id, pose, &mut out, 0);
        }
        out
    }

    fn world_of(
        &self,
        id: BoneId,
        pose: &Pose,
        memo: &mut BTreeMap<BoneId, Rigid>,
        depth: usize,
    ) -> Rigid {
        if let Some(m) = memo.get(&id) {
            return *m;
        }
        let Some(bone) = self.bone(id) else {
            return Rigid::IDENTITY;
        };
        let parent = match bone.parent {
            Some(p) if depth <= self.bones.len() => self.world_of(p, pose, memo, depth + 1),
            _ => Rigid::IDENTITY,
        };
        let m = parent.after(&self.local(bone, pose.get(id)));
        memo.insert(id, m);
        m
    }

    /// A bone's own motion, before its parent's.
    fn local(&self, bone: &Bone, p: BonePose) -> Rigid {
        Rigid::translation(p.offset).after(&Rigid::rotation_about(bone.head, p.rotation))
    }

    /// Every bone where `pose` puts it.
    pub fn posed(&self, pose: &Pose) -> Vec<PosedBone> {
        let world = self.world(pose);
        self.bones
            .iter()
            .map(|b| {
                let m = world[&b.id];
                PosedBone {
                    id: b.id,
                    head: m.apply(b.head),
                    tail: m.apply(b.tail()),
                    angle: normalize_angle(b.angle + m.angle),
                }
            })
            .collect()
    }

    /// The transform of `id`'s parent in `pose` (identity for a root bone).
    pub fn parent_world(&self, id: BoneId, pose: &Pose) -> Rigid {
        match self.bone(id).and_then(|b| b.parent) {
            Some(p) => self.world(pose)[&p],
            None => Rigid::IDENTITY,
        }
    }

    /// Turns bone `id` so its head-to-tail direction points at `target`.
    pub fn aim(&self, pose: &mut Pose, id: BoneId, target: Vec2) -> Result<(), Error> {
        let bone = self.bone(id).ok_or(Error::BoneNotFound(id))?;
        let world = self.world(pose);
        let m = world[&id];
        let head = m.apply(bone.head);
        let d = target - head;
        if d.length() < 1e-9 {
            return Ok(());
        }
        let current = bone.angle + m.angle;
        let mut p = pose.get(id);
        p.rotation = normalize_angle(p.rotation + d.angle() - current);
        pose.set(id, p);
        Ok(())
    }

    /// Moves bone `id` so its head lands on `target`.
    pub fn move_head(&self, pose: &mut Pose, id: BoneId, target: Vec2) -> Result<(), Error> {
        let bone = self.bone(id).ok_or(Error::BoneNotFound(id))?;
        let parent = self.parent_world(id, pose);
        let mut p = pose.get(id);
        // head = parent(bone.head + offset); solve for the offset.
        p.offset = parent.inverse().apply(target) - bone.head;
        pose.set(id, p);
        Ok(())
    }

    /// The bend of the chain ending at `end` in `pose`.
    pub fn bend_of(&self, pose: &Pose, end: BoneId) -> Bend {
        let posed = self.posed(pose);
        let at = |id| posed.iter().find(|b| b.id == id);
        let parent = self.bone(end).and_then(|b| b.parent);
        match (parent.and_then(at), at(end)) {
            (Some(a), Some(b)) => {
                if (b.head - a.head).cross(b.tail - b.head) >= 0.0 {
                    Bend::Clockwise
                } else {
                    Bend::CounterClockwise
                }
            }
            _ => Bend::Clockwise,
        }
    }

    /// Two-bone IK: turns `end` and its parent so `end`'s tail reaches
    /// `target`, or points at it when it is out of reach. The parent's head
    /// stays where it is.
    pub fn solve_ik(
        &self,
        pose: &mut Pose,
        end: BoneId,
        target: Vec2,
        bend: Bend,
    ) -> Result<(), Error> {
        let lower = self.bone(end).ok_or(Error::BoneNotFound(end))?;
        let upper_id = lower.parent.ok_or(Error::NoIkChain(end))?;
        let upper = self.bone(upper_id).ok_or(Error::BoneNotFound(upper_id))?;
        let above = self.parent_world(upper_id, pose);
        let (pu, pl) = (pose.get(upper_id), pose.get(end));
        let base = above.after(&Rigid::translation(pu.offset));
        let a = base.apply(upper.head);
        // The upper segment runs from its head to where the lower bone
        // starts, which need not be the upper bone's own tail.
        let elbow_rest = lower.head + pl.offset;
        let u = elbow_rest - upper.head;
        let (la, lb) = (u.length(), lower.length);
        let to = target - a;
        let d = to.length();
        let gamma = if d > 1e-9 { to.angle() } else { 0.0 };
        let upper_dir = if la < 1e-9 || lb < 1e-9 || d < 1e-9 {
            gamma
        } else {
            let lo = (la - lb).abs();
            let d = d.clamp(lo, la + lb);
            let cos = ((la * la + d * d - lb * lb) / (2.0 * la * d)).clamp(-1.0, 1.0);
            let phi = cos.acos().to_degrees();
            match bend {
                Bend::Clockwise => gamma - phi,
                Bend::CounterClockwise => gamma + phi,
            }
        };
        // Upper: its segment's posed direction is above + rotation + rest.
        let ru = normalize_angle(upper_dir - above.angle - u.angle());
        let theta_a = above.angle + ru;
        let elbow = a + Vec2::from_angle(upper_dir) * la;
        let lower_dir = if (target - elbow).length() > 1e-9 {
            (target - elbow).angle()
        } else {
            theta_a + lower.angle
        };
        let rl = normalize_angle(lower_dir - theta_a - lower.angle);
        pose.set(upper_id, BonePose { rotation: ru, ..pu });
        pose.set(end, BonePose { rotation: rl, ..pl });
        Ok(())
    }

    /// The pose the keys give frame `frame`: a key's own pose on its frame,
    /// eased between keys, held after the last key, and `None` before the
    /// first.
    pub fn pose_at(&self, frame: usize) -> Option<Pose> {
        let keyed = |f: &usize| self.frames[*f].key.is_some();
        let prev = (0..=frame.min(self.frames.len().checked_sub(1)?))
            .rev()
            .find(keyed)?;
        let k0 = self.frames[prev].key.as_ref()?;
        if prev == frame {
            return Some(k0.pose.clone());
        }
        let Some(next) = (frame + 1..self.frames.len()).find(keyed) else {
            return Some(k0.pose.clone());
        };
        let k1 = self.frames[next].key.as_ref()?;
        let t = (frame - prev) as f64 / (next - prev) as f64;
        Some(Pose::lerp(&k0.pose, &k1.pose, k0.easing.apply(t)))
    }

    /// The pose mirrored left to right: each bone takes the pose of its
    /// counterpart by name (`arm.L` and `arm.R`, `leg_l` and `leg_r`...), with
    /// rotations and horizontal offsets reversed. Bones without a
    /// counterpart mirror their own pose.
    pub fn mirror(&self, pose: &Pose) -> Pose {
        let mut out = Pose::default();
        for b in &self.bones {
            let from = mirror_name(&b.name)
                .and_then(|n| self.bone_named(&n))
                .map_or(b.id, |o| o.id);
            let p = pose.get(from);
            out.set(
                b.id,
                BonePose {
                    rotation: normalize_angle(-p.rotation),
                    offset: Vec2::new(-p.offset.x, p.offset.y),
                },
            );
        }
        out
    }

    /// Maps the rig through a canvas transform: `point` maps a position,
    /// `flip` says whether the transform mirrors, `turn` is the degrees it
    /// turns by, and `region` maps a region.
    pub(crate) fn map(
        &mut self,
        point: impl Fn(Vec2) -> Vec2,
        flip: bool,
        turn: f64,
        region: impl Fn(&Region) -> Region,
    ) {
        for b in &mut self.bones {
            let (h, t) = (point(b.head), point(b.tail()));
            b.head = h;
            b.length = (t - h).length();
            b.angle = if b.length > 0.0 {
                (t - h).angle()
            } else {
                normalize_angle(b.angle + turn)
            };
        }
        let pose = |p: &mut Pose| {
            for bp in p.bones.values_mut() {
                let o = point(bp.offset) - point(Vec2::ZERO);
                bp.offset = o;
                if flip {
                    bp.rotation = normalize_angle(-bp.rotation);
                }
            }
        };
        pose(&mut self.pose);
        for p in &mut self.library {
            pose(&mut p.pose);
        }
        for f in &mut self.frames {
            if let Some(k) = &mut f.key {
                pose(&mut k.pose);
            }
        }
        for b in &mut self.bindings {
            b.pivot = point(b.pivot);
            if let Some(r) = &b.region {
                b.region = Some(region(r));
            }
        }
    }

    /// A rig read from a file, checked: bone ids unique, parents present,
    /// no cycles, bindings on known bones.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        bones: Vec<Bone>,
        bindings: Vec<Binding>,
        pose: Pose,
        library: Vec<NamedPose>,
        frames: Vec<RigFrame>,
        source_frame: usize,
        next_bone: u32,
    ) -> Result<Rig, String> {
        let ids: BTreeSet<BoneId> = bones.iter().map(|b| b.id).collect();
        if ids.len() != bones.len() {
            return Err("a bone id appears twice".into());
        }
        let rig = Rig {
            next_bone: next_bone.max(ids.last().map_or(0, |b| b.0).saturating_add(1)),
            bones,
            bindings,
            pose,
            library,
            frames,
            source_frame,
        };
        for b in &rig.bones {
            if let Some(p) = b.parent {
                if !ids.contains(&p) {
                    return Err("a bone's parent is missing".into());
                }
                if rig.ancestors(b.id).contains(&b.id) || rig.ancestors(b.id).len() >= ids.len() {
                    return Err("bones form a cycle".into());
                }
            }
        }
        if rig.bindings.iter().any(|b| !ids.contains(&b.bone)) {
            return Err("a binding names a missing bone".into());
        }
        if rig.source_frame >= rig.frames.len() {
            return Err("the rig's source frame does not exist".into());
        }
        Ok(rig)
    }

    pub(crate) fn next_bone(&self) -> u32 {
        self.next_bone
    }
}

/// The counterpart of a left or right bone name, by the common suffixes:
/// `.L`/`.R`, `_L`/`_R`, `-L`/`-R` (either case) and `Left`/`Right` (as a
/// word ending the name, either case).
pub fn mirror_name(name: &str) -> Option<String> {
    const PAIRS: [(&str, &str); 14] = [
        (".L", ".R"),
        (".l", ".r"),
        ("_L", "_R"),
        ("_l", "_r"),
        ("-L", "-R"),
        ("-l", "-r"),
        (" L", " R"),
        (" l", " r"),
        ("Left", "Right"),
        ("left", "right"),
        ("LEFT", "RIGHT"),
        (".Left", ".Right"),
        ("_left", "_right"),
        (".left", ".right"),
    ];
    for (l, r) in PAIRS {
        if let Some(stem) = name.strip_suffix(l) {
            return Some(format!("{stem}{r}"));
        }
        if let Some(stem) = name.strip_suffix(r) {
            return Some(format!("{stem}{l}"));
        }
    }
    None
}

impl fmt::Display for Easing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ---- rendering ----

/// One rigid part cut out of the source frame.
struct Part {
    layer: LayerId,
    /// The bone it follows; `None` keeps it at rest.
    bone: Option<BoneId>,
    pivot: Vec2,
    origin: Point,
    pixels: Pixels,
    /// The 8x Scale2x upscale RotSprite samples, made on first use.
    big: OnceLock<Pixels>,
}

/// A layer's pixels in a pose, covering `origin` and `pixels`' size; pixels
/// equal to `clear` are empty.
#[derive(Clone, Debug, PartialEq)]
pub struct PosedLayer {
    pub layer: LayerId,
    pub origin: Point,
    pub pixels: Pixels,
    pub clear: u32,
}

/// The bound parts of a rig, cut out of its source frame once and posed any
/// number of times. The editor keeps one while a pose is edited, which is
/// what keeps posing live.
pub struct Parts {
    parts: Vec<Part>,
    /// Bound layers in paint order.
    layers: Vec<LayerId>,
    mode: ColorMode,
    clear: u32,
    bounds: Rect,
}

impl Parts {
    /// Cuts the parts of the rig on skeleton layer `skeleton`. Each layer's
    /// region bindings take their pixels in binding order (a pixel goes to
    /// the first region holding it); the layer's whole-layer binding takes
    /// the rest, which stays at rest when there is none.
    pub fn new(doc: &Document, skeleton: LayerId) -> Result<Parts, Error> {
        let rig = doc.rig(skeleton)?;
        let src = rig.source_frame();
        let mode = doc.mode();
        let clear = match mode {
            ColorMode::Rgba => 0,
            ColorMode::Indexed => doc.transparent_index() as u32,
        };
        let mut parts = Vec::new();
        let mut layers = Vec::new();
        for (_, l) in doc.walk() {
            if !matches!(l.kind, LayerKind::Raster { .. }) {
                continue;
            }
            let bound: Vec<&Binding> = rig.bindings.iter().filter(|b| b.layer == l.id).collect();
            if bound.is_empty() {
                continue;
            }
            layers.push(l.id);
            let Some(raster) = doc.cel_raster(l.id, src)? else {
                continue;
            };
            let content = raster.content_bounds().intersect(&doc.bounds());
            if content.is_empty() {
                continue;
            }
            let read = |x: i32, y: i32| -> u32 {
                let v = raster.get_raw(x, y);
                match raster {
                    Raster::Rgba(_) if Rgba::from_u32(v).a == 0 => 0,
                    _ => v,
                }
            };
            let cw = content.w as usize;
            let mut taken = vec![false; cw * content.h as usize];
            for b in bound.iter().filter(|b| b.region.is_some()) {
                let region = b.region.as_ref().expect("filtered");
                let r = region.bounds().intersect(&content);
                if r.is_empty() {
                    continue;
                }
                let mut px = Pixels::new(r.w as u32, r.h as u32, clear);
                for (i, p) in r.points().enumerate() {
                    let t = (p.y - content.y) as usize * cw + (p.x - content.x) as usize;
                    if !taken[t] && region.contains(p.x, p.y) {
                        taken[t] = true;
                        px.data[i] = read(p.x, p.y);
                    }
                }
                parts.push(Part {
                    layer: l.id,
                    bone: Some(b.bone),
                    pivot: b.pivot,
                    origin: Point::new(r.x, r.y),
                    pixels: px,
                    big: OnceLock::new(),
                });
            }
            let whole = bound.iter().find(|b| b.region.is_none());
            let mut rest = Pixels::new(content.w as u32, content.h as u32, clear);
            let mut any = false;
            for (i, p) in content.points().enumerate() {
                if !taken[i] {
                    let v = read(p.x, p.y);
                    rest.data[i] = v;
                    any |= v != clear;
                }
            }
            if any {
                parts.push(Part {
                    layer: l.id,
                    bone: whole.map(|b| b.bone),
                    pivot: whole.map_or(Vec2::ZERO, |b| b.pivot),
                    origin: Point::new(content.x, content.y),
                    pixels: rest,
                    big: OnceLock::new(),
                });
            }
        }
        Ok(Parts {
            parts,
            layers,
            mode,
            clear,
            bounds: doc.bounds(),
        })
    }

    /// Bound layers in paint order.
    pub fn layers(&self) -> &[LayerId] {
        &self.layers
    }

    /// Every bound layer's pixels in `pose`, clipped to the canvas. Parts of
    /// one layer land in binding order, later ones over earlier ones.
    pub fn render(&self, rig: &Rig, pose: &Pose) -> Vec<PosedLayer> {
        let world = rig.world(pose);
        let mut out: Vec<PosedLayer> = Vec::new();
        for &layer in &self.layers {
            let placed: Vec<(&Part, Rigid, Rect)> = self
                .parts
                .iter()
                .filter(|p| p.layer == layer)
                .map(|p| {
                    let m = p
                        .bone
                        .and_then(|b| world.get(&b).copied())
                        .unwrap_or(Rigid::IDENTITY);
                    let m = snapped(m, p.pivot);
                    (p, m, footprint(p, &m).intersect(&self.bounds))
                })
                .collect();
            let area = placed
                .iter()
                .fold(Rect::EMPTY, |acc, (_, _, r)| acc.union(r));
            let mut posed = PosedLayer {
                layer,
                origin: Point::new(area.x, area.y),
                pixels: Pixels::new(area.w.max(0) as u32, area.h.max(0) as u32, self.clear),
                clear: self.clear,
            };
            for (part, m, rect) in placed {
                self.draw(part, &m, rect, &mut posed);
            }
            out.push(posed);
        }
        out
    }

    fn draw(&self, part: &Part, m: &Rigid, rect: Rect, out: &mut PosedLayer) {
        let inv = m.inverse();
        let (w, h) = (part.pixels.width as i32, part.pixels.height as i32);
        let quarter = {
            let q = m.angle / 90.0;
            (q - q.round()).abs() < 1e-9
        };
        let big = (!quarter).then(|| part.big.get_or_init(|| part.pixels.upscale8()));
        for p in rect.points() {
            let s = inv.apply(Vec2::center_of(p));
            let (lx, ly) = (s.x - part.origin.x as f64, s.y - part.origin.y as f64);
            let v = match big {
                None => {
                    let (sx, sy) = ((lx + 1e-9).floor() as i32, (ly + 1e-9).floor() as i32);
                    if sx < 0 || sy < 0 || sx >= w || sy >= h {
                        continue;
                    }
                    part.pixels.get(sx, sy)
                }
                Some(big) => {
                    let (bx, by) = ((lx * 8.0).floor(), (ly * 8.0).floor());
                    if bx < 0.0 || by < 0.0 || bx >= big.width as f64 || by >= big.height as f64 {
                        continue;
                    }
                    big.get(bx as i32, by as i32)
                }
            };
            if v == self.clear {
                continue;
            }
            let i = ((p.y - out.origin.y) as u32 * out.pixels.width + (p.x - out.origin.x) as u32)
                as usize;
            out.pixels.data[i] = match self.mode {
                ColorMode::Indexed => v,
                ColorMode::Rgba => {
                    let src = Rgba::from_u32(v);
                    let dst = Rgba::from_u32(out.pixels.data[i]);
                    if src.a == 255 || dst.a == 0 {
                        v
                    } else {
                        blend(dst, src, BlendMode::Normal, 255).to_u32()
                    }
                }
            };
        }
    }
}

/// `m` moved by less than a pixel so `pivot` keeps its place within a pixel:
/// a part whose bone only moves lands on whole pixels, and a turning part's
/// pivot does not shimmer.
fn snapped(m: Rigid, pivot: Vec2) -> Rigid {
    let to = m.apply(pivot) - pivot;
    let whole = Vec2::new(to.x.round(), to.y.round());
    // The same turn about the pivot, then a whole-pixel move.
    Rigid::translation(whole).after(&Rigid::rotation_about(pivot, m.angle))
}

/// The canvas pixels a posed part can cover.
fn footprint(part: &Part, m: &Rigid) -> Rect {
    let (x0, y0) = (part.origin.x as f64, part.origin.y as f64);
    let (x1, y1) = (
        x0 + part.pixels.width as f64,
        y0 + part.pixels.height as f64,
    );
    let corners = [
        Vec2::new(x0, y0),
        Vec2::new(x1, y0),
        Vec2::new(x0, y1),
        Vec2::new(x1, y1),
    ]
    .map(|c| m.apply(c));
    let (mut lx, mut ly, mut hx, mut hy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in corners {
        lx = lx.min(c.x);
        ly = ly.min(c.y);
        hx = hx.max(c.x);
        hy = hy.max(c.y);
    }
    // Pixels are sampled at their centers: take those inside the corners.
    let first = |v: f64| (v - 0.5 - 1e-9).ceil() as i32;
    let end = |v: f64| (v - 0.5 + 1e-9).floor() as i32 + 1;
    let (x0, y0) = (first(lx), first(ly));
    Rect::new(x0, y0, end(hx) - x0, end(hy) - y0)
}

/// How a render writes frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderOptions {
    /// A raster layer the rig does not bind, with an empty cel on the
    /// rendered frame, shows the source frame's cel there (linked), so
    /// backgrounds and props carry over.
    pub link_unbound: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { link_unbound: true }
    }
}

impl Document {
    /// Writes posed layers ([`Parts::render`]) into frame `frame`: each
    /// bound layer's cel there is replaced by the posed pixels. Fails on the
    /// rig's source frame and on a locked layer, before changing anything.
    /// Returns the changed area.
    pub fn write_posed(
        &mut self,
        skeleton: LayerId,
        posed: &[PosedLayer],
        frame: usize,
        opts: RenderOptions,
    ) -> Result<Rect, Error> {
        let src = self.rig(skeleton)?.source_frame();
        if frame == src {
            return Err(Error::SourceFrame(frame));
        }
        if frame >= self.frame_count() {
            return Err(Error::FrameOutOfRange(frame));
        }
        for p in posed {
            if self.effective_locks(p.layer)?.pixels {
                return Err(Error::Locked(p.layer));
            }
        }
        let mut dirty = Rect::EMPTY;
        for p in posed {
            dirty = dirty.union(&self.clear_cel(p.layer, frame)?);
            let target = self.target(p.layer, frame)?;
            dirty = dirty.union(&p.pixels.write_to(target.raster, p.origin, Some(p.clear)));
        }
        if opts.link_unbound {
            let bound: BTreeSet<LayerId> = posed.iter().map(|p| p.layer).collect();
            let unbound: Vec<LayerId> = self
                .walk()
                .into_iter()
                .filter(|(_, l)| {
                    matches!(l.kind, LayerKind::Raster { .. }) && !bound.contains(&l.id)
                })
                .map(|(_, l)| l.id)
                .collect();
            for id in unbound {
                if self.cel(id, frame)?.is_none() && self.cel(id, src)?.is_some() {
                    dirty = dirty.union(&self.link_cel(id, src, frame)?);
                }
            }
        }
        Ok(dirty)
    }

    /// Renders `pose` of the rig on `skeleton` into frame `frame` (see
    /// [`Parts`]). Returns the changed area.
    pub fn render_pose(
        &mut self,
        skeleton: LayerId,
        pose: &Pose,
        frame: usize,
        opts: RenderOptions,
    ) -> Result<Rect, Error> {
        let parts = Parts::new(self, skeleton)?;
        let posed = parts.render(self.rig(skeleton)?, pose);
        self.write_posed(skeleton, &posed, frame, opts)
    }

    /// Inserts a frame at `at` (as long as the source frame), keys `pose` on
    /// it, renders it there and marks it generated. Returns the new frame.
    pub fn render_pose_to_new_frame(
        &mut self,
        skeleton: LayerId,
        pose: &Pose,
        at: usize,
        opts: RenderOptions,
    ) -> Result<usize, Error> {
        let src = self.rig(skeleton)?.source_frame();
        let duration = self.frames()[src].duration_ms;
        self.insert_frame(at, duration)?;
        let result = self.render_pose(skeleton, pose, at, opts);
        if let Err(e) = result {
            self.remove_frame(at)?;
            return Err(e);
        }
        let f = self.rig_mut(skeleton)?.frame_mut(at)?;
        f.key = Some(PoseKey {
            pose: pose.clone(),
            easing: Easing::Linear,
        });
        f.generated = true;
        Ok(at)
    }

    /// Renders every frame of `from..=to` the keys give a pose (see
    /// [`Rig::pose_at`]), except the source frame, and marks them
    /// generated. Returns the frames rendered and the changed area.
    pub fn render_keyed(
        &mut self,
        skeleton: LayerId,
        from: usize,
        to: usize,
        opts: RenderOptions,
    ) -> Result<(Vec<usize>, Rect), Error> {
        let frames: Vec<usize> = (from..=to.min(self.frame_count().saturating_sub(1))).collect();
        self.render_frames(skeleton, &frames, opts)
    }

    /// Renders again every frame marked generated.
    pub fn regenerate(
        &mut self,
        skeleton: LayerId,
        opts: RenderOptions,
    ) -> Result<(Vec<usize>, Rect), Error> {
        let frames: Vec<usize> = self
            .rig(skeleton)?
            .frames()
            .iter()
            .enumerate()
            .filter(|(_, f)| f.generated)
            .map(|(i, _)| i)
            .collect();
        self.render_frames(skeleton, &frames, opts)
    }

    fn render_frames(
        &mut self,
        skeleton: LayerId,
        frames: &[usize],
        opts: RenderOptions,
    ) -> Result<(Vec<usize>, Rect), Error> {
        let parts = Parts::new(self, skeleton)?;
        let rig = self.rig(skeleton)?.clone();
        let mut done = Vec::new();
        let mut dirty = Rect::EMPTY;
        for &f in frames {
            if f == rig.source_frame() {
                continue;
            }
            let Some(pose) = rig.pose_at(f) else {
                continue;
            };
            let posed = parts.render(&rig, &pose);
            dirty = dirty.union(&self.write_posed(skeleton, &posed, f, opts)?);
            self.rig_mut(skeleton)?.frame_mut(f)?.generated = true;
            done.push(f);
        }
        Ok((done, dirty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{NewLayer, Placement};

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-6
    }

    /// A 16x16 RGBA document with a raster layer holding a 1x4 vertical bar
    /// at x = 7, y = 4..8, and a skeleton layer.
    fn arm_doc() -> (Document, LayerId, LayerId) {
        let mut d = Document::new(16, 16, ColorMode::Rgba).unwrap();
        let art = d.layers()[0].id;
        {
            let t = d.target(art, 0).unwrap();
            for y in 4..8 {
                t.raster.set_raw(7, y, Rgba::BLACK.to_u32());
            }
        }
        let skel = d
            .add_layer(NewLayer::Skeleton, "Skeleton", Placement::default())
            .unwrap();
        (d, art, skel)
    }

    fn pixels(d: &Document, layer: LayerId, frame: usize) -> Vec<(i32, i32)> {
        let r = d.cel_raster(layer, frame).unwrap().unwrap();
        r.bounds()
            .points()
            .filter(|p| r.get_raw(p.x, p.y) != 0)
            .map(|p| (p.x, p.y))
            .collect()
    }

    #[test]
    fn rigid_motions_compose_and_invert() {
        let a = Rigid::rotation_about(Vec2::new(3.0, 4.0), 90.0);
        assert!(close(a.apply(Vec2::new(4.0, 4.0)), Vec2::new(3.0, 5.0)));
        let b = Rigid::translation(Vec2::new(1.0, -2.0)).after(&a);
        let p = Vec2::new(-7.5, 2.25);
        assert!(close(b.inverse().apply(b.apply(p)), p));
        assert_eq!(normalize_angle(540.0), 180.0);
        assert_eq!(normalize_angle(-190.0), 170.0);
    }

    #[test]
    fn children_follow_their_parent() {
        let mut rig = Rig::new(1);
        let upper = rig
            .add_bone("upper", None, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0))
            .unwrap();
        let lower = rig
            .add_bone(
                "lower",
                Some(upper),
                Vec2::new(10.0, 0.0),
                Vec2::new(20.0, 0.0),
            )
            .unwrap();
        let mut pose = Pose::default();
        pose.set(
            upper,
            BonePose {
                rotation: 90.0,
                offset: Vec2::ZERO,
            },
        );
        let posed = rig.posed(&pose);
        assert!(close(posed[1].head, Vec2::new(0.0, 10.0)));
        assert!(close(posed[1].tail, Vec2::new(0.0, 20.0)));
        assert_eq!(posed[1].angle, 90.0);
        assert_eq!(
            rig.set_parent(upper, Some(lower)),
            Err(Error::CyclicBone(upper))
        );
        rig.remove_bone(upper).unwrap();
        assert_eq!(rig.bone(lower).unwrap().parent, None);
    }

    #[test]
    fn two_bone_ik_reaches_the_target_with_either_bend() {
        let mut rig = Rig::new(1);
        let upper = rig
            .add_bone("thigh", None, Vec2::new(0.0, 0.0), Vec2::new(0.0, 10.0))
            .unwrap();
        let lower = rig
            .add_bone(
                "shin",
                Some(upper),
                Vec2::new(0.0, 10.0),
                Vec2::new(0.0, 18.0),
            )
            .unwrap();
        for bend in [Bend::Clockwise, Bend::CounterClockwise] {
            let mut pose = Pose::default();
            let target = Vec2::new(6.0, 12.0);
            rig.solve_ik(&mut pose, lower, target, bend).unwrap();
            let posed = rig.posed(&pose);
            assert!(
                close(posed[1].tail, target),
                "{bend:?}: {:?}",
                posed[1].tail
            );
            assert!(close(posed[0].head, Vec2::ZERO));
            assert_eq!(rig.bend_of(&pose, lower), bend);
        }
        // Out of reach: the chain straightens toward the target.
        let mut pose = Pose::default();
        rig.solve_ik(&mut pose, lower, Vec2::new(100.0, 0.0), Bend::Clockwise)
            .unwrap();
        let posed = rig.posed(&pose);
        assert!(
            close(posed[1].tail, Vec2::new(18.0, 0.0)),
            "{:?}",
            posed[1].tail
        );
        assert_eq!(
            rig.solve_ik(&mut pose, upper, Vec2::ZERO, Bend::Clockwise),
            Err(Error::NoIkChain(upper))
        );
    }

    #[test]
    fn keyed_poses_interpolate_the_short_way_with_easing() {
        let mut rig = Rig::new(5);
        let b = rig
            .add_bone("b", None, Vec2::ZERO, Vec2::new(4.0, 0.0))
            .unwrap();
        let key = |rot: f64, x: f64, easing| {
            let mut p = Pose::default();
            p.set(
                b,
                BonePose {
                    rotation: rot,
                    offset: Vec2::new(x, 0.0),
                },
            );
            Some(PoseKey { pose: p, easing })
        };
        rig.frames[0].key = key(170.0, 0.0, Easing::Linear);
        rig.frames[4].key = key(-170.0, 8.0, Easing::Linear);
        assert_eq!(rig.pose_at(2).unwrap().get(b).rotation, 180.0);
        assert_eq!(rig.pose_at(2).unwrap().get(b).offset, Vec2::new(4.0, 0.0));
        assert_eq!(rig.pose_at(1).unwrap().get(b).rotation, 175.0);
        assert_eq!(rig.pose_at(4).unwrap().get(b).rotation, -170.0);
        rig.frames[0].key.as_mut().unwrap().easing = Easing::EaseIn;
        assert_eq!(rig.pose_at(2).unwrap().get(b).offset, Vec2::new(2.0, 0.0));
        rig.frames[0].key.as_mut().unwrap().easing = Easing::Hold;
        assert_eq!(rig.pose_at(3).unwrap().get(b).rotation, 170.0);
        rig.frames[0].key = None;
        assert!(rig.pose_at(1).is_none(), "no pose before the first key");
        assert_eq!(rig.pose_at(4).unwrap().get(b).rotation, -170.0);
    }

    #[test]
    fn mirror_swaps_sides_by_name() {
        assert_eq!(mirror_name("arm.L").as_deref(), Some("arm.R"));
        assert_eq!(mirror_name("leg_r").as_deref(), Some("leg_l"));
        assert_eq!(mirror_name("handLeft").as_deref(), Some("handRight"));
        assert_eq!(mirror_name("spine"), None);
        let mut rig = Rig::new(1);
        let l = rig
            .add_bone("arm.L", None, Vec2::ZERO, Vec2::new(-4.0, 0.0))
            .unwrap();
        let r = rig
            .add_bone("arm.R", None, Vec2::ZERO, Vec2::new(4.0, 0.0))
            .unwrap();
        let s = rig
            .add_bone("spine", None, Vec2::ZERO, Vec2::new(0.0, -4.0))
            .unwrap();
        let mut pose = Pose::default();
        pose.set(
            l,
            BonePose {
                rotation: 30.0,
                offset: Vec2::new(1.0, 2.0),
            },
        );
        pose.set(
            s,
            BonePose {
                rotation: 10.0,
                offset: Vec2::ZERO,
            },
        );
        let m = rig.mirror(&pose);
        assert_eq!(m.get(r).rotation, -30.0);
        assert_eq!(m.get(r).offset, Vec2::new(-1.0, 2.0));
        assert!(m.get(l).is_rest());
        assert_eq!(m.get(s).rotation, -10.0);
        assert_eq!(rig.mirror(&m), pose);
    }

    #[test]
    fn quarter_turns_render_exactly() {
        let (mut d, art, skel) = arm_doc();
        let rig = d.rig_mut(skel).unwrap();
        let bone = rig
            .add_bone("arm", None, Vec2::new(7.5, 4.5), Vec2::new(7.5, 8.0))
            .unwrap();
        rig.bind(Binding {
            layer: art,
            region: None,
            bone,
            pivot: Vec2::new(7.5, 4.5),
        })
        .unwrap();
        let mut pose = Pose::default();
        pose.set(
            bone,
            BonePose {
                rotation: -90.0,
                offset: Vec2::ZERO,
            },
        );
        let f = d
            .render_pose_to_new_frame(skel, &pose, 1, RenderOptions::default())
            .unwrap();
        assert_eq!(f, 1);
        // Turned a quarter counterclockwise about its top pixel: the bar now
        // runs right from there.
        assert_eq!(pixels(&d, art, 1), vec![(7, 4), (8, 4), (9, 4), (10, 4)]);
        assert_eq!(pixels(&d, art, 0), vec![(7, 4), (7, 5), (7, 6), (7, 7)]);
        let rf = &d.rig(skel).unwrap().frames()[1];
        assert!(rf.generated && rf.key.is_some());
        // Moving the bone by a fraction snaps to whole pixels.
        pose.set(
            bone,
            BonePose {
                rotation: 0.0,
                offset: Vec2::new(2.4, -1.6),
            },
        );
        d.render_pose(skel, &pose, 1, RenderOptions::default())
            .unwrap();
        assert_eq!(pixels(&d, art, 1), vec![(9, 2), (9, 3), (9, 4), (9, 5)]);
        assert_eq!(
            d.render_pose(skel, &pose, 0, RenderOptions::default()),
            Err(Error::SourceFrame(0))
        );
    }

    #[test]
    fn rotsprite_turns_keep_one_pixel_lines_connected() {
        let mut d = Document::new(32, 32, ColorMode::Rgba).unwrap();
        let art = d.layers()[0].id;
        {
            let t = d.target(art, 0).unwrap();
            for x in 8..24 {
                t.raster.set_raw(x, 16, Rgba::BLACK.to_u32());
            }
        }
        let skel = d
            .add_layer(NewLayer::Skeleton, "Skeleton", Placement::default())
            .unwrap();
        let rig = d.rig_mut(skel).unwrap();
        let bone = rig
            .add_bone("b", None, Vec2::new(8.0, 16.5), Vec2::new(24.0, 16.5))
            .unwrap();
        rig.bind(Binding {
            layer: art,
            region: None,
            bone,
            pivot: Vec2::new(8.5, 16.5),
        })
        .unwrap();
        let mut pose = Pose::default();
        pose.set(
            bone,
            BonePose {
                rotation: 30.0,
                offset: Vec2::ZERO,
            },
        );
        d.render_pose_to_new_frame(skel, &pose, 1, RenderOptions::default())
            .unwrap();
        let px = pixels(&d, art, 1);
        // A one-pixel line keeps about its length, every pixel touches the
        // next (8-connected), and the pivot pixel stays put.
        assert!((14..=18).contains(&px.len()), "{px:?}");
        assert!(px.contains(&(8, 16)), "{px:?}");
        for p in &px {
            let near = px
                .iter()
                .filter(|q| *q != p && (q.0 - p.0).abs() <= 1 && (q.1 - p.1).abs() <= 1)
                .count();
            assert!(near >= 1, "{p:?} is cut off in {px:?}");
        }
        // The far end sits where the bone's tail went.
        let far = px.iter().max_by_key(|p| p.0).unwrap();
        assert!(
            (far.0 - 21).abs() <= 1 && (far.1 - 24).abs() <= 1,
            "{far:?}"
        );
        // Determinism: the same pose renders the same pixels.
        d.render_pose(skel, &pose, 1, RenderOptions::default())
            .unwrap();
        assert_eq!(pixels(&d, art, 1), px);
    }

    #[test]
    fn regions_split_one_layer_into_parts() {
        let (mut d, art, skel) = arm_doc();
        // Background layer, unbound: carried over by a link.
        let bg = d
            .add_layer(
                NewLayer::Raster,
                "bg",
                Placement {
                    parent: None,
                    index: Some(0),
                },
            )
            .unwrap();
        d.target(bg, 0)
            .unwrap()
            .raster
            .set_raw(0, 0, Rgba::WHITE.to_u32());
        let rig = d.rig_mut(skel).unwrap();
        let body = rig
            .add_bone("body", None, Vec2::new(7.5, 4.5), Vec2::new(7.5, 6.5))
            .unwrap();
        let hand = rig
            .add_bone("hand", Some(body), Vec2::new(7.5, 6.5), Vec2::new(7.5, 8.0))
            .unwrap();
        // The lower two pixels follow the hand; the rest follows the body.
        rig.bind(Binding {
            layer: art,
            region: Some(Region::rect(Rect::new(0, 6, 16, 10))),
            bone: hand,
            pivot: Vec2::new(7.5, 6.5),
        })
        .unwrap();
        rig.bind(Binding {
            layer: art,
            region: None,
            bone: body,
            pivot: Vec2::new(7.5, 4.5),
        })
        .unwrap();
        let mut pose = Pose::default();
        pose.set(
            hand,
            BonePose {
                rotation: 90.0,
                offset: Vec2::ZERO,
            },
        );
        pose.set(
            body,
            BonePose {
                rotation: 0.0,
                offset: Vec2::new(1.0, 0.0),
            },
        );
        d.insert_frame(1, 100).unwrap();
        let (done, _) = {
            d.rig_mut(skel).unwrap().frame_mut(1).unwrap().key = Some(PoseKey {
                pose: pose.clone(),
                easing: Easing::Linear,
            });
            d.render_keyed(skel, 0, 1, RenderOptions::default())
                .unwrap()
        };
        assert_eq!(done, vec![1], "the source frame is never rendered over");
        // Body moved right one pixel; the hand turned a quarter clockwise
        // about its top pixel, pointing left.
        assert_eq!(pixels(&d, art, 1), vec![(8, 4), (8, 5), (7, 6), (8, 6)]);
        assert_eq!(d.linked_frames(bg, 1).unwrap(), vec![0, 1]);
        // Touch up, then regenerate: the generated frame is rendered again.
        d.target(art, 1)
            .unwrap()
            .raster
            .set_raw(0, 15, Rgba::BLACK.to_u32());
        d.regenerate(skel, RenderOptions::default()).unwrap();
        assert_eq!(pixels(&d, art, 1), vec![(8, 4), (8, 5), (7, 6), (8, 6)]);
    }

    #[test]
    fn frame_edits_keep_rig_frames_aligned() {
        let (mut d, art, skel) = arm_doc();
        d.insert_frame(1, 100).unwrap();
        d.rig_mut(skel).unwrap().frame_mut(1).unwrap().generated = true;
        d.insert_frame(0, 100).unwrap();
        let rig = d.rig(skel).unwrap();
        assert_eq!(rig.source_frame(), 1);
        assert!(rig.frames()[2].generated);
        d.remove_frame(0).unwrap();
        assert_eq!(d.rig(skel).unwrap().source_frame(), 0);
        assert!(d.rig(skel).unwrap().frames()[1].generated);
        let copy = d.duplicate_frame(1).unwrap();
        assert!(d.rig(skel).unwrap().frames()[copy].generated);
        // Moving frame 0 to the end takes the source frame along.
        d.move_frames(0, 1, d.frame_count() - 1).unwrap();
        let rig = d.rig(skel).unwrap();
        assert_eq!(rig.source_frame(), d.frame_count() - 1);
        assert!(rig.frames()[0].generated && rig.frames()[1].generated);
        // Deleting a bound layer drops its bindings.
        let b = d
            .rig_mut(skel)
            .unwrap()
            .add_bone("b", None, Vec2::ZERO, Vec2::new(1.0, 0.0))
            .unwrap();
        d.rig_mut(skel)
            .unwrap()
            .bind(Binding {
                layer: art,
                region: None,
                bone: b,
                pivot: Vec2::ZERO,
            })
            .unwrap();
        d.add_layer(NewLayer::Raster, "keep", Placement::default())
            .unwrap();
        d.remove_layer(art).unwrap();
        assert!(d.rig(skel).unwrap().bindings().is_empty());
    }

    #[test]
    fn flatten_keeps_the_skeleton_and_drops_stale_bindings() {
        let (mut d, art, skel) = arm_doc();
        let rig = d.rig_mut(skel).unwrap();
        let b = rig
            .add_bone("b", None, Vec2::ZERO, Vec2::new(1.0, 0.0))
            .unwrap();
        rig.bind(Binding {
            layer: art,
            region: None,
            bone: b,
            pivot: Vec2::ZERO,
        })
        .unwrap();
        d.flatten().unwrap();
        assert!(d.rig(skel).unwrap().bindings().is_empty());
        assert_eq!(d.rig(skel).unwrap().bones().len(), 1);
        assert_eq!(d.merge_down_target(skel), Err(Error::NotRaster(skel)));
    }

    #[test]
    fn locked_layers_stop_a_render_before_it_writes() {
        let (mut d, art, skel) = arm_doc();
        let rig = d.rig_mut(skel).unwrap();
        let b = rig
            .add_bone("b", None, Vec2::ZERO, Vec2::new(1.0, 0.0))
            .unwrap();
        rig.bind(Binding {
            layer: art,
            region: None,
            bone: b,
            pivot: Vec2::ZERO,
        })
        .unwrap();
        d.update_layer(art, |p| p.locks.pixels = true).unwrap();
        let frames = d.frame_count();
        assert_eq!(
            d.render_pose_to_new_frame(skel, &Pose::default(), 1, RenderOptions::default()),
            Err(Error::Locked(art))
        );
        assert_eq!(d.frame_count(), frames, "the inserted frame is taken back");
    }

    #[test]
    fn parts_pose_many_times_without_recutting() {
        let (mut d, art, skel) = arm_doc();
        let rig = d.rig_mut(skel).unwrap();
        let b = rig
            .add_bone("b", None, Vec2::new(7.5, 4.0), Vec2::new(7.5, 8.0))
            .unwrap();
        rig.bind(Binding {
            layer: art,
            region: None,
            bone: b,
            pivot: Vec2::new(7.5, 4.0),
        })
        .unwrap();
        let parts = Parts::new(&d, skel).unwrap();
        let rig = d.rig(skel).unwrap();
        assert_eq!(parts.layers(), &[art]);
        let rest = parts.render(rig, &Pose::default());
        assert_eq!(rest[0].origin, Point::new(7, 4));
        assert_eq!((rest[0].pixels.width, rest[0].pixels.height), (1, 4));
        for deg in [15.0, 45.0, 180.0] {
            let mut pose = Pose::default();
            pose.set(
                b,
                BonePose {
                    rotation: deg,
                    offset: Vec2::ZERO,
                },
            );
            let posed = parts.render(rig, &pose);
            let n = posed[0].pixels.data.iter().filter(|&&v| v != 0).count();
            assert!((3..=6).contains(&n), "{deg}: {n}");
        }
    }

    #[test]
    fn canvas_transforms_carry_the_rig() {
        use crate::transform::{CanvasTransform, transform_canvas};
        let (mut d, art, skel) = arm_doc();
        let rig = d.rig_mut(skel).unwrap();
        let b = rig
            .add_bone("b", None, Vec2::new(7.5, 4.5), Vec2::new(7.5, 8.0))
            .unwrap();
        rig.bind(Binding {
            layer: art,
            region: Some(Region::rect(Rect::new(7, 4, 1, 2))),
            bone: b,
            pivot: Vec2::new(7.5, 4.5),
        })
        .unwrap();
        let mut pose = Pose::default();
        pose.set(
            b,
            BonePose {
                rotation: 90.0,
                offset: Vec2::new(1.0, 0.0),
            },
        );
        d.rig_mut(skel).unwrap().pose = pose.clone();
        transform_canvas(&mut d, CanvasTransform::FlipH).unwrap();
        let rig = d.rig(skel).unwrap();
        let bone = rig.bone(b).unwrap();
        assert!(close(bone.head, Vec2::new(8.5, 4.5)));
        assert_eq!(bone.angle, 90.0);
        assert_eq!(rig.pose.get(b).rotation, -90.0);
        assert_eq!(rig.pose.get(b).offset, Vec2::new(-1.0, 0.0));
        assert_eq!(
            rig.bindings()[0].region.as_ref().unwrap().bounds(),
            Rect::new(8, 4, 1, 2)
        );
        transform_canvas(&mut d, CanvasTransform::Rotate90(1)).unwrap();
        let bone = d.rig(skel).unwrap().bone(b).unwrap().clone();
        // (8.5, 4.5) turns to (16 - 4.5, 8.5); the bone now points left.
        assert!(close(bone.head, Vec2::new(11.5, 8.5)));
        assert_eq!(bone.angle, 180.0);
    }

    #[test]
    fn indexed_documents_render_palette_indices() {
        let (mut d, art, skel) = arm_doc();
        d.set_color_mode(ColorMode::Indexed);
        let rig = d.rig_mut(skel).unwrap();
        let b = rig
            .add_bone("b", None, Vec2::new(7.5, 4.0), Vec2::new(7.5, 8.0))
            .unwrap();
        rig.bind(Binding {
            layer: art,
            region: None,
            bone: b,
            pivot: Vec2::new(7.5, 4.0),
        })
        .unwrap();
        let mut pose = Pose::default();
        pose.set(
            b,
            BonePose {
                rotation: 37.0,
                offset: Vec2::ZERO,
            },
        );
        d.render_pose_to_new_frame(skel, &pose, 1, RenderOptions::default())
            .unwrap();
        let r = d.cel_raster(art, 1).unwrap().unwrap();
        let black = d.palette().nearest(Rgba::BLACK, Some(0)) as u32;
        let values: BTreeSet<u32> = r.bounds().points().map(|p| r.get_raw(p.x, p.y)).collect();
        assert_eq!(values, BTreeSet::from([0, black]), "no new colors");
    }
}
