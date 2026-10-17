# pixl

A pixel art editor for animation, tile work and sprite posing, built on Lumen.

## Why

pixl covers what an artist uses Aseprite and Photoshop for in one tool: layers,
frames and a timeline, tools that behave correctly at the pixel grid, and
pixel-art-aware transforms. It is also the flagship showcase for the Lumen
Rust SDK.

## Status

Early. `pixl-core` (in `core/`) holds the editor core with no UI: the document
model, compositing, undo history, raster tools, adjustments and filters,
palettes and their files, transforms, and PNG and sprite sheet I/O (sheets
carry Aseprite-compatible JSON). The editor itself is the
`pixl` program in `app/`, built with the Lumen Rust SDK: one document per
window, with a menu row, the tool bar, tool options, the canvas, and the color,
palette, layers and history panels.

Started without a file, pixl opens on its start screen: the documents opened
or saved most recently, each with a thumbnail of its first frame, its folder,
its size and when it last changed, with a field that filters them by name or
folder. A click opens one. A document that is no longer there shows as
missing, with a button that takes it off the list; one with a recovery copy
newer than its file offers to recover it. Beside the list are New project
and Open. The list and the thumbnails are kept in pixl's folder of the user
data directory (`~/.local/share/lumen/pixl` on Linux), never beside the
documents. File > Close (Ctrl+W) and the Home button in the menu row return
to the start screen; Home keeps the open document to go back to.

The canvas view zooms from 1 to 6400 percent around the pointer (the wheel,
or `+`, `-`, `0` to fit and `1` for 100 percent) and pans with the middle
button, a drag with the space bar held, or W, A, S and D held down (Shift
moves faster). Pixels stay sharp at every zoom, a
grid outlines them from 800 percent, and a checkerboard shows through
transparency. Each layer is its own pixel buffer that Lumen's renderer
composites with the layer's blend mode and opacity, on the GPU when the app
renders there; what the renderer cannot composite the way `pixl-core` does
(add and subtract, clipping, masks, groups) is composited by `pixl-core` on the
CPU instead, and so are the bottom layers of a document whose layers would
not fit the renderer's image budget. Large redraws run on worker threads and
show tile by tile as they finish.

The tools run on `pixl-core`: the pencil (pixel-perfect by default), eraser,
line, rectangle, ellipse, bucket, eyedropper, rectangle selection and move. The
tool bar shows each as a pixel icon, and its tooltip names the tool and its
key.
Their keys are B, E, L, U (Shift+U for the ellipse), G, I, M and V; X swaps the
foreground and background colors, the secondary button paints with the
background color, and Shift snaps lines to clean pixel steps and shapes to
squares and circles. Every tool keeps to the selection. With a pen, pressure
sets the brush size; the pen's eraser end erases.

The panels on the right show the document: the color panel edits the
foreground color with RGBA sliders and a hex field; the palette sets the
foreground with a click and the background with a right-click; the layers
panel adds, deletes, reorders and hides layers and sets the active layer's
opacity and blend mode; the history panel lists every edit and jumps to any of
them. Ctrl+Z undoes, Ctrl+Shift+Z or Ctrl+Y redoes.

Panels sit in docks left, right and below the canvas. Drag the gutter beside
a dock to resize it. Each panel's header collapses it to its title or hides
it, and a right-click on the header moves it to another dock or up and down
its dock. Window > Panels and layouts lists every panel with its dock, its
order and whether it shows, and switches between layouts: the built-in Paint,
Animate and Rig, and any the user saves under a name of their own. Window >
Reset layout puts the current layout back as it was saved. The arrangement in
use and the saved layouts are kept in `workspace.json` in pixl's data folder,
so the window reopens as it was left.

The File menu makes a new project (Ctrl+N), opens and saves `.pixl`
documents (Ctrl+O, Ctrl+S, Ctrl+Shift+S), and exports the current frame as a
PNG (Ctrl+E) or the animation as a GIF. Open also reads Aseprite, PNG and GIF
files, as new documents that save as `.pixl`. A new project has a name, a
size (typed, or one click on a square, sprite or screen preset), RGBA or
indexed color, and a transparent or colored background. Replacing a document
with unsaved edits asks first. While
a saved document has unsaved edits, the editor writes its recovery copy beside
it every 30 seconds, and opening a document whose recovery copy is newer
offers to recover it.

The editor has its own theme, written entirely in CSS over its own tokens: a
dark set of neutral greys so the colors in the art read true, and light, high
contrast and retro sets built from the same tokens. Window > Themes picks a
set and edits its color tokens (surfaces, text, accent, canvas background,
checkerboard, grid and selection colors) with a preview beside them, along
with the UI font and scale, and saves the result as a theme of the user's
own. A theme is a `.pixltheme` file of plain CSS custom properties, kept in
the `themes` folder of pixl's data folder; Import and Export share one. The
editor's preview follows each edit; the rest of the window takes a user
theme's colors once Lumen applies custom properties set from code
(lumen-fx/lumen#531). The UI font is Pixelify Sans by Stefie Justprince,
under the SIL Open Font License (`app/assets/fonts/OFL.txt`); a setting in the
menu row switches the UI to the platform's sans-serif instead. The tool
icons are pixl's own, drawn in `app/assets/icons/draw.py` in the theme's ink
colors, and share pixl's license.

pixl keeps its settings in `settings.json` in its data folder, beside the
recent list, and remembers the theme and font switches there.

The editor builds against Lumen 0.0.10, which is not on crates.io yet. Until it
is, building `pixl` needs `lumenui` and `lumenui-canvas` patched in from a Lumen
checkout, and CI fails on the app crate. A release build reads its
stylesheets, font, icons and `lumen.toml` from the directory it starts in, so
run it from `app/`: `cd app && ../target/release/pixl`.

A skeleton layer rigs a character so it can be posed instead of redrawn:
bones in a tree, with raster layers (or selected regions of them) bound to
one bone each as rigid parts. A pose turns and moves bones, with two-bone IK
for limbs; poses save to a library and mirror left to right by bone name
(`arm.L` and `arm.R`, `leg_l` and `leg_r`). Rendering a pose draws every part
at its bone's place with RotSprite, so turned parts keep clean one-pixel
outlines, into a new frame of ordinary pixels to touch up. Poses keyed on
frames fill the frames between them, eased, and generated frames are marked
so they can be rendered again after the rig changes.

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
cargo run -p pixl                 # the start screen
cargo run -p pixl -- art.pixl     # straight into a document
cargo test
```

`cargo bench -p pixl-core --bench rig` times posing and rendering a
20-bone character.

`cargo bench -p pixl --bench view -- gpu` (or `cpu`) measures the canvas
view on a 4096x4096 document of 32 layers: the progressive redraws, the time
from a stroke to the frame that shows it, and the frame after a pan.

On Linux the build links GTK 3, X11, xkbcommon and Wayland, so their
development packages must be installed.

## License

MIT, see [LICENSE](LICENSE).
