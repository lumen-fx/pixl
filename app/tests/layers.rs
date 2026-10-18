//! The layers panel's groups, masks, clipping, locks, tags and layer
//! commands, driven over a rendered editor.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use lumenui::prelude::dom;
use pixl_core::{ColorMode, Document, LayerId, LayerKind, Locks, NewLayer, Placement, Rgba};

/// Layer 1 at the bottom with a red pixel at (1, 1), then layers 2 and 3.
fn doc() -> Document {
    let mut d = Document::new(16, 16, ColorMode::Rgba).unwrap();
    let base = d.layers()[0].id;
    d.target(base, 0)
        .unwrap()
        .raster
        .set_raw(1, 1, Rgba::opaque(255, 0, 0).to_u32());
    d.add_layer(NewLayer::Raster, "Layer 2", Placement::default())
        .unwrap();
    d.add_layer(NewLayer::Raster, "Layer 3", Placement::default())
        .unwrap();
    d
}

fn mask_of(doc: &Document, id: LayerId) -> Option<pixl_core::LayerMask> {
    match &doc.layer(id)?.kind {
        LayerKind::Raster { mask, .. } => *mask,
        _ => None,
    }
}

#[test]
fn a_group_takes_a_dragged_layer_and_folds() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("layer-group"),
            Step::Wait,
            // Layer 3, the row under the group, dragged onto the middle of
            // the group. The bottom rows are scrolled out of the panel's
            // view at the default layout.
            Step::Move(At::Element("layer-row-3")),
            Step::Down(At::Element("layer-row-3"), PointerButton::Primary),
            Step::Move(At::Element("layer-row-4")),
            Step::Move(At::Element("layer-row-4")),
            Step::Up(At::Element("layer-row-4"), PointerButton::Primary),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "layers-group");
    let group = LayerId(4);
    assert!(matches!(
        out.doc.layer(group).unwrap().kind,
        LayerKind::Group { .. }
    ));
    assert_eq!(out.doc.parent_of(LayerId(3)), Some(group));
    assert_eq!(out.undo, ["New group", "Move layer"]);
}

#[test]
fn a_mask_is_painted_then_applied() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("layer-row-1"),
            Step::Menu("lm-mask-add"),
            Step::Wait,
            // Black on the mask hides the red pixel.
            Step::Down(At::Doc(1, 1), PointerButton::Primary),
            Step::Up(At::Doc(1, 1), PointerButton::Primary),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "layers-mask");
    let mask = mask_of(&out.doc, LayerId(1)).expect("a mask");
    assert_eq!(out.doc.image(mask.image).unwrap().get_raw(1, 1), 0);
    let cel = out.doc.cel_raster(LayerId(1), 0).unwrap().unwrap();
    assert_eq!(
        cel.get_raw(1, 1),
        Rgba::opaque(255, 0, 0).to_u32(),
        "pixels untouched"
    );

    let out = drive(
        editor_over(out.doc),
        vec![
            Step::Click("layer-row-1"),
            Step::Menu("lm-mask-apply"),
            Step::Wait,
        ],
    );
    assert!(mask_of(&out.doc, LayerId(1)).is_none());
    let cel = out.doc.cel_raster(LayerId(1), 0).unwrap().unwrap();
    assert_eq!(
        Rgba::from_u32(cel.get_raw(1, 1)).a,
        0,
        "the mask is baked in"
    );
}

#[test]
fn clipping_locks_and_the_lock_button() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Menu("lm-clip"),
            Step::Click("layer-lock-2"),
            Step::Wait,
            Step::Click("layer-row-2"),
            Step::Down(At::Doc(3, 3), PointerButton::Primary),
            Step::Up(At::Doc(3, 3), PointerButton::Primary),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "layers-locked");
    assert!(out.doc.layer(LayerId(3)).unwrap().props.clip);
    assert_eq!(out.doc.layer(LayerId(2)).unwrap().props.locks, Locks::ALL);
    let painted = out
        .doc
        .cel_raster(LayerId(2), 0)
        .unwrap()
        .is_some_and(|r| r.get_raw(3, 3) != 0);
    assert!(!painted, "nothing painted");
    assert!(out.message.is_some(), "the refusal is said");
}

#[test]
fn duplicate_merge_down_and_flatten() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("layer-row-1"),
            Step::Click("layer-duplicate"),
            Step::Wait,
            Step::Menu("lm-merge"),
            Step::Wait,
        ],
    );
    // Layer 1 copied above itself, then the copy merged back down.
    assert_eq!(out.doc.layers().len(), 3);
    assert_eq!(out.undo, ["Duplicate layer", "Merge down"]);
    let out = drive(
        editor_over(out.doc),
        vec![Step::Menu("lm-flatten"), Step::Wait],
    );
    assert_eq!(out.doc.layers().len(), 1);
    let flat = out.doc.layers()[0].id;
    let cel = out.doc.cel_raster(flat, 0).unwrap().unwrap();
    assert_eq!(cel.get_raw(1, 1), Rgba::opaque(255, 0, 0).to_u32());
}

