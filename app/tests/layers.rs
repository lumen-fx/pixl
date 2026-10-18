//! The layers' groups, masks, clipping, locks, tags and layer commands,
//! driven over a rendered editor through both lists of the layers: the
//! timeline's rows, which the default layout shows, and the Layers panel.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use lumenui::prelude::dom;
use pixl::layers::List;
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

/// An id that lives as long as the steps that name it.
fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

fn row(list: List, n: u32) -> &'static str {
    leak(list.row(LayerId(n)))
}

fn eye(list: List, n: u32) -> &'static str {
    leak(list.eye(LayerId(n)))
}

/// `steps` with `list` on screen and room for its rows: the timeline as the
/// default layout has it, or the Layers panel shown with the panels around
/// it closed.
fn in_list(list: List, mut steps: Vec<Step>) -> Vec<Step> {
    let mut out = vec![Step::Wait];
    if list == List::Panel {
        out.extend([
            // What the Shown switch in Window > Panels and layouts runs.
            Step::Menu("ws-show-panel-layers"),
            Step::Click("panel-color-close"),
            Step::Click("panel-palette-close"),
            Step::Click("panel-history-close"),
            Step::Click("panel-brushes-close"),
            Step::Wait,
            Step::Wait,
        ]);
    }
    out.append(&mut steps);
    out
}

fn mask_of(doc: &Document, id: LayerId) -> Option<pixl_core::LayerMask> {
    match &doc.layer(id)?.kind {
        LayerKind::Raster { mask, .. } => *mask,
        _ => None,
    }
}

#[test]
fn a_group_takes_a_dragged_layer_in_either_list() {
    for list in List::ALL {
        let out = drive(
            editor_over(doc()),
            in_list(
                list,
                vec![
                    Step::Menu("lm-group"),
                    Step::Wait,
                    // Layer 3, the row under the group, dragged onto the
                    // middle of the group.
                    Step::Move(At::Element(row(list, 3))),
                    Step::Down(At::Element(row(list, 3)), PointerButton::Primary),
                    Step::Move(At::Element(row(list, 4))),
                    Step::Move(At::Element(row(list, 4))),
                    Step::Up(At::Element(row(list, 4)), PointerButton::Primary),
                    Step::Wait,
                ],
            ),
        );
        common::save(&out.frame, &format!("layers-group-{list:?}"));
        let group = LayerId(4);
        assert!(matches!(
            out.doc.layer(group).unwrap().kind,
            LayerKind::Group { .. }
        ));
        assert_eq!(out.doc.parent_of(LayerId(3)), Some(group), "{list:?}");
        assert_eq!(out.undo, ["New group", "Move layer"]);
    }
}

#[test]
fn a_drag_in_the_timeline_moves_a_layer_under_another() {
    // Rows top first: 3, 2, 1. Layer 3 dropped on the lower half of 1.
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Wait,
            Step::Move(At::Element("tl-layer-3")),
            Step::Down(At::Element("tl-layer-3"), PointerButton::Primary),
            Step::Move(At::Element("tl-layer-2")),
            Step::Move(At::Element("tl-layer-1")),
            Step::Up(At::Element("tl-layer-1"), PointerButton::Primary),
            Step::Wait,
        ],
    );
    let order: Vec<u32> = out.doc.layers().iter().map(|l| l.id.0).collect();
    assert_eq!(order, [3, 1, 2], "bottom first");
    assert_eq!(out.undo, ["Move layer"]);
    assert_eq!(out.at.1, LayerId(3), "the moved layer is the active one");
}

