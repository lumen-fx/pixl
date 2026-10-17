//! The headless core of the pixl pixel art editor: document model,
//! compositing, history, raster tools, transforms, skeletons and
//! pose-to-frame, the native `.pixl` file format and image I/O. It has no UI
//! dependency; the editor's UI drives it.

pub mod aseprite;
mod bytes;
pub mod color;
pub mod composite;
pub mod document;
pub mod error;
pub mod filters;
pub mod geom;
pub mod gif;
pub mod history;
pub mod image;
pub mod io;
pub mod merge;
pub mod native;
pub mod palettes;
pub mod rig;
pub mod selection;
pub mod sheet;
pub mod tools;
pub mod transform;

pub use color::{Color, ColorMode, Hsl, Palette, Rgba};
pub use composite::{CompositeOptions, OnionSkin, composite, composite_frame, composite_ghosts};
pub use document::{
    Background, BlendMode, Cel, Document, Frame, ImageId, Layer, LayerId, LayerKind, LayerMask,
    LayerProps, Locks, LoopDirection, NewLayer, PixelAspect, Placement, Slice, Tag, Target,
};
pub use error::Error;
pub use geom::{Point, Rect};
pub use history::{History, Scope, Transaction};
pub use image::{Raster, TILE_SIZE, TiledImage};
pub use rig::{
    Bend, Binding, Bone, BoneId, BonePose, Easing, NamedPose, Parts, Pose, PoseKey, PosedLayer,
    Region, RenderOptions, Rig, RigFrame, Rigid, Vec2,
};
pub use selection::{SelectOp, Selection};
pub use tools::{
    Bayer, Brush, BrushShape, Canvas, Ink, Symmetry, SymmetryMode, ToolContext, edit_cel, edit_mask,
};
pub use transform::{
    Affine, CanvasTransform, CelTransform, Pixels, Resample, crop, resize_canvas, transform_canvas,
    transform_cel,
};
