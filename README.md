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
of it. pixl has no native file format yet.

## Quick start

```sh
cargo test
```
