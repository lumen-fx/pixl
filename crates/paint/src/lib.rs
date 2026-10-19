//! Raster tools and filters over a `pixl-document` document: the pencil,
//! brushes and inks, fills, gradients, shapes and text, symmetry, and the
//! adjustment and effect filters. Each edits a cel's pixels through a
//! [`Canvas`] that honors the selection and the locks.

pub mod filters;
pub mod tools;

pub use tools::{
    Brush, BrushShape, Canvas, Ink, Symmetry, SymmetryMode, ToolContext, edit_cel, edit_mask,
};
