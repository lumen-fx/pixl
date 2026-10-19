//! Scripting for pixl, in candela: the API scripts and extensions call,
//! running them over a set of documents, and the scripts folder and
//! extensions. It has no UI dependency; the editor and `pixl-cli` both run
//! scripts through it.
//!
//! A run works on a [`Session`]: copies of the open documents (tiles are
//! copy-on-write, so a copy is cheap), the colors and the parameters. The
//! script edits the session through the functions in [`host::DEFS`], which
//! `pixl.cdl` wraps in handle types, and the host takes back what changed.

pub mod extensions;
pub mod host;
pub mod run;
pub mod session;

pub use extensions::{Extension, Manifest, ScriptFile, install, list_scripts, load_extensions};
pub use run::{Library, Outcome, ScriptError, api_source, run, run_file};
pub use session::{Cancel, ScriptDoc, Session};

/// An example script pixl ships.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Example {
    /// What the Scripts dialog shows.
    pub label: &'static str,
    /// The file it was written as, in `crates/script/scripts/`.
    pub file: &'static str,
    pub source: &'static str,
}

/// The example scripts, which the editor lists beside the user's own.
pub const EXAMPLES: [Example; 3] = [
    Example {
        label: "Export each layer",
        file: "export-each-layer.cdl",
        source: include_str!("../scripts/export-each-layer.cdl"),
    },
    Example {
        label: "Outline all frames",
        file: "outline-all-frames.cdl",
        source: include_str!("../scripts/outline-all-frames.cdl"),
    },
    Example {
        label: "Palette from image",
        file: "palette-from-image.cdl",
        source: include_str!("../scripts/palette-from-image.cdl"),
    },
];
