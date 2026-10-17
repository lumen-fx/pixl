//! The layers panel's list and the layer commands behind it: groups that
//! nest, collapse and take layers dragged into them, layer masks and the
//! mask editing mode, clipping, reference images, locks, color tags,
//! renaming, duplicating, merging down and flattening, and the layer
//! properties dialog. The Layer menu and a right-click on a layer reach the
//! same commands.

use lumenui::ecs::hierarchy::ChildOf;
use lumenui::ecs::system::SystemParam;
use lumenui::ecs::world::World;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{
    ContextMenuEvent, DoubleClickEvent, MenuClicked, PointerButton, PointerMoved, PointerPressed,
    PointerReleased,
};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::{
    BlendMode, CompositeOptions, Error, ImageId, LayerId, LayerKind, LayerProps, Locks, NewLayer,
    Placement, Rect, Rgba, Scope, Target, TiledImage, ToolContext, composite_frame, edit_cel,
    edit_mask,
};

use pixl_core::history::Transaction;

use crate::editor::Editor;
use crate::home::Home;
use crate::panels::{Panels, blend_from_name, blend_name, close_edit, escape};
use crate::timeline::{LayerTree, rows};
use crate::tools::hex;
use crate::view::parse_hex;

// ---- what tools paint on ----

impl Editor {
    /// What tools paint on: the active layer's cel in the current frame, or
    /// its mask while the mask is being edited.
    pub fn target(&mut self) -> std::result::Result<Target<'_>, Error> {
        if self.mask_edit {
            self.doc.mask_target(self.layer)
        } else {
            self.doc.target(self.layer, self.frame)
        }
    }

    /// The image [`Editor::target`] writes to.
    pub fn target_image(&mut self) -> std::result::Result<ImageId, Error> {
        if self.mask_edit {
            self.doc.mask_image(self.layer)
        } else {
            self.doc.target_image(self.layer, self.frame)
        }
    }

    /// Starts the undo step of an edit to what tools paint on, and returns
    /// the image it writes to. A layer whose pixels are locked is refused
    /// before anything changes, and the step begins before an empty frame's
    /// cel is made, so undo takes the new cel away again.
    pub fn begin_paint(
        &mut self,
        label: &str,
    ) -> std::result::Result<(ImageId, Transaction), Error> {
        if !self.mask_edit && self.doc.effective_locks(self.layer)?.pixels {
            return Err(Error::Locked(self.layer));
        }
        let mut tx = self
            .history
            .begin(&self.doc, label, Scope::Images(Vec::new()));
        let image = self.target_image()?;
        tx.watch(&self.doc, image);
        Ok((image, tx))
    }

    /// Runs a tool on [`Editor::target`] as one undo step.
    pub fn edit<R>(
        &mut self,
        label: &str,
        ctx: &ToolContext,
        tool: impl FnOnce(&mut pixl_core::Canvas) -> R,
    ) -> std::result::Result<(R, Rect), Error> {
        if self.mask_edit {
            edit_mask(
                &mut self.doc,
                &mut self.history,
                label,
                self.layer,
                ctx,
                tool,
            )
        } else {
            edit_cel(
                &mut self.doc,
                &mut self.history,
                label,
                self.layer,
                self.frame,
                ctx,
                tool,
            )
        }
    }
}

// ---- layer commands ----

