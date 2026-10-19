//! The clipboard: copied pixels go to the system clipboard as an image, so
//! other programs can paste them, and pixl keeps its own copy beside it with
//! what the image cannot say: where the pixels came from on the canvas and,
//! for an indexed document, their palette indices. A paste takes pixl's own
//! copy while the system clipboard still holds the image it wrote, and the
//! system clipboard's image once another program has put one there.
//!
//! [`Pasteboard`] chooses the clipboard: the system one for the `pixl`
//! program, or one in memory, which the editor the tests run starts with,
//! so a test never reads or writes the desktop's clipboard.

use lumenui::ecs::system::SystemParam;
use lumenui::prelude::*;
use pixl_document::{ColorMode, Palette, Pixels, Point, Rgba};

/// The pixels one copy took.
#[derive(Clone, Debug, PartialEq)]
pub struct Copied {
    /// Raw values: packed RGBA, or palette indices when `palette` is set.
    pub pixels: Pixels,
    /// The palette and transparent index of indexed pixels.
    pub palette: Option<(Palette, u8)>,
    /// Where the pixels' top-left sat on the canvas.
    pub at: Point,
    /// The pixels as the system clipboard got them.
    pub image: RgbaImage,
}

impl Copied {
    /// A copy of `pixels`, raw values of a document in `mode` with
    /// `palette` and transparent index `ti`.
    pub fn new(pixels: Pixels, mode: ColorMode, palette: &Palette, ti: u8, at: Point) -> Self {
        let palette = (mode == ColorMode::Indexed).then(|| (palette.clone(), ti));
        let image = image_of(&pixels, palette.as_ref());
        Self {
            pixels,
            palette,
            at,
            image,
        }
    }

    /// The pixels as raw values for a document in `mode` with `palette`
    /// and transparent index `ti`. Indexed pixels keep their indices on
    /// the same palette; anything else lands on the palette the way an
    /// imported image does.
    pub fn values_for(&self, mode: ColorMode, palette: &Palette, ti: u8) -> Pixels {
        match (mode, &self.palette) {
            (ColorMode::Indexed, Some((p, t))) if p == palette && *t == ti => self.pixels.clone(),
            (ColorMode::Rgba, None) => self.pixels.clone(),
            _ => values_of(&self.image, mode, palette, ti),
        }
    }
}

/// `pixels` as RGBA8 bytes: indices through `palette`, its transparent
/// index clear.
pub fn image_of(pixels: &Pixels, palette: Option<&(Palette, u8)>) -> RgbaImage {
    let mut bytes = Vec::with_capacity(pixels.data.len() * 4);
    for &v in &pixels.data {
        let c = match palette {
            Some((_, ti)) if v == u32::from(*ti) => Rgba::TRANSPARENT,
            Some((p, _)) => p.get(v as u8),
            None => Rgba::from_u32(v),
        };
        bytes.extend_from_slice(&[c.r, c.g, c.b, c.a]);
    }
    RgbaImage::new(pixels.width, pixels.height, bytes).expect("four bytes a pixel")
}

/// An RGBA image as raw values for a document in `mode`: on an indexed
/// document, each color goes to its palette entry the way an imported
/// image's does.
pub fn values_of(image: &RgbaImage, mode: ColorMode, palette: &Palette, ti: u8) -> Pixels {
    let data = image
        .pixels()
        .chunks_exact(4)
        .map(|c| {
            let c = Rgba::new(c[0], c[1], c[2], c[3]);
            match mode {
                ColorMode::Rgba if c.a == 0 => 0,
                ColorMode::Rgba => c.to_u32(),
                ColorMode::Indexed => u32::from(palette.import_index(c, ti)),
            }
        })
        .collect();
    Pixels {
        width: image.width(),
        height: image.height(),
        data,
    }
}

/// Whether `pasted` is the image pixl wrote for `copied`, as the system
/// clipboard gives it back. A platform may round the colors of
/// see-through pixels on the way, so they match within a step or two, and
/// clear pixels match whatever color they carry.
fn same_image(copied: &RgbaImage, pasted: &RgbaImage) -> bool {
    copied.width() == pasted.width()
        && copied.height() == pasted.height()
        && copied
            .pixels()
            .chunks_exact(4)
            .zip(pasted.pixels().chunks_exact(4))
            .all(|(a, b)| {
                (a[3] == 0 && b[3] == 0) || a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 2)
            })
}

/// Where the clipboard lives.
enum Board {
    /// The system clipboard, through Lumen.
    System,
    /// A clipboard in memory: the text and the image it holds.
    Memory {
        text: String,
        image: Option<RgbaImage>,
    },
}

/// The clipboard pixl writes to and reads from, and pixl's own copy of what
/// it last copied.
#[derive(Resource)]
pub struct Pasteboard {
    board: Board,
    copied: Option<Copied>,
}

impl Pasteboard {
    /// The system clipboard, as the `pixl` program uses.
    pub fn system() -> Self {
        Self {
            board: Board::System,
            copied: None,
        }
    }

    /// A clipboard in memory, empty.
    pub fn in_memory() -> Self {
        Self {
            board: Board::Memory {
                text: String::new(),
                image: None,
            },
            copied: None,
        }
    }

    /// A clipboard in memory holding `image`, as if another program had
    /// copied it.
    pub fn holding(image: RgbaImage) -> Self {
        Self {
            board: Board::Memory {
                text: String::new(),
                image: Some(image),
            },
            copied: None,
        }
    }

