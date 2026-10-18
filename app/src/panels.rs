//! The right-hand panels: color, palette, layers and history. Each panel's
//! content is markup the editor writes when what it shows changes; clicks
//! come back as click messages on the ids the markup carries.

use lumenui::ecs::query::Added;
use lumenui::lumen_core::input::{EditCommitted, PointerButton, PointerPressed, PointerReleased};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::history::Transaction;
use pixl_core::{BlendMode, LayerId, Rgba, Scope};

use crate::editor::Editor;
use crate::home::Home;
use crate::ids::ElementIds;
use crate::keymap::KeyCommands;
use crate::tools::{hex, set_if_changed};
use crate::view::parse_hex;

/// The name a blend mode has in the layers panel, and its dropdown value.
pub fn blend_name(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "Normal",
        BlendMode::Multiply => "Multiply",
        BlendMode::Screen => "Screen",
        BlendMode::Overlay => "Overlay",
        BlendMode::Darken => "Darken",
        BlendMode::Lighten => "Lighten",
        BlendMode::ColorDodge => "Color dodge",
        BlendMode::ColorBurn => "Color burn",
        BlendMode::HardLight => "Hard light",
        BlendMode::SoftLight => "Soft light",
        BlendMode::Difference => "Difference",
        BlendMode::Exclusion => "Exclusion",
        BlendMode::Hue => "Hue",
        BlendMode::Saturation => "Saturation",
        BlendMode::Color => "Color",
        BlendMode::Luminosity => "Luminosity",
        BlendMode::Add => "Add",
        BlendMode::Subtract => "Subtract",
    }
}

pub fn blend_from_name(name: &str) -> Option<BlendMode> {
    BlendMode::ALL.into_iter().find(|m| blend_name(*m) == name)
}

/// Text made safe for a markup attribute value.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '{' | '}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// What each panel last showed, and a layer property edit in progress.
#[derive(Resource, Default)]
pub struct Panels {
    palette: Option<String>,
    /// The history list as shown: its labels and the current entry.
    history: Option<(Vec<String>, usize)>,
    history_len: Option<usize>,
    /// The document state the palette and history were last brought up to
    /// date with: generation, revision, undo and redo entries.
    seen: Option<(u64, u64, usize, usize)>,
    colors: Option<(Rgba, Rgba)>,
    /// The foreground color as the channel sliders and the hex field last
    /// showed it.
    channels: Option<Rgba>,
    /// The active layer's opacity and blend as the controls last showed them.
    props: Option<(LayerId, u8, BlendMode)>,
    /// An opacity or blend change, kept open while the control is dragged
    /// so it lands as one undo step.
    editing: Option<Transaction>,
}

