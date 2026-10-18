//! The element an input event is about.
//!
//! Lumen names the innermost element under the pointer, as a browser sets
//! `event.target`: a press on a tool button's icon names the icon, and a
//! click on a part of a row names that part. The editor's commands are keyed
//! on the ids of buttons, rows and panels, so input resolves an entity to the
//! nearest element, itself or an ancestor, that has an id.

use lumenui::ecs::hierarchy::ChildOf;
use lumenui::ecs::system::SystemParam;
use lumenui::prelude::*;

/// Resolves entities to the ids of the elements they are part of.
#[derive(SystemParam)]
pub struct ElementIds<'w, 's> {
    ids: Query<'w, 's, &'static LumenId>,
    parents: Query<'w, 's, &'static ChildOf>,
}

/// No element at or above an entity has an id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoId;

impl ElementIds<'_, '_> {
    /// The id of `entity` or of its nearest ancestor that has one.
    pub fn of(&self, entity: Entity) -> std::result::Result<&LumenId, NoId> {
        let mut e = entity;
        loop {
            if let Ok(id) = self.ids.get(e) {
                return Ok(id);
            }
            e = self.parents.get(e).map_err(|_| NoId)?.parent();
        }
    }

    /// [`ElementIds::of`] as an owned string.
    pub fn id_of(&self, entity: Entity) -> Option<String> {
        self.of(entity).ok().map(|id| id.0.clone())
    }

    /// The ids of `entity` and of every ancestor that has one, innermost
    /// first.
    pub fn up(&self, entity: Entity) -> impl Iterator<Item = &str> + '_ {
        let mut next = Some(entity);
        std::iter::from_fn(move || {
            loop {
                let e = next?;
                next = self.parents.get(e).ok().map(ChildOf::parent);
                if let Ok(id) = self.ids.get(e) {
                    return Some(id.0.as_str());
                }
            }
        })
    }
}