    /// The image an in-memory clipboard holds; `None` for the system one.
    pub fn memory_image(&self) -> Option<&RgbaImage> {
        match &self.board {
            Board::Memory { image, .. } => image.as_ref(),
            Board::System => None,
        }
    }

    /// The text an in-memory clipboard holds; `None` for the system one.
    pub fn memory_text(&self) -> Option<&str> {
        match &self.board {
            Board::Memory { text, .. } => Some(text),
            Board::System => None,
        }
    }
}

/// The clipboard for a system to take: the board and, for the system
/// board, Lumen's clipboard (which keeps the system on the main thread).
#[derive(SystemParam)]
pub struct ClipboardAccess<'w> {
    board: ResMut<'w, Pasteboard>,
    os: Clipboard<'w>,
}

impl ClipboardAccess<'_> {
    /// Puts `text` on the clipboard.
    pub fn set_text(&mut self, text: &str) {
        match &mut self.board.board {
            Board::System => {
                self.os.set_text(text);
            }
            Board::Memory { text: held, image } => {
                *held = text.to_string();
                *image = None;
            }
        }
    }

    /// Keeps `copied` as pixl's own copy and puts its image on the
    /// clipboard.
    pub fn copy(&mut self, copied: Copied) {
        match &mut self.board.board {
            Board::System => {
                self.os.set_image(&copied.image);
            }
            Board::Memory { text, image } => {
                text.clear();
                *image = Some(copied.image.clone());
            }
        }
        self.board.copied = Some(copied);
    }

    /// What a paste takes: pixl's own copy while the clipboard holds the
    /// image it wrote (or holds no image, or there is no system clipboard),
    /// and otherwise the clipboard's image, with no place on the canvas.
    pub fn paste(&self) -> Option<Pasted> {
        let image = match &self.board.board {
            Board::System => self.os.image(),
            Board::Memory { image, .. } => image.clone(),
        };
        match (image, &self.board.copied) {
            (Some(image), Some(c)) if !same_image(&c.image, &image) => Some(Pasted::Image(image)),
            (_, Some(c)) => Some(Pasted::Copied(c.clone())),
            (Some(image), None) => Some(Pasted::Image(image)),
            (None, None) => None,
        }
    }
}

/// What a paste brings.
pub enum Pasted {
    /// pixl's own copy: exact values and a place on the canvas.
    Copied(Copied),
    /// An image another program put on the clipboard.
    Image(RgbaImage),
}

impl Pasted {
    /// The pixels as raw values for a document in `mode`.
    pub fn values_for(&self, mode: ColorMode, palette: &Palette, ti: u8) -> Pixels {
        match self {
            Pasted::Copied(c) => c.values_for(mode, palette, ti),
            Pasted::Image(i) => values_of(i, mode, palette, ti),
        }
    }

    /// Where the pixels came from on the canvas, if pixl copied them.
    pub fn origin(&self) -> Option<Point> {
        match self {
            Pasted::Copied(c) => Some(c.at),
            Pasted::Image(_) => None,
        }
    }

    /// The width and height.
    pub fn size(&self) -> (u32, u32) {
        match self {
            Pasted::Copied(c) => (c.pixels.width, c.pixels.height),
            Pasted::Image(i) => (i.width(), i.height()),
        }
    }

    /// The palette of indexed pixels pixl copied.
    pub fn palette(&self) -> Option<&(Palette, u8)> {
        match self {
            Pasted::Copied(c) => c.palette.as_ref(),
            Pasted::Image(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexed_copy() -> Copied {
        let palette = Palette::new(vec![
            Rgba::TRANSPARENT,
            Rgba::opaque(255, 0, 0),
            Rgba::opaque(0, 0, 255),
        ])
        .unwrap();
        let pixels = Pixels {
            width: 2,
            height: 1,
            data: vec![0, 2],
        };
        Copied::new(pixels, ColorMode::Indexed, &palette, 0, Point::new(3, 4))
    }

    #[test]
    fn indexed_pixels_keep_their_indices_on_their_own_palette() {
        let c = indexed_copy();
        assert_eq!(c.image.pixels(), &[0, 0, 0, 0, 0, 0, 255, 255]);
        let (p, ti) = c.palette.clone().unwrap();
        assert_eq!(c.values_for(ColorMode::Indexed, &p, ti).data, vec![0, 2]);
    }

    #[test]
    fn indexed_pixels_find_their_colors_on_another_palette() {
        let c = indexed_copy();
        let other = Palette::new(vec![Rgba::TRANSPARENT, Rgba::opaque(0, 0, 250)]).unwrap();
        assert_eq!(c.values_for(ColorMode::Indexed, &other, 0).data, vec![0, 1]);
        let rgba = c.values_for(ColorMode::Rgba, &other, 0).data;
        assert_eq!(rgba, vec![0, Rgba::opaque(0, 0, 255).to_u32()]);
    }

    #[test]
    fn a_round_trip_that_rounds_see_through_pixels_is_still_pixls_copy() {
        let a = RgbaImage::new(2, 1, vec![10, 20, 30, 128, 1, 2, 3, 0]).unwrap();
        let b = RgbaImage::new(2, 1, vec![11, 19, 30, 128, 9, 9, 9, 0]).unwrap();
        let c = RgbaImage::new(2, 1, vec![40, 20, 30, 128, 0, 0, 0, 0]).unwrap();
        assert!(same_image(&a, &b));
        assert!(!same_image(&a, &c));
    }
}
