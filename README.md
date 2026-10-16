# pixl

A pixel art editor for animation, tile work and sprite posing, built on Lumen.

## Why

pixl covers what an artist uses Aseprite and Photoshop for in one tool: layers,
frames and a timeline, tools that behave correctly at the pixel grid, and
pixel-art-aware transforms. It is also the flagship showcase for the Lumen
Rust SDK.

## Status

Early. `pixl-core` holds the editor core with no UI: the document model,
compositing, undo history, raster tools, transforms, and PNG and sprite sheet
I/O (sheets carry Aseprite-compatible JSON). The editor UI comes next, on top
of it.

Documents save as `.pixl` files: one versioned, chunked, compressed file that
stores each distinct tile once. Saves are atomic, and autosave keeps a
recovery copy beside the document. A file from a newer pixl opens in an older
one when the newer data is optional; what the older version cannot read is
skipped with a warning.

Aseprite `.ase` and `.aseprite` files open and save as a first-class format:
layers and groups, linked cels, RGBA and indexed color, the palette, tags and
slices carry over. What one side cannot hold (pixl's layer masks and clipping,
Aseprite's tilemaps) is listed as a warning instead of dropped silently.

## Quick start

```sh
cargo test
```
