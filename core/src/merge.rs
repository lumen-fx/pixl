//! Layer edits that bake pixels together or copy them: duplicate a layer,
//! merge a layer down onto the one below it, flatten the document, and apply
//! a layer mask to the layer's pixels.

use std::collections::BTreeMap;

use crate::color::{ColorMode, Rgba};
use crate::composite::{CompositeOptions, composite};
use crate::document::{Cel, Document, ImageId, Layer, LayerId, LayerKind, LayerProps};
use crate::error::Error;
use crate::geom::Rect;
use crate::image::{Raster, TileSnap, TiledImage};

/// The area of a raster's present tiles: where it may hold pixels.
fn tile_bounds(r: &Raster) -> Rect {
    let mut out = Rect::EMPTY;
    for t in 0..r.tile_count() {
        let present = match r.tile_snap(t) {
            TileSnap::Rgba(p) => p.is_some(),
            TileSnap::Byte(p) => p.is_some(),
        };
        if present {
            out = out.union(&r.tile_rect(t));
        }
    }
    out
}

/// Root layers flattening leaves in place: they paint no exported pixels.
fn kept_by_flatten(l: &Layer) -> bool {
    matches!(
        l.kind,
        LayerKind::Reference { .. } | LayerKind::Skeleton { .. }
    )
}

/// Where a layer (and everything in a group) may hold pixels at `frame`.
fn layer_bounds(doc: &Document, layer: &Layer, frame: usize) -> Rect {
    match &layer.kind {
        LayerKind::Raster { cels, .. } => cels
            .get(frame)
            .copied()
            .flatten()
            .and_then(|c| doc.image(c.image))
            .map_or(Rect::EMPTY, tile_bounds),
        LayerKind::Group { children } => children
            .iter()
            .fold(Rect::EMPTY, |r, c| r.union(&layer_bounds(doc, c, frame))),
        LayerKind::Reference { image } => doc.image(*image).map_or(Rect::EMPTY, tile_bounds),
        LayerKind::Skeleton { .. } => Rect::EMPTY,
    }
}

/// Pixels composited over `rect` turned into a raster of the document's
/// color mode, empty outside `rect`.
fn to_raster(doc: &Document, rect: Rect, px: &[Rgba]) -> Raster {
    let (w, h) = (doc.width(), doc.height());
    let ti = doc.transparent_index();
    match doc.mode() {
        ColorMode::Rgba => {
            let mut img = TiledImage::new(w, h, Rgba::TRANSPARENT);
            for (i, p) in rect.points().enumerate() {
                if px[i].a != 0 {
                    img.set(p.x, p.y, px[i]);
                }
            }
            img.compact();
            Raster::Rgba(img)
        }
        ColorMode::Indexed => {
            let palette = doc.palette();
            let mut img = TiledImage::new(w, h, ti);
            for (i, p) in rect.points().enumerate() {
                if px[i].a != 0 {
                    img.set(p.x, p.y, palette.nearest(px[i], Some(ti)));
                }
            }
            img.compact();
            Raster::Indexed(img)
        }
    }
}

/// A copy of `layer` with fresh ids and its own copies of every image.
/// Cels linked inside the layer stay linked to each other in the copy.
fn deep_copy(doc: &mut Document, layer: &Layer) -> Layer {
    let id = LayerId(doc.s.next_layer);
    doc.s.next_layer += 1;
    let mut copies: BTreeMap<ImageId, ImageId> = BTreeMap::new();
    let mut copy = |doc: &mut Document, image: ImageId| -> ImageId {
        if let Some(c) = copies.get(&image) {
            return *c;
        }
        let raster = doc.images[&image].clone();
        let fresh = doc.alloc_image(raster);
        copies.insert(image, fresh);
        fresh
    };
    let kind = match &layer.kind {
        LayerKind::Raster { cels, mask } => {
            let cels = cels
                .iter()
                .map(|c| {
                    c.map(|c| Cel {
                        image: copy(doc, c.image),
                        opacity: c.opacity,
                    })
                })
                .collect();
            let mask = mask.map(|m| crate::document::LayerMask {
                image: copy(doc, m.image),
                enabled: m.enabled,
            });
            LayerKind::Raster { cels, mask }
        }
        LayerKind::Group { children } => LayerKind::Group {
            children: children.iter().map(|c| deep_copy(doc, c)).collect(),
        },
        LayerKind::Reference { image } => LayerKind::Reference {
            image: copy(doc, *image),
        },
        LayerKind::Skeleton { rig } => LayerKind::Skeleton { rig: rig.clone() },
    };
    Layer {
        id,
        props: layer.props.clone(),
        kind,
    }
}