fn palette_markup(editor: &Editor) -> String {
    let mut out = String::new();
    for (i, c) in editor.doc.palette().colors().iter().enumerate() {
        // The swatch's fill is the palette's data, so it rides in the markup;
        // an empty entry takes the theme's look.
        if c.a == 0 {
            out.push_str(&format!(r#"<tile id="swatch-{i}" class="swatch empty"/>"#));
        } else {
            out.push_str(&format!(
                r#"<tile id="swatch-{i}" class="swatch" bg="{}"/>"#,
                hex(*c)
            ));
        }
    }
    out
}

/// The history list's entries (the document as opened, then each edit,
/// undone ones last) and the index of the current one.
fn history_entries(editor: &Editor) -> (Vec<String>, usize) {
    let current = editor.history.undo_labels().count();
    let labels = std::iter::once("Opened")
        .chain(editor.history.undo_labels())
        .chain(editor.history.redo_labels())
        .map(str::to_string)
        .collect();
    (labels, current)
}

fn history_class(k: usize, current: usize) -> &'static str {
    match k.cmp(&current) {
        std::cmp::Ordering::Less => "history-row",
        std::cmp::Ordering::Equal => "history-row current",
        std::cmp::Ordering::Greater => "history-row undone",
    }
}

fn history_markup(labels: &[String], current: usize) -> String {
    let mut out = String::new();
    for (k, label) in labels.iter().enumerate() {
        out.push_str(&format!(
            r#"<row id="history-{k}" class="{}"><label class="history-label" text="{}"/></row>"#,
            history_class(k, current),
            escape(label)
        ));
    }
    out
}

/// Brings the history list up to date: an undo or a redo moves the current
/// entry by changing two rows' classes; a new edit writes the list again.
fn show_history(panels: &mut Panels, entries: (Vec<String>, usize)) -> bool {
    let (labels, current) = &entries;
    match &panels.history {
        Some((shown, was)) if shown == labels => {
            if was == current {
                return false;
            }
            for k in [*was, *current] {
                if let Some(row) = dom::get_by_id(&format!("history-{k}")) {
                    row.set_class(history_class(k, *current));
                }
            }
            // Rows between the two change from done to undone or back.
            let (a, b) = ((*was).min(*current), (*was).max(*current));
            for k in a + 1..b {
                if let Some(row) = dom::get_by_id(&format!("history-{k}")) {
                    row.set_class(history_class(k, *current));
                }
            }
        }
        _ => {
            let Some(node) = dom::get_by_id("history-list") else {
                return false;
            };
            node.set_inner_markup(&history_markup(labels, *current));
        }
    }
    panels.history = Some(entries);
    true
}

/// Rewrites a panel's content when what it shows changed.
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

/// Keeps the panels showing the document.
pub fn show_panels(
    editor: Res<Editor>,
    mut panels: ResMut<Panels>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let mut changed = false;
    // The palette and the history change only with the document.
    let undo = editor.history.undo_labels().count();
    let entries = undo + editor.history.redo_labels().count();
    let now = (editor.generation, editor.revision, undo, entries);
    let current = panels.seen == Some(now);
    if !current {
        changed |= refresh(
            &mut panels.palette,
            "palette-swatches",
            palette_markup(&editor),
        );
    }
    if !current && show_history(&mut panels, history_entries(&editor)) {
        changed = true;
        // A new edit scrolls the list to its end, where the edit is.
        if panels.history_len.is_some_and(|n| entries > n) {
            signals.set("history_scroll", f64::from(u32::MAX));
        }
        panels.history_len = Some(entries);
    }
    if !current && panels.palette.is_some() && panels.history.is_some() {
        panels.seen = Some(now);
    }
    if panels.colors != Some((editor.fg, editor.bg))
        && let (Some(fg), Some(bg)) = (dom::get_by_id("color-fg"), dom::get_by_id("color-bg"))
    {
        fg.set_style("bg", &hex(editor.fg));
        bg.set_style("bg", &hex(editor.bg));
        panels.colors = Some((editor.fg, editor.bg));
        changed = true;
    }
    if panels.channels != Some(editor.fg) {
        let c = editor.fg;
        for (name, v) in [
            ("color_r", c.r),
            ("color_g", c.g),
            ("color_b", c.b),
            ("color_a", c.a),
        ] {
            signals.set(name, f64::from(v));
            signals.set(&format!("{name}_text"), v.to_string());
        }
        signals.set("color_hex", hex(c));
        panels.channels = Some(c);
        changed = true;
    }
    if let Some(layer) = editor.doc.layer(editor.layer) {
        let now = (layer.id, layer.props.opacity, layer.props.blend);
        if panels.props != Some(now) {
            let percent = (f64::from(layer.props.opacity) * 100.0 / 255.0).round();
            signals.set("layer_opacity", percent);
            signals.set("layer_opacity_text", format!("{percent}%"));
            signals.set("layer_blend", blend_name(layer.props.blend));
            panels.props = Some(now);
            changed = true;
        }
    }
    set_if_changed(&mut signals, "doc_title", editor.title());
    if changed {
        crate::repaint_soon(&wake);
    }
}

/// The ids of the click messages this tick, primary button only.
fn clicked(clicks: &mut MessageReader<ClickEvent>, ids: &ElementIds) -> Vec<String> {
    clicks.read().filter_map(|c| ids.id_of(c.entity)).collect()
}

/// The panel buttons, rows and swatches.
pub fn panel_clicks(
    mut editor: ResMut<Editor>,
    mut panels: ResMut<Panels>,
    mut clicks: MessageReader<ClickEvent>,
    mut presses: MessageReader<PointerPressed>,
    ids: ElementIds,
    hovered: Query<&LumenId, With<Hovered>>,
) {
    let editor = &mut *editor;
    for id in clicked(&mut clicks, &ids) {
        match id.as_str() {
            "layer-add" => {
                close_edit(editor, &mut panels);
                editor.add_layer();
            }
            "layer-delete" => {
                close_edit(editor, &mut panels);
                editor.delete_layer();
            }
            "layer-up" => {
                close_edit(editor, &mut panels);
                editor.move_layer(true);
            }
            "layer-down" => {
                close_edit(editor, &mut panels);
                editor.move_layer(false);
            }
            "color-swap" => std::mem::swap(&mut editor.fg, &mut editor.bg),
            "history-undo" => {
                close_edit(editor, &mut panels);
                editor.undo();
            }
            "history-redo" => {
                close_edit(editor, &mut panels);
                editor.redo();
            }
            other => {
                if let Some(n) = other
                    .strip_prefix("layer-eye-")
                    .and_then(|n| n.parse().ok())
                {
                    close_edit(editor, &mut panels);
                    let id = LayerId(n);
                    editor.update_layer("Visibility", id, |p| p.visible = !p.visible);
                } else if let Some(n) = other
                    .strip_prefix("layer-row-")
                    .and_then(|n| n.parse().ok())
                {
                    close_edit(editor, &mut panels);
                    if editor.doc.layer(LayerId(n)).is_some() {
                        editor.layer = LayerId(n);
                    }
                } else if let Some(i) = other
                    .strip_prefix("swatch-")
                    .and_then(|n| n.parse::<u8>().ok())
                {
                    let c = editor.doc.palette().get(i);
                    if c.a > 0 {
                        editor.fg = c;
                    }
                } else if let Some(k) = other
                    .strip_prefix("history-")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    close_edit(editor, &mut panels);
                    jump_to(editor, k);
                }
            }
        }
    }
    // The secondary button picks a swatch as the background color.
    for press in presses.read() {
        if press.button != PointerButton::Secondary {
            continue;
        }
        for id in &hovered {
            if let Some(i) =
                id.0.strip_prefix("swatch-")
                    .and_then(|n| n.parse::<u8>().ok())
            {
                let c = editor.doc.palette().get(i);
                if c.a > 0 {
                    editor.bg = c;
                }
            }
        }
    }
}