#[test]
fn a_mask_is_painted_then_applied() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("tl-layer-1"),
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
            Step::Click("tl-layer-1"),
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
            Step::Click("tl-lock-2"),
            Step::Wait,
            Step::Edit(|_| {
                let classes = |id: &str| dom::get_by_id(id).unwrap().classes();
                assert!(classes("tl-lock-2").iter().any(|c| c == "all"));
                assert!(classes("layer-lock-2").iter().any(|c| c == "all"));
            }),
            Step::Click("tl-layer-2"),
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
            Step::Click("tl-layer-1"),
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
            Step::ContextMenu("tl-layer-1"),
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
    let img = pixl::layers::reference_image(&path).unwrap();
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
            Step::Click("tl-edit-layer"),
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
    for list in List::ALL {
        let out = drive(
            editor_over(d.clone()),
            vec![
                Step::Edit(|_| assert!(dom::get_by_id("tl-layer-1").is_some())),
                Step::Click(leak(list.fold(LayerId(4)))),
                Step::Wait,
                Step::Wait,
                // The folded group's child has no row in either list.
                Step::Edit(|_| {
                    assert!(dom::get_by_id("layer-row-1").is_none());
                    assert!(dom::get_by_id("tl-layer-1").is_none());
                }),
                Step::Click(leak(list.fold(LayerId(4)))),
                Step::Wait,
                Step::Wait,
                Step::Edit(|_| assert!(dom::get_by_id("tl-layer-1").is_some())),
            ],
        );
        common::save(&out.frame, "layers-folded");
        let out = drive(
            editor_over(d.clone()),
            in_list(
                list,
                vec![
                    Step::Move(At::Element(row(list, 1))),
                    Step::Down(At::Element(row(list, 1)), PointerButton::Primary),
                    Step::Move(At::Element(row(list, 2))),
                    Step::Move(At::Element(row(list, 2))),
                    Step::Up(At::Element(row(list, 2)), PointerButton::Primary),
                    Step::Wait,
                ],
            ),
        );
        assert_eq!(out.doc.parent_of(LayerId(1)), None, "out of the group");
    }
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

/// A click with the pointer, pressed and let go over an element.
fn click(id: &'static str) -> [Step; 3] {
    [
        Step::Move(At::Element(id)),
        Step::Down(At::Element(id), PointerButton::Primary),
        Step::Up(At::Element(id), PointerButton::Primary),
    ]
}

fn shown(doc: &Document) -> Vec<u32> {
    doc.walk()
        .iter()
        .filter(|(_, l)| l.props.visible)
        .map(|(_, l)| l.id.0)
        .collect()
}

#[test]
fn an_alt_click_on_an_eye_shows_only_that_layer_and_another_puts_back() {
    let mut d = doc();
    d.update_layer(LayerId(1), |p| p.visible = false).unwrap();
    for list in List::ALL {
        let mut steps = vec![Step::Alt(true)];
        steps.extend(click(eye(list, 2)));
        steps.push(Step::Wait);
        steps.push(Step::Edit(|e| assert_eq!(shown(&e.doc), [2])));
        steps.extend(click(eye(list, 2)));
        steps.extend([Step::Wait, Step::Alt(false)]);
        let out = drive(editor_over(d.clone()), in_list(list, steps));
        common::save(&out.frame, &format!("layers-solo-{list:?}"));
        assert_eq!(shown(&out.doc), [2, 3], "layer 1 stays hidden as before");
        assert_eq!(out.undo, ["Show only", "Restore visibility"]);
    }
}

#[test]
fn the_eye_is_an_icon_that_shuts_when_hidden() {
    let mut d = doc();
    d.update_layer(LayerId(2), |p| p.visible = false).unwrap();
    let out = drive(
        editor_over(d),
        in_list(
            List::Panel,
            vec![
                Step::Edit(|_| {
                    let classes = |id: &str| dom::get_by_id(id).unwrap().classes();
                    for list in ["layer", "tl"] {
                        assert!(classes(&format!("{list}-eye-2")).iter().any(|c| c == "off"));
                        assert!(classes(&format!("{list}-eye-3")).iter().any(|c| c == "on"));
                    }
                }),
                // Showing it again turns both lists' eyes.
                Step::Click("tl-eye-2"),
                Step::Wait,
                Step::Wait,
                Step::Edit(|_| {
                    let classes = |id: &str| dom::get_by_id(id).unwrap().classes();
                    assert!(classes("tl-eye-2").iter().any(|c| c == "on"));
                    assert!(classes("layer-eye-2").iter().any(|c| c == "on"));
                }),
            ],
        ),
    );
    common::save(&out.frame, "layers-eyes");
}

