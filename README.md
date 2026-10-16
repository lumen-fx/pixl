# pixl

A pixel art editor for animation, tile work and sprite posing, built on Lumen.

## Why

pixl covers what an artist uses Aseprite and Photoshop for in one tool: layers,
frames and a timeline, tools that behave correctly at the pixel grid, and
pixel-art-aware transforms. It is also the flagship showcase for the Lumen
Rust SDK.

## Status

Early. `pixl-core` (in `core/`) holds the editor core with no UI: the document
model, compositing, undo history, raster tools, transforms, and PNG and sprite
sheet I/O (sheets carry Aseprite-compatible JSON). The editor itself is the
`pixl` program in `app/`, built with the Lumen Rust SDK: one document per
window, with a menu row, the tool bar, tool options, the canvas, and the color,
palette, layers and history panels.

The canvas view zooms from 1 to 6400 percent around the pointer (the wheel,
or `+`, `-`, `0` to fit and `1` for 100 percent) and pans with the middle
button or a drag with the space bar held. Pixels stay sharp at every zoom, a
grid outlines them from 800 percent, and a checkerboard shows through
transparency. Each layer is its own pixel buffer that Lumen's renderer
composites with the layer's blend mode and opacity, on the GPU when the app
renders there; what the renderer cannot composite the way `pixl-core` does
(add and subtract, clipping, masks, groups) is composited by `pixl-core` on the
CPU instead, and so are the bottom layers of a document whose layers would
not fit the renderer's image budget. Large redraws run on worker threads and
show tile by tile as they finish.

The tools run on `pixl-core`: the pencil (pixel-perfect by default), eraser,
line, rectangle, ellipse, bucket, eyedropper, rectangle selection and move.
Their keys are B, E, L, U (Shift+U for the ellipse), G, I, M and V; X swaps the
foreground and background colors, the secondary button paints with the
background color, and Shift snaps lines to clean pixel steps and shapes to
squares and circles. Every tool keeps to the selection. With a pen, pressure
sets the brush size; the pen's eraser end erases.

The editor has its own theme, written entirely in CSS over its own tokens: a
dark set of neutral greys so the colors in the art read true, and a light set
built from the same tokens. The UI font is Pixelify Sans by Stefie Justprince,
under the SIL Open Font License (`app/assets/fonts/OFL.txt`); a setting in the
menu row switches the UI to the platform's sans-serif instead.

The editor builds against Lumen 0.0.10, which is not on crates.io yet. Until it
is, building `pixl` needs `lumenui` and `lumenui-canvas` patched in from a Lumen
checkout, and CI fails on the app crate.

Documents save as `.pixl` files: one versioned, chunked, compressed file that
stores each distinct tile once. Saves are atomic, and autosave keeps a
recovery copy beside the document. A file from a newer pixl opens in an older
one when the newer data is optional; what the older version cannot read is
skipped with a warning.

Aseprite `.ase` and `.aseprite` files open and save as a first-class format:
layers and groups, linked cels, RGBA and indexed color, the palette, tags and
slices carry over. What one side cannot hold (pixl's layer masks and clipping,
Aseprite's tilemaps) is listed as a warning instead of dropped silently.

Animations export as GIF with each frame's duration. One palette covers the
whole animation; past 256 colors it is reduced by median cut, without
dithering. GIFs import with their frames, timing and disposal applied.

## Quick start

```sh
cargo run -p pixl
cargo test
```

On Linux the build links GTK 3, X11, xkbcommon and Wayland, so their
development packages must be installed.

## License

MIT, see [LICENSE](LICENSE).