#[test]
fn the_properties_dialog_renames_tags_and_locks() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("layer-properties"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Text("lp_name", "Hero"),
            Step::Text("lp_opacity", "50"),
            Step::Flag("lp_lock_alpha", true),
            Step::Click("lp-tag-3"),
            Step::Click("lp-ok"),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "layers-tagged");
    let p = &out.doc.layer(LayerId(3)).unwrap().props;
    assert_eq!(p.name, "Hero");
    assert_eq!(p.opacity, 128);
    assert!(p.locks.alpha && !p.locks.pixels);
    assert!(p.color_tag.is_some());
    assert_eq!(out.undo, ["Layer properties"]);
}

#[test]
fn a_right_click_on_a_row_acts_on_that_layer() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::ContextMenu("layer-row-1"),
            Step::Wait,
            Step::Menu("lm-duplicate"),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "layers-menu");
    let names: Vec<&str> = out
        .doc
        .layers()
        .iter()
        .map(|l| l.props.name.as_str())
        .collect();
    assert_eq!(names, ["Layer 1", "Layer 1 copy", "Layer 2", "Layer 3"]);
}

fn add_reference(e: &mut pixl::Editor) {
    let dir = std::env::temp_dir().join(format!("pixl-ref-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ref.png");
    let mut src = Document::new(4, 4, ColorMode::Rgba).unwrap();
    let id = src.layers()[0].id;
    src.target(id, 0)
        .unwrap()
        .raster
        .set_raw(2, 2, Rgba::opaque(0, 0, 255).to_u32());
    std::fs::write(&path, pixl_core::io::export_frame_png(&src, 0, 1).unwrap()).unwrap();
    let img = pixl::layers::reference_image(&path, (16, 16)).unwrap();
    e.add_reference(img, "ref".into());
}

#[test]
fn a_reference_image_shows_but_does_not_export() {
    let out = drive(
        editor_over(doc()),
        vec![Step::Edit(add_reference), Step::Wait, Step::Wait],
    );
    common::save(&out.frame, "layers-reference");
    let reference = out.doc.layers().last().unwrap();
    assert!(matches!(reference.kind, LayerKind::Reference { .. }));
    assert_eq!(reference.props.opacity, 128);
    let exported = pixl_core::composite_frame(&out.doc, 0, &pixl_core::CompositeOptions::EXPORT);
    assert_eq!(exported[2 * 16 + 2].a, 0, "references stay out of exports");
}

#[test]
fn the_properties_dialog_opens_on_the_layer_as_it_is() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("layer-properties"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "layers-properties-open");
}

#[test]
fn a_folded_group_hides_its_layers_and_a_drag_takes_one_out() {
    let mut d = doc();
    let g = d
        .add_layer(NewLayer::Group, "Group", Placement::default())
        .unwrap();
    d.move_layer(
        LayerId(1),
        Placement {
            parent: Some(g),
            index: None,
        },
    )
    .unwrap();
    let out = drive(
        editor_over(d.clone()),
        vec![
            Step::Edit(|_| assert!(dom::get_by_id("layer-row-1").is_some())),
            Step::Click("layer-fold-4"),
            Step::Wait,
            Step::Wait,
            // The folded group's child has no row in the list or the
            // timeline.
            Step::Edit(|_| {
                assert!(dom::get_by_id("layer-row-1").is_none());
                assert!(dom::get_by_id("tl-layer-1").is_none());
            }),
        ],
    );
    common::save(&out.frame, "layers-folded");
    let out = drive(
        editor_over(d),
        vec![
            Step::Move(At::Element("layer-row-1")),
            Step::Down(At::Element("layer-row-1"), PointerButton::Primary),
            Step::Move(At::Element("layer-row-2")),
            Step::Move(At::Element("layer-row-2")),
            Step::Up(At::Element("layer-row-2"), PointerButton::Primary),
            Step::Wait,
        ],
    );
    assert_eq!(out.doc.parent_of(LayerId(1)), None, "out of the group");
}

#[test]
fn a_mask_turns_off_and_goes() {
    let mut d = doc();
    d.add_layer_mask(LayerId(3)).unwrap();
    let out = drive(
        editor_over(d),
        vec![
            Step::Menu("lm-mask-toggle"),
            Step::Wait,
            Step::Menu("lm-mask-delete"),
            Step::Wait,
        ],
    );
    assert!(mask_of(&out.doc, LayerId(3)).is_none());
    assert_eq!(out.undo, ["Disable mask", "Delete mask"]);
}
