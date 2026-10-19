//! Reading a document from any file pixl opens: `.pixl` natively, and
//! `.aseprite` / `.ase`, `.png` and `.gif` through their importers.

use std::path::Path;

use pixl_document::document::Document;
use pixl_document::native;

use crate::aseprite::import_aseprite;
use crate::gif::import_gif;
use crate::png::import_png;

/// The extensions pixl opens through an importer rather than natively.
pub const IMPORTED: [&str; 4] = ["png", "gif", "aseprite", "ase"];

/// A document read from a file, with what the reader could not use.
pub struct Opened {
    pub doc: Document,
    pub warnings: Vec<String>,
}

/// Whether `path` is a file pixl imports rather than opens as its own.
pub fn is_imported(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| IMPORTED.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Reads a document: `.pixl` natively, `.aseprite` / `.ase`, `.png` and
/// `.gif` through their importers.
pub fn read_document(path: &Path) -> Result<Opened, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let fail = |e: pixl_document::Error| format!("could not open {}: {e}", path.display());
    let read =
        || std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()));
    match ext.as_str() {
        "png" => Ok(Opened {
            doc: import_png(&read()?).map_err(fail)?,
            warnings: Vec::new(),
        }),
        "gif" => Ok(Opened {
            doc: import_gif(&read()?).map_err(fail)?,
            warnings: Vec::new(),
        }),
        "aseprite" | "ase" => {
            let import = import_aseprite(&read()?).map_err(fail)?;
            Ok(Opened {
                doc: import.document,
                warnings: import.warnings.iter().map(ToString::to_string).collect(),
            })
        }
        _ => {
            let loaded = native::load_file(path).map_err(fail)?;
            Ok(Opened {
                doc: loaded.document,
                warnings: loaded.warnings.iter().map(ToString::to_string).collect(),
            })
        }
    }
}