/// Undoes or redoes until `k` edits are applied.
pub fn jump_to(editor: &mut Editor, k: usize) {
    loop {
        let applied = editor.history.undo_labels().count();
        if applied > k {
            if editor.undo().is_none() {
                break;
            }
        } else if applied < k {
            if editor.redo().is_none() {
                break;
            }
        } else {
            break;
        }
    }
}

/// Ends an opacity or blend edit, making it one undo step.
pub fn close_edit(editor: &mut Editor, panels: &mut Panels) {
    if let Some(tx) = panels.editing.take() {
        editor.history.commit(&editor.doc, tx);
        editor.touch();
    }
}

/// Applies the opacity slider and the blend dropdown to the active layer.
/// A drag of the slider is one undo step, closed when the pointer comes up.
pub fn layer_controls(
    mut editor: ResMut<Editor>,
    mut panels: ResMut<Panels>,
    signals: Signals,
    mut released: MessageReader<PointerReleased>,
) {
    let editor = &mut *editor;
    let Some((id, opacity, blend)) = panels.props else {
        return;
    };
    if id != editor.layer {
        close_edit(editor, &mut panels);
        return;
    }
    let want_opacity = signals
        .get::<f64>("layer_opacity")
        .map(|p| (p.clamp(0.0, 100.0) * 255.0 / 100.0).round() as u8);
    let want_blend = signals
        .get::<String>("layer_blend")
        .and_then(|n| blend_from_name(&n));
    let opacity_moved = want_opacity.is_some_and(|o| {
        // The slider moves in whole percent; only a different percent is
        // a change.
        let shown = (f64::from(opacity) * 100.0 / 255.0).round();
        let wanted = (f64::from(o) * 100.0 / 255.0).round();
        shown != wanted
    });
    let blend_moved = want_blend.is_some_and(|b| b != blend);
    if opacity_moved || blend_moved {
        if panels.editing.is_none() {
            let label = if blend_moved { "Blend mode" } else { "Opacity" };
            panels.editing = Some(editor.history.begin(
                &editor.doc,
                label,
                Scope::Images(Vec::new()),
            ));
        }
        let _ = editor.doc.update_layer(id, |p| {
            if let Some(o) = want_opacity.filter(|_| opacity_moved) {
                p.opacity = o;
            }
            if let Some(b) = want_blend.filter(|_| blend_moved) {
                p.blend = b;
            }
        });
        editor.touch();
        if blend_moved {
            close_edit(editor, &mut panels);
        }
    }
    if released.read().count() > 0 {
        close_edit(editor, &mut panels);
    }
}

