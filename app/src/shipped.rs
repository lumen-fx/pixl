//! The documents pixl ships: the rig demo, a small character rigged and keyed
//! as a walk, and the lessons the tutorials open, the rig lesson among them,
//! the same character with no skeleton. Each opens as an untitled copy, so
//! the file it came from is never written. `tests/common/demo.rs` builds
//! them with pixl-core, and `tests/demo.rs` checks the files here are what it
//! builds.

use std::sync::OnceLock;

use pixl_core::Document;
use pixl_core::native;

/// A document pixl ships.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shipped {
    RigDemo,
    RigLesson,
    StartLesson,
    LayersLesson,
    SelectionLesson,
    AnimationLesson,
    PaletteLesson,
}

impl Shipped {
    pub const ALL: [Shipped; 7] = [
        Shipped::RigDemo,
        Shipped::RigLesson,
        Shipped::StartLesson,
        Shipped::LayersLesson,
        Shipped::SelectionLesson,
        Shipped::AnimationLesson,
        Shipped::PaletteLesson,
    ];

    /// The document's `.pixl` file.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Shipped::RigDemo => include_bytes!("../assets/demo/rig-demo.pixl"),
            Shipped::RigLesson => include_bytes!("../assets/demo/rig-lesson.pixl"),
            Shipped::StartLesson => include_bytes!("../assets/demo/start-lesson.pixl"),
            Shipped::LayersLesson => include_bytes!("../assets/demo/layers-lesson.pixl"),
            Shipped::SelectionLesson => include_bytes!("../assets/demo/selections-lesson.pixl"),
            Shipped::AnimationLesson => include_bytes!("../assets/demo/animation-lesson.pixl"),
            Shipped::PaletteLesson => include_bytes!("../assets/demo/palettes-lesson.pixl"),
        }
    }

    /// The name the copy opens under.
    pub fn name(self) -> &'static str {
        match self {
            Shipped::RigDemo => "rig demo",
            Shipped::RigLesson => "rig lesson",
            Shipped::StartLesson => "getting started",
            Shipped::LayersLesson => "layers lesson",
            Shipped::SelectionLesson => "selections lesson",
            Shipped::AnimationLesson => "animation lesson",
            Shipped::PaletteLesson => "palettes lesson",
        }
    }

    /// A short name for the files made from the document, such as its
    /// thumbnail, and for its entry on the start screen. No slug is another
    /// one followed by a dash and more.
    pub fn slug(self) -> &'static str {
        match self {
            Shipped::RigDemo => "rig-demo",
            Shipped::RigLesson => "rig-lesson",
            Shipped::StartLesson => "start-lesson",
            Shipped::LayersLesson => "layers-lesson",
            Shipped::SelectionLesson => "selections-lesson",
            Shipped::AnimationLesson => "animation-lesson",
            Shipped::PaletteLesson => "palettes-lesson",
        }
    }

    /// The document's width and height, read once.
    pub fn size(self) -> (u32, u32) {
        static SIZES: [OnceLock<(u32, u32)>; Shipped::ALL.len()] =
            [const { OnceLock::new() }; Shipped::ALL.len()];
        *SIZES[self as usize]
            .get_or_init(|| self.load().map_or((0, 0), |d| (d.width(), d.height())))
    }

    /// A copy of the document.
    pub fn load(self) -> std::result::Result<Document, String> {
        native::load(self.bytes())
            .map(|l| l.document)
            .map_err(|e| format!("could not open the {}: {e}", self.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_document_has_its_own_slug_and_place() {
        for (i, a) in Shipped::ALL.into_iter().enumerate() {
            assert_eq!(a as usize, i);
            for b in Shipped::ALL {
                assert!(a == b || !b.slug().starts_with(&format!("{}-", a.slug())));
            }
        }
    }
}