impl Editor {
    /// Runs a layer edit as one undo step; a failure becomes the status
    /// line's message.
    fn layer_edit<R>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut pixl_core::Document) -> std::result::Result<R, Error>,
    ) -> Option<R> {
        match self
            .history
            .transact(&mut self.doc, label, Scope::All, edit)
        {
            Ok(r) => {
                self.touch();
                Some(r)
            }
            Err(e) => {
                self.message = Some(e.to_string());
                None
            }
        }
    }

    /// Adds an empty group above the active layer and makes it active.
    pub fn add_group(&mut self) {
        let at = self.placement_above(self.layer);
        let n = self
            .doc
            .walk()
            .iter()
            .filter(|(_, l)| matches!(l.kind, LayerKind::Group { .. }))
            .count();
        let name = format!("Group {}", n + 1);
        if let Some(id) =
            self.layer_edit("New group", |doc| doc.add_layer(NewLayer::Group, name, at))
        {
            self.layer = id;
            self.mask_edit = false;
        }
    }

    /// Copies the active layer above itself and makes the copy active.
    pub fn duplicate_active(&mut self) {
        let id = self.layer;
        if let Some(copy) = self.layer_edit("Duplicate layer", |doc| doc.duplicate_layer(id)) {
            self.layer = copy;
            self.mask_edit = false;
        }
    }

    /// Merges the active layer onto the raster layer below it.
    pub fn merge_down_active(&mut self) {
        let id = self.layer;
        if let Some(below) = self.layer_edit("Merge down", |doc| doc.merge_down(id)) {
            self.layer = below;
            self.mask_edit = false;
        }
    }

    /// Flattens every visible layer into one.
    pub fn flatten(&mut self) {
        if let Some(id) = self.layer_edit("Flatten", |doc| doc.flatten()) {
            self.layer = id;
            self.mask_edit = false;
        }
    }

    /// Gives the active layer a mask that shows all of it, and starts
    /// editing the mask.
    pub fn add_mask(&mut self) {
        let id = self.layer;
        if self
            .layer_edit("Add mask", |doc| doc.add_layer_mask(id))
            .is_some()
        {
            self.mask_edit = true;
        }
    }

    pub fn delete_mask(&mut self) {
        let id = self.layer;
        self.layer_edit("Delete mask", |doc| doc.remove_layer_mask(id));
        self.mask_edit = false;
    }

    /// Bakes the active layer's mask into its pixels.
    pub fn apply_mask(&mut self) {
        let id = self.layer;
        self.layer_edit("Apply mask", |doc| doc.apply_layer_mask(id));
        self.mask_edit = false;
    }

    /// Turns the active layer's mask on or off.
    pub fn toggle_mask(&mut self) {
        let id = self.layer;
        let Some(enabled) = mask_of(&self.doc, id).map(|m| m.enabled) else {
            self.message = Some("The layer has no mask".into());
            return;
        };
        let label = if enabled {
            "Disable mask"
        } else {
            "Enable mask"
        };
        self.layer_edit(label, |doc| doc.set_mask_enabled(id, !enabled));
    }

    /// Starts or stops painting on the active layer's mask.
    pub fn toggle_mask_edit(&mut self) {
        if mask_of(&self.doc, self.layer).is_some() {
            self.mask_edit = !self.mask_edit;
        } else {
            self.mask_edit = false;
            self.message = Some("The layer has no mask".into());
        }
    }

    /// Clips the active layer to the layer below it, or stops clipping.
    pub fn toggle_clip(&mut self) {
        let id = self.layer;
        self.update_layer("Clipping", id, |p| p.clip = !p.clip);
    }

    /// Sets the active layer's locks.
    pub fn set_locks(&mut self, id: LayerId, locks: Locks) {
        self.update_layer("Lock", id, |p| p.locks = locks);
    }

    /// Adds an image as a reference layer above the active layer, half
    /// transparent, for tracing over. It is never exported.
    pub fn add_reference(&mut self, image: TiledImage<Rgba>, name: String) {
        let at = self.placement_above(self.layer);
        if let Some(id) = self.layer_edit("Reference image", |doc| {
            let id = doc.add_layer(NewLayer::Reference(image), name, at)?;
            doc.update_layer(id, |p| p.opacity = 128)?;
            Ok(id)
        }) {
            self.layer = id;
            self.mask_edit = false;
        }
    }

    /// Moves a layer to `to`, as a drag in the layers panel drops it.
    pub fn place_layer(&mut self, id: LayerId, to: Placement) {
        if self
            .layer_edit("Move layer", |doc| doc.move_layer(id, to))
            .is_some()
        {
            self.layer = id;
        }
    }
}

fn mask_of(doc: &pixl_core::Document, id: LayerId) -> Option<pixl_core::LayerMask> {
    match &doc.layer(id)?.kind {
        LayerKind::Raster { mask, .. } => *mask,
        _ => None,
    }
}