/// The color panel: the channel sliders and the hex field edit the
/// foreground color.
pub fn color_controls(
    mut editor: ResMut<Editor>,
    mut panels: ResMut<Panels>,
    mut signals: Signals,
    committed: Query<&LumenId, Added<EditCommitted>>,
) {
    // What the controls showed; a difference is the user's doing.
    let Some(c) = panels.channels else {
        return;
    };
    let channel = |s: &Signals, name: &str, was: u8| {
        s.get::<f64>(name)
            .map_or(was, |v| v.round().clamp(0.0, 255.0) as u8)
    };
    let next = Rgba::new(
        channel(&signals, "color_r", c.r),
        channel(&signals, "color_g", c.g),
        channel(&signals, "color_b", c.b),
        channel(&signals, "color_a", c.a),
    );
    if next != c {
        editor.fg = next;
        panels.channels = Some(next);
        signals.set("color_hex", hex(next));
        for (name, v) in [
            ("color_r", next.r),
            ("color_g", next.g),
            ("color_b", next.b),
            ("color_a", next.a),
        ] {
            signals.set(&format!("{name}_text"), v.to_string());
        }
        return;
    }
    if committed.iter().any(|id| id.0 == "color-hex")
        && let Some(text) = signals.get::<String>("color_hex")
    {
        let text = if text.starts_with('#') {
            text
        } else {
            format!("#{text}")
        };
        match parse_hex(&text) {
            Some(c) => editor.fg = c,
            None => signals.set("color_hex", hex(editor.fg)),
        }
    }
}

/// Undo and redo from the keyboard, with the keys the keymap gives them
/// (Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y by default). A held key repeats.
pub fn history_keys(
    mut editor: ResMut<Editor>,
    mut panels: ResMut<Panels>,
    keys: Res<KeyCommands>,
    home: Res<Home>,
    toolbox: Res<crate::toolbox::Toolbox>,
) {
    // The start screen keeps the editor's keys until a document shows, and
    // a paint still being drawn on worker threads finishes first.
    if home.open || toolbox.painting() {
        return;
    }
    let editor = &mut *editor;
    for (id, _) in &keys.pressed {
        match id.as_str() {
            "undo" => {
                close_edit(editor, &mut panels);
                editor.undo();
            }
            "redo" => {
                close_edit(editor, &mut panels);
                editor.redo();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_in_markup() {
        assert_eq!(
            escape(r#"a "b" <c> & {d}"#),
            "a &quot;b&quot; &lt;c&gt; &amp; d"
        );
    }

    #[test]
    fn every_blend_mode_has_a_name_that_reads_back() {
        for m in BlendMode::ALL {
            assert_eq!(blend_from_name(blend_name(m)), Some(m));
        }
    }
}
