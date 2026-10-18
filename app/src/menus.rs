//! The Select, Image, Filter and View menus: their buttons in the menu row,
//! and the commands their items, their keys and the command palette run.
//! Each tick's chosen commands land in [`Chosen`], where the selection,
//! image, filter and view modules pick up their own.

use lumenui::lumen_core::input::MenuClicked;
use lumenui::prelude::*;

use crate::home::Home;
use crate::ids::ElementIds;
use crate::keymap::KeyCommands;

/// The command ids chosen this tick: menu items, keys, the palette, and the
/// ids of clicked elements (dialog buttons).
#[derive(Resource, Default, Debug)]
pub struct Chosen(pub Vec<String>);

impl Chosen {
    pub fn has(&self, id: &str) -> bool {
        self.0.iter().any(|c| c == id)
    }

    /// The chosen ids that start with `prefix`.
    pub fn with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .map(String::as_str)
            .filter(move |c| c.starts_with(prefix))
    }
}

/// The menu row buttons this module opens, and the menu each opens.
const MENUS: [(&str, &str); 4] = [
    ("menu-select", "select-menu"),
    ("menu-image", "image-menu"),
    ("menu-filter", "filter-menu"),
    ("menu-view", "view-menu"),
];

/// Gathers this tick's commands and opens the menus.
pub fn collect(
    mut chosen: ResMut<Chosen>,
    keys: Res<KeyCommands>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    home: Res<Home>,
    mut signals: Signals,
) {
    chosen.0.clear();
    let clicked: Vec<String> = clicks.read().filter_map(|c| ids.id_of(c.entity)).collect();
    let menus: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    if home.open {
        return;
    }
    for id in &clicked {
        if let Some((_, menu)) = MENUS.iter().find(|(button, _)| button == id) {
            signals.toggle(&format!("__menu_open:{menu}"));
        }
    }
    for (id, repeat) in &keys.pressed {
        // Only the brush size steps with a held key.
        if !*repeat || id.starts_with("brush-") {
            chosen.0.push(id.clone());
        }
    }
    chosen.0.extend(menus);
    chosen.0.extend(clicked);
}
