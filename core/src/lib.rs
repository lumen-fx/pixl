//! The headless core of the pixl pixel art editor: document model,
//! compositing, history, raster tools, transforms, the native `.pixl` file
//! format and image I/O. It has no UI dependency; the editor's UI drives it.

pub mod aseprite;
mod bytes;
pub mod color;
pub mod composite;
pub mod document;
pub mod error;
pub mod geom;
pub mod gif;
pub mod history;
pub mod image;
pub mod io;
pub mod merge;
pub mod native;
pub mod selection;
pub mod tools;
pub mod transform;

pub use color::{Color, ColorMode, Palette, Rgba};
pub use composite::{CompositeOptions, OnionSkin, composite, composite_frame, composite_ghosts};
pub use document::{
    Background, BlendMode, Cel, Document, Frame, ImageId, Layer, LayerId, LayerKind, LayerMask,
    LayerProps, Locks, LoopDirection, NewLayer, PixelAspect, Placement, Slice, Tag, Target,
};
pub use error::Error;
pub use geom::{Point, Rect};
pub use history::{History, Scope, Transaction};
pub use image::{Raster, TILE_SIZE, TiledImage};
pub use selection::{SelectOp, Selection};
pub use tools::{
    Bayer, Brush, BrushShape, Canvas, Ink, Symmetry, SymmetryMode, ToolContext, edit_cel,
};
pub use transform::{CanvasTransform, CelTransform, Pixels, transform_canvas, transform_cel};