#[test]
fn ctrl_and_shift_select_rows_for_the_visibility_commands() {
    let mut d = doc();
    d.add_layer(NewLayer::Raster, "Layer 4", Placement::default())
        .unwrap();
    for list in List::ALL {
        // Rows top first: 4, 3, 2, 1.
        let mut steps = Vec::new();
        steps.extend(click(row(list, 4)));
        steps.push(Step::HoldCtrl(true));
        steps.extend(click(row(list, 2)));
        // The click lands a tick after the button comes up.
        steps.extend([Step::Wait, Step::HoldCtrl(false), Step::Wait]);
        steps.push(Step::Edit(|e| {
            assert_eq!(e.layer, LayerId(2), "the last row clicked is active");
            let has = |id: &str, class: &str| {
                dom::get_by_id(id)
                    .unwrap()
                    .classes()
                    .iter()
                    .any(|c| c == class)
            };
            // Both lists show the pick.
            for [four, three, two] in [
                ["layer-row-4", "layer-row-3", "layer-row-2"],
                ["tl-layer-4", "tl-layer-3", "tl-layer-2"],
            ] {
                assert!(has(four, "selected"), "{four}");
                assert!(has(two, "active"), "{two}");
                assert!(!has(three, "selected"), "{three}");
            }
        }));
        // A right click inside the selection keeps it.
        steps.extend([
            Step::ContextMenu(row(list, 4)),
            Step::Wait,
            Step::Menu("lm-hide-selected"),
            Step::Wait,
        ]);
        steps.push(Step::Edit(|e| assert_eq!(shown(&e.doc), [1, 3])));
        // Shift selects from the active layer, 4, down to 3.
        steps.push(Step::Shift(true));
        steps.extend(click(row(list, 3)));
        steps.extend([Step::Wait, Step::Shift(false), Step::Wait]);
        steps.push(Step::Edit(|e| assert_eq!(e.layer, LayerId(3))));
        steps.extend([Step::Menu("lm-show-only"), Step::Wait]);
        steps.push(Step::Edit(|e| assert_eq!(shown(&e.doc), [3, 4])));
        steps.extend([Step::Wait, Step::Wait]);
        let out = drive(editor_over(d.clone()), in_list(list, steps));
        common::save(&out.frame, &format!("layers-selected-{list:?}"));
        let out = drive(
            editor_over(out.doc),
            vec![Step::Menu("lm-show-all"), Step::Wait],
        );
        assert_eq!(shown(&out.doc), [1, 2, 3, 4]);
        assert_eq!(out.undo, ["Show all layers"]);
    }
}

#[test]
fn an_alt_click_inside_a_group_keeps_the_group_shown() {
    let mut d = doc();
    let g = d
        .add_layer(NewLayer::Group, "Group", Placement::default())
        .unwrap();
    for id in [LayerId(1), LayerId(2)] {
        d.move_layer(
            id,
            Placement {
                parent: Some(g),
                index: None,
            },
        )
        .unwrap();
    }
    d.update_layer(g, |p| p.visible = false).unwrap();
    // Rows top first: Group 4 with 2 and 1 in it, then 3.
    let mut steps = vec![Step::Alt(true)];
    steps.extend(click("tl-eye-1"));
    steps.extend([Step::Wait, Step::Alt(false), Step::Wait]);
    let out = drive(editor_over(d), in_list(List::Timeline, steps));
    common::save(&out.frame, "layers-solo-group");
    assert_eq!(shown(&out.doc), [4, 1], "the group and layer 1");
    let px = pixl_core::composite_frame(&out.doc, 0, &pixl_core::CompositeOptions::EXPORT);
    assert_eq!(px[16 + 1], Rgba::opaque(255, 0, 0), "layer 1 shows");
    assert_eq!(out.undo, ["Show only"]);
}

#[test]
fn a_drag_down_the_eyes_sets_each_like_the_first() {
    let mut d = doc();
    d.add_layer(NewLayer::Raster, "Layer 4", Placement::default())
        .unwrap();
    d.update_layer(LayerId(3), |p| p.visible = false).unwrap();
    for list in List::ALL {
        let out = drive(
            editor_over(d.clone()),
            in_list(
                list,
                vec![
                    Step::Move(At::Element(eye(list, 4))),
                    Step::Down(At::Element(eye(list, 4)), PointerButton::Primary),
                    Step::Move(At::Element(eye(list, 3))),
                    Step::Move(At::Element(eye(list, 2))),
                    Step::Up(At::Element(eye(list, 2)), PointerButton::Primary),
                    Step::Wait,
                    Step::Wait,
                ],
            ),
        );
        common::save(&out.frame, &format!("layers-eye-drag-{list:?}"));
        // Layer 4's eye went off, so 3 stays off and 2 goes off.
        assert_eq!(shown(&out.doc), [1], "{list:?}");
        assert_eq!(out.undo, ["Visibility"], "one undo step");
        let order: Vec<u32> = out.doc.layers().iter().map(|l| l.id.0).collect();
        assert_eq!(order, [1, 2, 3, 4], "no layer moved");
    }
}