/// An image read from a file, fitted to `size`: its first frame placed at
/// the top left, cut where it is larger and transparent where it is
/// smaller.
pub fn reference_image(
    path: &std::path::Path,
    size: (u32, u32),
) -> std::result::Result<TiledImage<Rgba>, String> {
    let opened = crate::files::read_document(path)?;
    let src = &opened.doc;
    let px = composite_frame(src, 0, &CompositeOptions::EXPORT);
    let mut img = TiledImage::new(size.0, size.1, Rgba::TRANSPARENT);
    let (w, h) = (src.width().min(size.0), src.height().min(size.1));
    for y in 0..h {
        for x in 0..w {
            let c = px[(y * src.width() + x) as usize];
            if c.a != 0 {
                img.set(x as i32, y as i32, c);
            }
        }
    }
    img.compact();
    Ok(img)
}

// ---- the list ----

/// The list's drop marker while a layer is dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    Above(LayerId),
    Below(LayerId),
    Into(LayerId),
}

/// A layer being dragged in the list.
#[derive(Clone, Copy, Debug)]
struct Dragging {
    layer: LayerId,
    from: Vec2,
    moved: bool,
    drop: Option<Drop>,
}

/// The layers panel's state.
#[derive(Resource, Default)]
pub struct LayerPanel {
    /// The list's markup as last written, and what it was written from:
    /// the document's generation and revision, the active layer, mask
    /// editing and the folded groups.
    shown: Option<String>,
    shown_for: Option<u64>,
    drag: Option<Dragging>,
    /// The layer a context menu or the properties dialog is about.
    about: Option<LayerId>,
    /// The color tag the properties dialog has picked: `Some(None)` for no
    /// tag.
    picked_tag: Option<Option<Rgba>>,
    /// The drop marker as last drawn.
    marker: Option<Drop>,
    /// Ticks left to write the properties dialog's fields again: they
    /// appear a tick after it opens, and the name field, which has focus,
    /// takes only a direct write (lumen-fx/lumen#536).
    refill: u8,
}

