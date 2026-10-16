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