impl Document {
    /// The siblings of `id`: the root layers or its group's children.
    fn siblings_of(&self, id: LayerId) -> Result<&[Layer], Error> {
        self.layer(id).ok_or(Error::LayerNotFound(id))?;
        Ok(match self.parent_of(id) {
            None => self.layers(),
            Some(p) => self.layer(p).map_or(&[][..], |l| l.children()),
        })
    }

    /// Copies a layer (a group with everything in it) just above itself and
    /// returns the copy's id. The copy owns its pixels.
    pub fn duplicate_layer(&mut self, id: LayerId) -> Result<LayerId, Error> {
        let original = self.layer(id).ok_or(Error::LayerNotFound(id))?.clone();
        let parent = self.parent_of(id);
        let mut copy = deep_copy(self, &original);
        copy.props.name = format!("{} copy", original.props.name);
        let new_id = copy.id;
        let siblings = self.siblings_mut(parent)?;
        let pos = siblings
            .iter()
            .position(|l| l.id == id)
            .ok_or(Error::LayerNotFound(id))?;
        siblings.insert(pos + 1, copy);
        Ok(new_id)
    }

    /// The raster layer `id` would merge down onto: its sibling just below.
    pub fn merge_down_target(&self, id: LayerId) -> Result<LayerId, Error> {
        let siblings = self.siblings_of(id)?;
        let pos = siblings
            .iter()
            .position(|l| l.id == id)
            .ok_or(Error::LayerNotFound(id))?;
        let upper = &siblings[pos];
        if matches!(
            upper.kind,
            LayerKind::Reference { .. } | LayerKind::Skeleton { .. }
        ) {
            return Err(Error::NotRaster(id));
        }
        let below = pos
            .checked_sub(1)
            .map(|p| &siblings[p])
            .ok_or(Error::NotRaster(id))?;
        if !below.is_raster() {
            return Err(Error::NotRaster(below.id));
        }
        Ok(below.id)
    }

    /// Composites layer `id` (a raster layer or a group) onto the raster
    /// layer below it, frame by frame, and removes it. The result keeps the
    /// lower layer's properties; the upper layer's opacity, blend mode and
    /// mask and the lower layer's cel opacities and enabled mask are baked
    /// into the pixels. Frames whose two cels are both linked to the same
    /// other frame stay linked. Returns the lower layer's id.
    pub fn merge_down(&mut self, id: LayerId) -> Result<LayerId, Error> {
        let below_id = self.merge_down_target(id)?;
        for l in [id, below_id] {
            if self.effective_locks(l)?.pixels {
                return Err(Error::Locked(l));
            }
        }
        let upper = self.layer(id).cloned().ok_or(Error::LayerNotFound(id))?;
        let below = self
            .layer(below_id)
            .cloned()
            .ok_or(Error::LayerNotFound(below_id))?;
        let LayerKind::Raster {
            cels: below_cels,
            mask: below_mask,
        } = &below.kind
        else {
            return Err(Error::NotRaster(below_id));
        };
        // A scratch document showing the two layers alone, at full strength
        // for the lower one, so its own opacity and blend apply later.
        let mut scratch = self.clone();
        let mut lower = below.clone();
        lower.props = LayerProps {
            visible: true,
            opacity: 255,
            blend: crate::document::BlendMode::Normal,
            clip: false,
            ..below.props.clone()
        };
        let mut top = upper.clone();
        top.props.visible = true;
        // A clipped pair both clip to the same base, which the merged layer
        // keeps clipping to; the upper one then shows unclipped over the
        // lower one.
        top.props.clip = upper.props.clip && !below.props.clip;
        scratch.s.layers = vec![lower, top];
        let opts = CompositeOptions {
            include_reference: false,
            background: false,
            onion: None,
        };
        let upper_key = |frame: usize| -> Option<ImageId> {
            match &upper.kind {
                LayerKind::Raster { cels, .. } => {
                    cels.get(frame).copied().flatten().map(|c| c.image)
                }
                _ => None,
            }
        };
        let grouped = !upper.is_raster();
        let mut done: BTreeMap<(Option<ImageId>, Option<ImageId>), Option<Cel>> = BTreeMap::new();
        let mut result = Vec::with_capacity(below_cels.len());
        for (frame, below_cel) in below_cels.iter().enumerate() {
            let key = (below_cel.map(|c| c.image), upper_key(frame));
            if !grouped && let Some(cel) = done.get(&key) {
                result.push(*cel);
                continue;
            }
            let rect = layer_bounds(&scratch, &scratch.s.layers[0], frame)
                .union(&layer_bounds(&scratch, &scratch.s.layers[1], frame))
                .intersect(&self.bounds());
            let cel = if rect.is_empty() {
                None
            } else {
                let px = composite(&scratch, frame, rect, &opts);
                let raster = to_raster(self, rect, &px);
                Some(Cel {
                    image: self.alloc_image(raster),
                    opacity: 255,
                })
            };
            if !grouped {
                done.insert(key, cel);
            }
            result.push(cel);
        }
        let keep_mask = below_mask.filter(|m| !m.enabled);
        if let Some(LayerKind::Raster { cels, mask }) =
            self.layer_mut(below_id).map(|l| &mut l.kind)
        {
            *cels = result;
            *mask = keep_mask;
        }
        self.remove_layer(id)?;
        Ok(below_id)
    }