/// The layers list, top layer first, as markup.
pub fn layers_markup(editor: &Editor, tree: &LayerTree) -> String {
    let mut out = String::new();
    for (depth, layer) in rows(&editor.doc, tree) {
        let id = layer.id.0;
        let p = &layer.props;
        let mut class = format!("layer-row depth-{}", depth.min(4));
        if layer.id == editor.layer {
            class.push_str(" active");
        }
        if !p.visible {
            class.push_str(" hidden");
        }
        if p.clip {
            class.push_str(" clipped");
        }
        let kind = match &layer.kind {
            LayerKind::Raster { .. } => "",
            LayerKind::Group { .. } => " group",
            LayerKind::Reference { .. } => " reference",
            LayerKind::Skeleton { .. } => " skeleton",
        };
        class.push_str(kind);
        let eye = if p.visible { "on" } else { "off" };
        let lock = if p.locks == Locks::ALL {
            "all"
        } else if p.locks == Locks::NONE {
            "none"
        } else {
            "some"
        };
        // The color tag is the layer's data, so it rides in the markup.
        let tag = match p.color_tag {
            Some(c) => format!(r#"<tile class="layer-tag" bg="{}"/>"#, hex(c)),
            None => r#"<tile class="layer-tag untagged"/>"#.to_string(),
        };
        let fold = match &layer.kind {
            LayerKind::Group { .. } => {
                let (cls, text) = if tree.collapsed.contains(&layer.id) {
                    ("folded", "+")
                } else {
                    ("open", "-")
                };
                format!(r#"<button id="layer-fold-{id}" class="layer-fold {cls}" text="{text}"/>"#)
            }
            _ => String::new(),
        };
        let clip = if p.clip {
            r#"<tile class="layer-clip"/>"#
        } else {
            ""
        };
        let mask = match &layer.kind {
            LayerKind::Raster { mask: Some(m), .. } => {
                let mut c = String::from("layer-mask");
                c.push_str(if m.enabled { " on" } else { " off" });
                if editor.mask_edit && layer.id == editor.layer {
                    c.push_str(" editing");
                }
                format!(r#"<button id="layer-mask-{id}" class="{c}" text="mask"/>"#)
            }
            _ => String::new(),
        };
        let note = match &layer.kind {
            LayerKind::Reference { .. } => r#"<label class="layer-kind" text="ref"/>"#,
            _ => "",
        };
        let meta = if p.blend == BlendMode::Normal && p.opacity == 255 {
            String::new()
        } else {
            format!(
                "{} {}%",
                blend_name(p.blend),
                (u32::from(p.opacity) * 100 + 127) / 255
            )
        };
        out.push_str(&format!(
            r#"<row id="layer-row-{id}" class="{class}">{tag}<button id="layer-eye-{id}" class="layer-eye {eye}" text=""/><button id="layer-lock-{id}" class="layer-lock {lock}" text=""/>{fold}{clip}<label class="layer-name" text="{name}"/>{note}{mask}<label class="layer-meta" text="{meta}"/></row>"#,
            name = escape(&p.name),
            meta = escape(&meta),
        ));
    }
    out
}

/// Keeps the list showing the document, and the mask editing mode on a
/// layer that has a mask.
pub fn show_layers(
    mut editor: ResMut<Editor>,
    mut panel: ResMut<LayerPanel>,
    tree: Res<LayerTree>,
    wake: Option<Res<WakeDeadline>>,
) {
    if editor.mask_edit && mask_of(&editor.doc, editor.layer).is_none() {
        editor.mask_edit = false;
    }
    let mut changed = false;
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            editor.generation,
            editor.revision,
            editor.layer.0,
            editor.mask_edit,
        )
            .hash(&mut h);
        tree.collapsed.hash(&mut h);
        h.finish()
    };
    let markup = (panel.shown_for != Some(key)).then(|| layers_markup(&editor, &tree));
    if let Some(markup) = markup
        && let Some(node) = dom::get_by_id("layer-list")
    {
        panel.shown_for = Some(key);
        if panel.shown.as_deref() != Some(markup.as_str()) {
            node.set_inner_markup(&markup);
            panel.shown = Some(markup);
            panel.marker = None;
            changed = true;
        }
    }
    let want = panel.drag.and_then(|d| d.drop.filter(|_| d.moved));
    if panel.marker != want {
        let mark = |drop: Drop, on: bool| {
            let (id, class) = match drop {
                Drop::Above(id) => (id, "drop-above"),
                Drop::Below(id) => (id, "drop-below"),
                Drop::Into(id) => (id, "drop-into"),
            };
            if let Some(n) = dom::get_by_id(&format!("layer-row-{}", id.0)) {
                if on {
                    n.add_class(class);
                } else {
                    n.remove_class(class);
                }
            }
        };
        if let Some(old) = panel.marker {
            mark(old, false);
        }
        if let Some(new) = want {
            mark(new, true);
        }
        panel.marker = want;
        changed = true;
    }
    if changed {
        crate::repaint_soon(&wake);
    }
}

// ---- input ----

/// What reaches the layers panel this tick.
#[derive(SystemParam)]
pub struct LayerInput<'w, 's> {
    clicks: MessageReader<'w, 's, ClickEvent>,
    doubles: MessageReader<'w, 's, DoubleClickEvent>,
    menus: MessageReader<'w, 's, ContextMenuEvent>,
    chosen: MessageReader<'w, 's, MenuClicked>,
    pressed: MessageReader<'w, 's, PointerPressed>,
    moved: MessageReader<'w, 's, PointerMoved>,
    released: MessageReader<'w, 's, PointerReleased>,
    ids: Query<'w, 's, &'static LumenId>,
    parents: Query<'w, 's, &'static ChildOf>,
    hovered: Query<'w, 's, Entity, With<Hovered>>,
}

impl LayerInput<'_, '_> {
    /// The id of `entity` or its nearest ancestor with one.
    fn id_of(&self, mut entity: Entity) -> Option<String> {
        loop {
            if let Ok(id) = self.ids.get(entity) {
                return Some(id.0.clone());
            }
            entity = self.parents.get(entity).ok()?.parent();
        }
    }

    /// The layer row the hovered element is in: `Some(None)` when the
    /// hovered element is elsewhere, `None` when nothing is hovered.
    fn hovered_row(&self) -> Option<Option<LayerId>> {
        let e = self.hovered.iter().next()?;
        let mut e = e;
        loop {
            if let Ok(id) = self.ids.get(e)
                && let Some(row) = row_id(&id.0)
            {
                return Some(Some(row));
            }
            match self.parents.get(e) {
                Ok(p) => e = p.parent(),
                Err(_) => return Some(None),
            }
        }
    }
}

/// The layer a `layer-row-N` id names.
fn row_id(id: &str) -> Option<LayerId> {
    id.strip_prefix("layer-row-")?.parse().ok().map(LayerId)
}

/// The row under a window position and where in it: the top third drops
/// above, the bottom third below, and the middle of a group into it.
fn drop_at(at: Vec2, order: &[(LayerId, bool)]) -> Option<Drop> {
    for (id, group) in order {
        let Some(r) = dom::get_by_id(&format!("layer-row-{}", id.0)).and_then(|n| n.rect()) else {
            continue;
        };
        if at.y < r.client_y || at.y >= r.client_y + r.height {
            continue;
        }
        let t = (at.y - r.client_y) / r.height.max(1.0);
        return Some(if *group && (1.0 / 3.0..2.0 / 3.0).contains(&t) {
            Drop::Into(*id)
        } else if t < 0.5 {
            Drop::Above(*id)
        } else {
            Drop::Below(*id)
        });
    }
    None
}

/// Where `layer` goes for a drop: beside the target in the target's group,
/// or at the top of a group. `None` when the drop would put a group inside
/// itself or change nothing.
pub fn placement_for(doc: &pixl_core::Document, layer: LayerId, drop: Drop) -> Option<Placement> {
    let target = match drop {
        Drop::Above(t) | Drop::Below(t) | Drop::Into(t) => t,
    };
    if target == layer || doc.ancestors(target).contains(&layer) {
        return None;
    }
    if let Drop::Into(group) = drop {
        return Some(Placement {
            parent: Some(group),
            index: None,
        });
    }
    let parent = doc.parent_of(target);
    let siblings = match parent {
        None => doc.layers(),
        Some(p) => doc.layer(p)?.children(),
    };
    let at = siblings.iter().position(|l| l.id == target)?;
    let mut index = if matches!(drop, Drop::Above(_)) {
        at + 1
    } else {
        at
    };
    // Taking the layer out first moves the later siblings down one.
    if doc.parent_of(layer) == parent
        && let Some(from) = siblings.iter().position(|l| l.id == layer)
    {
        if from < index {
            index -= 1;
        }
        if from == index {
            return None;
        }
    }
    Some(Placement {
        parent,
        index: Some(index),
    })
}

/// The tag colors the properties dialog offers, read from the theme.
fn tag_choice(i: u8) -> Option<Rgba> {
    dom::get_by_id(&format!("lp-tag-{i}"))
        .and_then(|n| n.computed_style_of("bg"))
        .as_deref()
        .and_then(parse_hex)
}

fn mark_tag_choice(picked: Option<Rgba>) {
    for i in 0..=8u8 {
        let Some(node) = dom::get_by_id(&format!("lp-tag-{i}")) else {
            continue;
        };
        let this = if i == 0 { None } else { tag_choice(i) };
        crate::set_class(node, "chosen", this == picked);
    }
}

/// Fills the properties dialog with a layer's properties and opens it.
fn open_properties(editor: &Editor, panel: &mut LayerPanel, id: LayerId, signals: &mut Signals) {
    if editor.doc.layer(id).is_none() {
        return;
    }
    panel.about = Some(id);
    panel.picked_tag = editor.doc.layer(id).map(|l| l.props.color_tag);
    panel.refill = 2;
    fill_properties(editor, id, signals);
    signals.set("lp_open", true);
}

/// Writes a layer's properties into the dialog's fields.
fn fill_properties(editor: &Editor, id: LayerId, signals: &mut Signals) {
    let Some(layer) = editor.doc.layer(id) else {
        return;
    };
    let p = &layer.props;
    signals.set("lp_name", p.name.clone());
    if let Some(field) = dom::get_by_id("lp-name") {
        field.set_text(&p.name);
    }
    signals.set(
        "lp_opacity",
        ((u32::from(p.opacity) * 100 + 127) / 255).to_string(),
    );
    signals.set("lp_blend", blend_name(p.blend));
    signals.set("lp_clip", p.clip);
    signals.set("lp_lock_pixels", p.locks.pixels);
    signals.set("lp_lock_alpha", p.locks.alpha);
    signals.set("lp_lock_position", p.locks.position);
    let mask = mask_of(&editor.doc, id);
    signals.set("lp_has_mask", mask.is_some());
    signals.set("lp_mask_on", mask.is_some_and(|m| m.enabled));
    mark_tag_choice(p.color_tag);
}

/// The properties dialog's fields, applied as one undo step.
fn apply_properties(editor: &mut Editor, panel: &LayerPanel, signals: &Signals) {
    let Some(id) = panel.about else {
        return;
    };
    let Some(before) = editor.doc.layer(id).map(|l| l.props.clone()) else {
        return;
    };
    let text = |n: &str| signals.get::<String>(n).unwrap_or_default();
    let flag = |n: &str| signals.get::<bool>(n).unwrap_or(false);
    let name = text("lp_name").trim().to_string();
    let opacity = text("lp_opacity")
        .trim()
        .parse::<u32>()
        .map_or(before.opacity, |v| ((v.min(100) * 255 + 50) / 100) as u8);
    let props = LayerProps {
        name: if name.is_empty() {
            before.name.clone()
        } else {
            name
        },
        opacity,
        blend: blend_from_name(&text("lp_blend")).unwrap_or(before.blend),
        clip: flag("lp_clip"),
        locks: Locks {
            pixels: flag("lp_lock_pixels"),
            alpha: flag("lp_lock_alpha"),
            position: flag("lp_lock_position"),
        },
        color_tag: panel.picked_tag.unwrap_or(before.color_tag),
        ..before.clone()
    };
    let mask_on = mask_of(&editor.doc, id).map(|m| m.enabled);
    let want_mask = flag("lp_mask_on");
    if props == before && mask_on.is_none_or(|on| on == want_mask) {
        return;
    }
    editor.layer_edit("Layer properties", |doc| {
        doc.update_layer(id, |p| *p = props)?;
        if mask_on.is_some_and(|on| on != want_mask) {
            doc.set_mask_enabled(id, want_mask)?;
        }
        Ok(())
    });
}

/// Opens the file dialog for a reference image.
fn import_reference(commands: &mut Commands) {
    commands.queue(
        FileDialog::open()
            .filter("Images", &["png", "gif", "aseprite", "ase", "pixl"])
            .then(|world: &mut World, paths| {
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                let mut editor = world.resource_mut::<Editor>();
                let size = (editor.doc.width(), editor.doc.height());
                match reference_image(&path, size) {
                    Ok(img) => {
                        let name = path.file_stem().map_or_else(
                            || "Reference".into(),
                            |s| s.to_string_lossy().into_owned(),
                        );
                        editor.add_reference(img, name);
                    }
                    Err(e) => editor.message = Some(e),
                }
            }),
    );
}

/// Carries out a layer command from a button, the Layer menu or a context
/// menu, on the active layer.
pub fn layer_command(
    id: &str,
    editor: &mut Editor,
    panel: &mut LayerPanel,
    tree: &mut LayerTree,
    commands: &mut Commands,
    signals: &mut Signals,
) -> bool {
    match id {
        "layer-group" | "lm-group" => editor.add_group(),
        "layer-duplicate" | "lm-duplicate" => editor.duplicate_active(),
        "lm-new" => editor.add_layer(),
        "lm-delete" => editor.delete_layer(),
        "lm-merge" => editor.merge_down_active(),
        "lm-flatten" => editor.flatten(),
        "lm-reference" => import_reference(commands),
        "lm-mask-add" => editor.add_mask(),
        "lm-mask-edit" => editor.toggle_mask_edit(),
        "lm-mask-toggle" => editor.toggle_mask(),
        "lm-mask-apply" => editor.apply_mask(),
        "lm-mask-delete" => editor.delete_mask(),
        "lm-clip" => editor.toggle_clip(),
        "lm-lock-pixels" | "lm-lock-alpha" | "lm-lock-position" | "lm-lock-all" => {
            let layer = editor.layer;
            if let Some(mut locks) = editor.doc.layer(layer).map(|l| l.props.locks) {
                match id {
                    "lm-lock-pixels" => locks.pixels = !locks.pixels,
                    "lm-lock-alpha" => locks.alpha = !locks.alpha,
                    "lm-lock-position" => locks.position = !locks.position,
                    _ => {
                        locks = if locks == Locks::ALL {
                            Locks::NONE
                        } else {
                            Locks::ALL
                        }
                    }
                }
                editor.set_locks(layer, locks);
            }
        }
        "lm-collapse" => {
            let layer = editor.layer;
            if !tree.collapsed.remove(&layer) {
                tree.collapsed.insert(layer);
            }
        }
        "lm-properties" | "layer-properties" => {
            open_properties(editor, panel, editor.layer, signals)
        }
        "lp-ok" => {
            signals.set("lp_open", false);
            apply_properties(editor, panel, signals);
        }
        "lp-cancel" => signals.set("lp_open", false),
        "menu-layer" => {
            panel.about = None;
            let at = dom::get_by_id("menu-layer").and_then(|n| n.rect());
            let (x, y) = at.map_or((0.0, 0.0), |r| (r.client_x, r.client_y + r.height));
            signals.open_menu_at("layer-menu", x, y);
        }
        other => {
            let n = |prefix: &str| {
                other
                    .strip_prefix(prefix)
                    .and_then(|n| n.parse::<u32>().ok())
                    .map(LayerId)
            };
            if let Some(i) = other
                .strip_prefix("lp-tag-")
                .and_then(|i| i.parse::<u8>().ok())
            {
                let c = if i == 0 { None } else { tag_choice(i) };
                panel.picked_tag = Some(c);
                mark_tag_choice(c);
            } else if let Some(layer) = n("layer-fold-") {
                if !tree.collapsed.remove(&layer) {
                    tree.collapsed.insert(layer);
                }
            } else if let Some(layer) = n("layer-lock-") {
                if let Some(locks) = editor.doc.layer(layer).map(|l| l.props.locks) {
                    let next = if locks == Locks::NONE {
                        Locks::ALL
                    } else {
                        Locks::NONE
                    };
                    editor.set_locks(layer, next);
                }
            } else if let Some(layer) = n("layer-mask-") {
                if editor.layer == layer {
                    editor.toggle_mask_edit();
                } else if editor.doc.layer(layer).is_some() {
                    editor.layer = layer;
                    editor.mask_edit = true;
                }
            } else {
                return false;
            }
        }
    }
    true
}

/// Clicks, drags, menus and the properties dialog of the layers panel.
#[allow(clippy::too_many_arguments)]
pub fn layer_input(
    mut editor: ResMut<Editor>,
    mut panel: ResMut<LayerPanel>,
    mut panels: ResMut<Panels>,
    mut tree: ResMut<LayerTree>,
    home: Res<Home>,
    mut input: LayerInput,
    mut commands: Commands,
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
    if panel.refill > 0 {
        panel.refill -= 1;
        if let Some(id) = panel.about {
            fill_properties(editor, id, &mut signals);
            mark_tag_choice(panel.picked_tag.flatten());
        }
    }

    // A row click picks the layer and leaves the mask; the buttons in the
    // row and the Layer menu run commands.
    let mut chosen: Vec<String> = input.chosen.read().map(|m| m.id.clone()).collect();
    let clicked: Vec<Entity> = input.clicks.read().map(|c| c.entity).collect();
    for e in clicked {
        let Some(id) = input.id_of(e) else { continue };
        if let Some(layer) = row_id(&id) {
            if editor.doc.layer(layer).is_some() && editor.layer != layer {
                editor.mask_edit = false;
            }
            continue;
        }
        chosen.push(id);
    }
    for id in chosen {
        if id.starts_with("lm-") || id.starts_with("layer-") || id.starts_with("lp-") {
            close_edit(editor, &mut panels);
        }
        let about = panel.about.filter(|_| id.starts_with("lm-"));
        if let Some(layer) = about
            && editor.doc.layer(layer).is_some()
        {
            // A context menu acts on the layer it was opened on.
            editor.layer = layer;
        }
        layer_command(
            &id,
            editor,
            &mut panel,
            &mut tree,
            &mut commands,
            &mut signals,
        );
        if id.starts_with("lm-") {
            panel.about = None;
        }
    }

    // A double click on a row opens its properties.
    let doubles: Vec<Entity> = input.doubles.read().map(|d| d.entity).collect();
    for e in doubles {
        if let Some(layer) = input.id_of(e).as_deref().and_then(row_id)
            && editor.doc.layer(layer).is_some()
        {
            editor.layer = layer;
            open_properties(editor, &mut panel, layer, &mut signals);
        }
    }

    // A right click on a row opens the Layer menu for it.
    let menus: Vec<ContextMenuEvent> = input.menus.read().cloned().collect();
    for m in menus {
        if let Some(layer) = input.id_of(m.entity).as_deref().and_then(row_id)
            && editor.doc.layer(layer).is_some()
        {
            if editor.layer != layer {
                editor.mask_edit = false;
            }
            editor.layer = layer;
            panel.about = Some(layer);
            signals.open_menu_at("layer-menu", m.position.x, m.position.y);
        }
    }

    // Dragging a row moves the layer: beside another row, or into a group.
    let presses: Vec<PointerPressed> = input.pressed.read().cloned().collect();
    for p in presses {
        if p.button != PointerButton::Primary {
            continue;
        }
        // The row under the pointer: the hovered one, or with nothing
        // hovered, the one laid out there.
        let row = match input.hovered_row() {
            Some(row) => row,
            None => {
                let order: Vec<(LayerId, bool)> = rows(&editor.doc, &tree)
                    .iter()
                    .map(|(_, l)| (l.id, false))
                    .collect();
                match drop_at(p.position, &order) {
                    Some(Drop::Above(id) | Drop::Below(id) | Drop::Into(id)) => Some(id),
                    None => None,
                }
            }
        };
        if let Some(layer) = row {
            panel.drag = Some(Dragging {
                layer,
                from: p.position,
                moved: false,
                drop: None,
            });
        }
    }
    let last = input.moved.read().last().map(|m| m.position);
    if let (Some(at), Some(drag)) = (last, panel.drag.as_mut()) {
        if (at - drag.from).length() > 4.0 {
            drag.moved = true;
        }
        if drag.moved {
            let order: Vec<(LayerId, bool)> = rows(&editor.doc, &tree)
                .iter()
                .map(|(_, l)| (l.id, matches!(l.kind, LayerKind::Group { .. })))
                .collect();
            drag.drop = drop_at(at, &order);
        }
    }
    if input.released.read().count() > 0
        && let Some(drag) = panel.drag.take()
        && drag.moved
        && let Some(drop) = drag.drop
        && let Some(to) = placement_for(&editor.doc, drag.layer, drop)
    {
        editor.place_layer(drag.layer, to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_core::{ColorMode, Document};

    fn doc() -> (Document, Vec<LayerId>, LayerId) {
        let mut d = Document::new(8, 8, ColorMode::Rgba).unwrap();
        let a = d.layers()[0].id;
        let b = d
            .add_layer(NewLayer::Raster, "b", Placement::default())
            .unwrap();
        let g = d
            .add_layer(NewLayer::Group, "g", Placement::default())
            .unwrap();
        (d, vec![a, b], g)
    }

    #[test]
    fn drops_place_layers_beside_rows_and_into_groups() {
        let (d, ids, g) = doc();
        let (a, b) = (ids[0], ids[1]);
        // a is at the bottom; above b means index 2 before a leaves, 1 after.
        assert_eq!(
            placement_for(&d, a, Drop::Above(b)),
            Some(Placement {
                parent: None,
                index: Some(1)
            })
        );
        assert_eq!(placement_for(&d, a, Drop::Below(b)), None, "already there");
        assert_eq!(
            placement_for(&d, a, Drop::Into(g)),
            Some(Placement {
                parent: Some(g),
                index: None
            })
        );
        assert_eq!(placement_for(&d, g, Drop::Into(g)), None);
    }

    #[test]
    fn mask_editing_paints_the_mask() {
        let (d, ids, _) = doc();
        let mut e = Editor::new(d);
        e.layer = ids[1];
        e.add_mask();
        assert!(e.mask_edit);
        let image = e.target_image().unwrap();
        assert_eq!(Some(image), mask_of(&e.doc, ids[1]).map(|m| m.image));
        e.toggle_mask_edit();
        assert_ne!(
            Some(e.target_image().unwrap()),
            mask_of(&e.doc, ids[1]).map(|m| m.image)
        );
    }
}
