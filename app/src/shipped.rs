//! The documents pixl ships: the rig demo, a small character rigged and keyed
//! as a walk, and the rig lesson the tutorial opens, the same character with
//! no skeleton. Each opens as an untitled copy, so the file it came from is
//! never written. `tests/common/demo.rs` builds both with pixl-core, and
//! `tests/demo.rs` checks the files here are what it builds.

use pixl_core::Document;
use pixl_core::native;

/// A document pixl ships.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shipped {
    RigDemo,
    RigLesson,
}

impl Shipped {
    /// The document's `.pixl` file.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Shipped::RigDemo => include_bytes!("../assets/demo/rig-demo.pixl"),
            Shipped::RigLesson => include_bytes!("../assets/demo/rig-lesson.pixl"),
        }
    }

    /// The name the copy opens under.
    pub fn name(self) -> &'static str {
        match self {
            Shipped::RigDemo => "rig demo",
            Shipped::RigLesson => "rig lesson",
        }
    }

    /// A copy of the document.
    pub fn load(self) -> std::result::Result<Document, String> {
        native::load(self.bytes())
            .map(|l| l.document)
            .map_err(|e| format!("could not open the {}: {e}", self.name()))
    }
}
