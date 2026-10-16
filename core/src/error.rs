//! The error type of every fallible core operation.

use std::fmt;

use crate::document::LayerId;

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
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}
