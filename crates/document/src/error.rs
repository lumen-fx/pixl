//! The error type of every fallible core operation.

use std::fmt;

use crate::document::LayerId;
use crate::rig::BoneId;
use crate::tiles::TilesetId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No layer has this id.
    LayerNotFound(LayerId),
    /// The operation needs a raster layer.
    NotRaster(LayerId),
    /// The operation needs a group layer.
    NotGroup(LayerId),
    /// The layer's lock forbids the operation.
    Locked(LayerId),
    /// The frame index is past the last frame.
    FrameOutOfRange(usize),
    /// A document needs at least one frame.
    LastFrame,
    /// A layer cannot move into itself or one of its children.
    CyclicMove(LayerId),
    /// The palette already holds 256 colors.
    PaletteFull,
    /// The palette has no entry at this index.
    PaletteIndex(u8),
    /// Width or height is zero or too large.
    InvalidSize { width: u32, height: u32 },
    /// The image does not match the document's color mode.
    ColorModeMismatch,
    /// An image could not be decoded or encoded.
    Image(String),
    /// A document file is truncated, corrupt or of an unsupported version.
    Format(String),
    /// Reading or writing a file failed.
    Io(String),
    /// The operation needs a skeleton layer.
    NotSkeleton(LayerId),
    /// The rig has no bone with this id.
    BoneNotFound(BoneId),
    /// A bone cannot move under itself or one of its children.
    CyclicBone(BoneId),
    /// IK needs the bone to have a parent bone.
    NoIkChain(BoneId),
    /// The rig has no binding at this index.
    BindingNotFound(usize),
    /// A pose cannot render over the rig's source frame.
    SourceFrame(usize),
    /// The operation needs a reference layer.
    NotReference(LayerId),
    /// A reference placement has a zero or non-finite scale, or a
    /// non-finite position or angle.
    InvalidPlace,
    /// The operation needs a tilemap layer.
    NotTilemap(LayerId),
    /// The document has no tileset with this id.
    TilesetNotFound(TilesetId),
    /// The tileset has no tile with this index (or it is the empty tile 0,
    /// which cannot be edited, moved or removed).
    TileNotFound(u32),
    /// A tile width or height is zero or too large.
    InvalidTileSize { width: u32, height: u32 },
    /// A tileset already holds as many tiles as a tile reference can name.
    TooManyTiles,
    /// A tileset still has tilemap layers drawing from it.
    TilesetInUse(TilesetId),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::LayerNotFound(id) => write!(f, "no layer with id {}", id.0),
            Error::NotRaster(id) => write!(f, "layer {} is not a raster layer", id.0),
            Error::NotGroup(id) => write!(f, "layer {} is not a group", id.0),
            Error::Locked(id) => write!(f, "layer {} is locked", id.0),
            Error::FrameOutOfRange(i) => write!(f, "frame {i} does not exist"),
            Error::LastFrame => f.write_str("a document needs at least one frame"),
            Error::CyclicMove(id) => write!(f, "layer {} cannot move into itself", id.0),
            Error::PaletteFull => f.write_str("the palette already holds 256 colors"),
            Error::PaletteIndex(i) => write!(f, "the palette has no entry {i}"),
            Error::InvalidSize { width, height } => write!(f, "invalid size {width}x{height}"),
            Error::ColorModeMismatch => f.write_str("the image does not match the color mode"),
            Error::Image(msg) => write!(f, "image error: {msg}"),
            Error::Format(msg) => write!(f, "file error: {msg}"),
            Error::Io(msg) => write!(f, "i/o error: {msg}"),
            Error::NotSkeleton(id) => write!(f, "layer {} is not a skeleton layer", id.0),
            Error::NotReference(id) => write!(f, "layer {} is not a reference layer", id.0),
            Error::InvalidPlace => f.write_str("a reference needs a finite place and a scale"),
            Error::NotTilemap(id) => write!(f, "layer {} is not a tilemap layer", id.0),
            Error::TilesetNotFound(id) => write!(f, "no tileset with id {}", id.0),
            Error::TileNotFound(i) => write!(f, "the tileset has no tile {i} to change"),
            Error::InvalidTileSize { width, height } => {
                write!(f, "invalid tile size {width}x{height}")
            }
            Error::TooManyTiles => f.write_str("the tileset is full"),
            Error::TilesetInUse(id) => write!(f, "tileset {} is still in use", id.0),
            Error::BoneNotFound(id) => write!(f, "no bone with id {}", id.0),
            Error::CyclicBone(id) => write!(f, "bone {} cannot move under itself", id.0),
            Error::NoIkChain(id) => write!(f, "bone {} has no parent to bend with", id.0),
            Error::BindingNotFound(i) => write!(f, "no binding {i}"),
            Error::SourceFrame(i) => {
                write!(
                    f,
                    "frame {i} holds the rig's rest art and cannot be rendered over"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}
