//! pixl documents in other programs' formats: Aseprite files, GIF
//! animations, PNG images and sprite sheets with their JSON, and the export
//! presets the editor and `pixl-cli` both run. The native `.pixl` format is
//! in `pixl-document`.

pub mod aseprite;
pub mod export;
pub mod gif;
pub mod open;
pub mod png;
pub mod sheet;
