//! The headless core of the pixl pixel art editor: document model,
//! compositing, history, raster tools, transforms and image I/O. It has no UI
//! dependency; the editor's UI drives it.

pub mod color;
pub mod composite;
pub mod document;
pub mod error;
pub mod geom;
pub mod image;

pub use color::{Color, ColorMode, Palette, Rgba};
pub use composite::{CompositeOptions, OnionSkin, composite, composite_frame};
pub use document::{
    Background, BlendMode, Cel, Document, Frame, ImageId, Layer, LayerId, LayerKind, LayerMask,
    LayerProps, Locks, LoopDirection, NewLayer, PixelAspect, Placement, Slice, Tag, Target,
};
pub use error::Error;
pub use geom::{Point, Rect};
pub use image::{Raster, TILE_SIZE, TiledImage};
