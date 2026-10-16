//! The look settings: the dark or light token set and the pixel or plain UI
//! font. Both are classes on the root element; the stylesheet does the rest.

use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;

/// Which token set and which UI font the window uses.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub light: bool,
    pub plain_font: bool,
}

impl Look {
    /// The root element's classes for this look.
    pub fn classes(self) -> String {
        let mut classes = String::from("pixl");
        classes.push_str(if self.light { " light" } else { " dark" });
        if self.plain_font {
            classes.push_str(" font-plain");
        }
        classes
    }

    /// Puts the look on the window, and labels the two switches with what a
    /// click on each turns to.
    pub fn apply(self) {
        if let Some(root) = dom::get_by_id("app") {
            root.set_class(&self.classes());
        }
        if let Some(button) = dom::get_by_id("toggle-theme") {
            button.set_text(if self.light { "Dark" } else { "Light" });
        }
        if let Some(button) = dom::get_by_id("toggle-font") {
            button.set_text(if self.plain_font {
                "Pixel font"
            } else {
                "Plain font"
            });
        }
    }
}

/// Flips the look when one of its switches is clicked.
pub fn look_switches(
    mut clicks: MessageReader<ClickEvent>,
    ids: Query<&LumenId>,
    mut look: ResMut<Look>,
) {
    for click in clicks.read() {
        match ids.get(click.entity).map(|id| id.0.as_str()) {
            Ok("toggle-theme") => look.light = !look.light,
            Ok("toggle-font") => look.plain_font = !look.plain_font,
            _ => {}
        }
    }
}

/// Puts the [`Look`] resource on the window until the root element shows
/// it. DOM edits land on a later tick, so this checks rather than assumes.
pub fn show_look(look: Res<Look>, wake: Option<Res<WakeDeadline>>) {
    let Some(root) = dom::get_by_id("app") else {
        return;
    };
    let wanted = look.classes();
    let mut shown = root.classes();
    shown.sort();
    let mut expected: Vec<String> = wanted.split(' ').map(str::to_string).collect();
    expected.sort();
    if shown != expected {
        look.apply();
        crate::repaint_soon(&wake);
    }
}