    /// Composites every visible layer into one raster layer, frame by
    /// frame, and removes the layers it replaced, hidden ones too. Reference
    /// layers stay as they are. Returns the new layer's id.
    pub fn flatten(&mut self) -> Result<LayerId, Error> {
        let opts = CompositeOptions {
            include_reference: false,
            background: false,
            onion: None,
        };
        let mut cels = Vec::with_capacity(self.frame_count());
        for frame in 0..self.frame_count() {
            let rect = self
                .layers()
                .iter()
                .filter(|l| !kept_by_flatten(l))
                .fold(Rect::EMPTY, |r, l| r.union(&layer_bounds(self, l, frame)))
                .intersect(&self.bounds());
            let cel = if rect.is_empty() {
                None
            } else {
                let px = composite(self, frame, rect, &opts);
                let raster = to_raster(self, rect, &px);
                Some(Cel {
                    image: self.alloc_image(raster),
                    opacity: 255,
                })
            };
            cels.push(cel);
        }
        let id = LayerId(self.s.next_layer);
        self.s.next_layer += 1;
        let flat = Layer {
            id,
            props: LayerProps::named("Flattened"),
            kind: LayerKind::Raster { cels, mask: None },
        };
        let roots = std::mem::take(&mut self.s.layers);
        let first = roots
            .iter()
            .position(|l| !kept_by_flatten(l))
            .unwrap_or(roots.len());
        // The new layer takes the place of the lowest layer it replaces.
        let mut kept: Vec<Layer> = roots.into_iter().filter(kept_by_flatten).collect();
        kept.insert(first.min(kept.len()), flat);
        self.s.layers = kept;
        self.prune_rigs();
        self.collect_garbage();
        Ok(id)
    }

    /// Multiplies the layer's pixels by its mask in every frame and removes
    /// the mask. In an indexed document a pixel the mask covers less than
    /// half becomes transparent.
    pub fn apply_layer_mask(&mut self, id: LayerId) -> Result<Rect, Error> {
        if self.effective_locks(id)?.pixels {
            return Err(Error::Locked(id));
        }
        let (cels, mask) = match &self.layer(id).ok_or(Error::LayerNotFound(id))?.kind {
            LayerKind::Raster {
                cels,
                mask: Some(m),
            } => (cels.clone(), *m),
            _ => return Err(Error::NotRaster(id)),
        };
        let Some(Raster::Mask(mask_img)) = self.image(mask.image).cloned() else {
            return Err(Error::NotRaster(id));
        };
        let ti = self.transparent_index();
        let mut done: BTreeMap<ImageId, ImageId> = BTreeMap::new();
        let mut fresh = Vec::with_capacity(cels.len());
        for cel in &cels {
            let Some(cel) = cel else {
                fresh.push(None);
                continue;
            };
            if let Some(image) = done.get(&cel.image) {
                fresh.push(Some(Cel {
                    image: *image,
                    ..*cel
                }));
                continue;
            }
            let mut raster = self.images[&cel.image].clone();
            let bounds = tile_bounds(&raster);
            for p in bounds.points() {
                let m = mask_img.get(p.x, p.y);
                if m == 255 {
                    continue;
                }
                match &mut raster {
                    Raster::Rgba(img) => {
                        let c = img.get(p.x, p.y);
                        if c.a != 0 {
                            let a = (u32::from(c.a) * u32::from(m) + 127) / 255;
                            img.set(p.x, p.y, Rgba { a: a as u8, ..c });
                        }
                    }
                    Raster::Indexed(img) => {
                        if m < 128 {
                            img.set(p.x, p.y, ti);
                        }
                    }
                    Raster::Mask(_) => {}
                }
            }
            match &mut raster {
                Raster::Rgba(img) => img.compact(),
                Raster::Indexed(img) | Raster::Mask(img) => img.compact(),
            }
            let image = self.alloc_image(raster);
            done.insert(cel.image, image);
            fresh.push(Some(Cel { image, ..*cel }));
        }
        if let Some(LayerKind::Raster { cels, mask }) = self.layer_mut(id).map(|l| &mut l.kind) {
            *cels = fresh;
            *mask = None;
        }
        self.collect_garbage();
        Ok(self.bounds())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite::composite_frame;
    use crate::document::{BlendMode, NewLayer, Placement};

    const RED: Rgba = Rgba::opaque(255, 0, 0);
    const BLUE: Rgba = Rgba::opaque(0, 0, 255);

    fn paint(doc: &mut Document, layer: LayerId, frame: usize, x: i32, y: i32, c: Rgba) {
        doc.target(layer, frame)
            .unwrap()
            .raster
            .set_raw(x, y, c.to_u32());
    }

    fn two() -> (Document, LayerId, LayerId) {
        let mut d = Document::new(8, 8, ColorMode::Rgba).unwrap();
        let bottom = d.layers()[0].id;
        let top = d
            .add_layer(NewLayer::Raster, "top", Placement::default())
            .unwrap();
        (d, bottom, top)
    }

    #[test]
    fn merge_down_looks_the_same_and_leaves_one_layer() {
        let (mut d, bottom, top) = two();
        d.insert_frame(1, 100).unwrap();
        paint(&mut d, bottom, 0, 1, 1, BLUE);
        paint(&mut d, top, 0, 1, 1, Rgba::new(255, 0, 0, 128));
        paint(&mut d, top, 1, 2, 2, RED);
        d.update_layer(top, |p| {
            p.blend = BlendMode::Multiply;
            p.opacity = 200;
        })
        .unwrap();
        let before: Vec<_> = (0..2)
            .map(|f| composite_frame(&d, f, &CompositeOptions::EXPORT))
            .collect();
        assert_eq!(d.merge_down(top), Ok(bottom));
        assert_eq!(d.layers().len(), 1);
        for (f, want) in before.iter().enumerate() {
            assert_eq!(&composite_frame(&d, f, &CompositeOptions::EXPORT), want);
        }
    }

    #[test]
    fn merge_down_keeps_links_and_needs_a_raster_below() {
        let (mut d, bottom, top) = two();
        d.insert_frame(1, 100).unwrap();
        paint(&mut d, bottom, 0, 0, 0, BLUE);
        d.link_cel(bottom, 0, 1).unwrap();
        paint(&mut d, top, 0, 3, 3, RED);
        d.link_cel(top, 0, 1).unwrap();
        d.merge_down(top).unwrap();
        assert_eq!(d.linked_frames(bottom, 0).unwrap(), vec![0, 1]);
        assert_eq!(d.merge_down(bottom), Err(Error::NotRaster(bottom)));
    }

    #[test]
    fn duplicate_copies_pixels_and_keeps_inner_links() {
        let (mut d, bottom, _) = two();
        d.insert_frame(1, 100).unwrap();
        paint(&mut d, bottom, 0, 0, 0, BLUE);
        d.link_cel(bottom, 0, 1).unwrap();
        let copy = d.duplicate_layer(bottom).unwrap();
        assert_eq!(d.layers()[1].id, copy);
        assert_eq!(d.layer(copy).unwrap().props.name, "Layer 1 copy");
        assert_eq!(d.linked_frames(copy, 0).unwrap(), vec![0, 1]);
        assert_ne!(d.cel(copy, 0).unwrap(), d.cel(bottom, 0).unwrap());
        paint(&mut d, copy, 0, 0, 0, RED);
        assert_eq!(
            d.cel_raster(bottom, 0).unwrap().unwrap().get_raw(0, 0),
            BLUE.to_u32()
        );
    }

    #[test]
    fn flatten_bakes_visible_layers_and_keeps_references() {
        let (mut d, bottom, top) = two();
        paint(&mut d, bottom, 0, 0, 0, BLUE);
        paint(&mut d, top, 0, 1, 0, RED);
        let reference = d
            .add_layer(
                NewLayer::Reference(TiledImage::new(8, 8, RED)),
                "ref",
                Placement::default(),
            )
            .unwrap();
        let before = composite_frame(&d, 0, &CompositeOptions::EXPORT);
        let flat = d.flatten().unwrap();
        let ids: Vec<_> = d.layers().iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![flat, reference]);
        assert_eq!(composite_frame(&d, 0, &CompositeOptions::EXPORT), before);
    }

    #[test]
    fn applying_a_mask_bakes_it_into_alpha() {
        let (mut d, bottom, _) = two();
        paint(&mut d, bottom, 0, 0, 0, BLUE);
        paint(&mut d, bottom, 0, 1, 0, BLUE);
        d.add_layer_mask(bottom).unwrap();
        d.mask_target(bottom).unwrap().raster.set_raw(0, 0, 0);
        let before = composite_frame(&d, 0, &CompositeOptions::EXPORT);
        d.apply_layer_mask(bottom).unwrap();
        assert!(matches!(
            d.layer(bottom).unwrap().kind,
            LayerKind::Raster { mask: None, .. }
        ));
        assert_eq!(composite_frame(&d, 0, &CompositeOptions::EXPORT), before);
    }
}
