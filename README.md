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
window, with a menu row, the tool bar, tool options, the canvas, and the
color, palette, layers, history, timeline and preview panels.

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
not fit the layer buffer memory set under Performance in the settings. More
buffers make strokes on lower layers cheaper and every frame of the renderer
dearer, so the default holds eight buffers of 4096x4096. Large redraws run on
worker threads and show tile by tile as they finish.

F3 shows the performance overlay over the canvas: frames per second, the
frame time (average, 95th percentile and longest over the last three
seconds) with a graph of it, the time from the last stroke to the screen,
the editor's own work and the compositing in the last frame, the tiles it
wrote, the layer buffers and their memory, the document's tiles and the
undo history in memory, an estimate of the GPU memory, the worker threads
and their queue, and the renderer. Lumen does not tell an app when a frame
reached the screen, so frame times run from one frame's tick to the next.

The tools run on `pixl-core`, and the tool bar shows each as a pixel icon
whose tooltip names the tool and its key:

- Paint: the pencil (pixel-perfect by default), a soft brush (opacity and
  hardness, or the same coverage laid down as a dither of whole pixels), the
  eraser, a spray, a shading brush that steps pixels along the palette, a
  dither brush, the clone stamp (Alt+click picks its source), a pattern
  stamp (any selection makes the pattern), a smudge and a blur that can keep
  to the colors already there, and a brush that replaces the background
  color with the foreground.
- Shapes: line, Bezier curve, rectangle, ellipse, polygon and contour (a
  freehand shape filled on release), filled or outlined; lines and curves
  are pixel-perfect when asked.
- Fill: the bucket, and gradients (linear, radial or conic) that step
  between two colors or along the palette, broken up by a Bayer matrix, blue
  noise or a pattern instead of blending.
- Select: rectangle, ellipse, lasso, polygon lasso and magic wand
  (tolerance, contiguous or not, one layer or all, diagonals or not). Shift
  adds, Alt subtracts, both intersect.
- Other: eyedropper, move, crop, text in bitmap fonts (pixl's own 5x7 font,
  or any BDF font), measure, and slices.

The size slider, the brush shape (round, square, or a custom brush made from
the selection), the stroke stabilizer, pixel-perfect (kept per tool) and
symmetry (mirror left and right, top and bottom, both, or radial with any
number of copies) sit in the tool options bar. X swaps the foreground and
background colors, the secondary button paints with the background color,
and Shift snaps lines to clean pixel steps and shapes to squares and
circles. Every tool keeps to the selection. With a pen, pressure sets the
brush size; the pen's eraser end erases.

The panels on the right show the document: the color panel edits the
foreground color with RGBA sliders and a hex field; the palette sets the
foreground with a click and the background with a right-click; the layers
panel adds, deletes, reorders and hides layers and sets the active layer's
opacity and blend mode; the history panel lists every edit and jumps to any of
them. Ctrl+Z undoes, Ctrl+Shift+Z or Ctrl+Y redoes.

Layers nest in groups: a new layer goes into the active group, a group's
fold button hides what it holds, and a layer dragged onto another row lands
above or below it, or into a group dropped on its middle. Each row shows the
layer's color tag, its visibility, a lock button that locks or unlocks
everything, and marks for clipping and a mask. A mask hides the layer where
it is dark: Add mask starts painting on the mask instead of the pixels, the
mask button in the row switches between the two, and the mask can be turned
off, applied to the pixels or deleted. A clipped layer shows only where the
layer below it does. A reference image (Layer > Reference image) is any
image pixl opens, placed at the top left at half opacity to trace over; it
is never exported. The Layer menu, also opened by a right-click on a row,
duplicates, merges down, flattens, clips, locks pixels, alpha or position,
and opens the layer's properties: name, opacity, blend mode, color tag,
clipping, locks and the mask.

Panels sit in docks left, right and below the canvas. Drag the gutter beside
a dock to resize it. Each panel's header collapses it to its title or hides
it, and a right-click on the header moves it to another dock or up and down
its dock. Window > Panels and layouts lists every panel with its dock, its
order and whether it shows, and switches between layouts: the built-in Paint,
Animate and Rig, and any the user saves under a name of their own. Window >
Reset layout puts the current layout back as it was saved. The arrangement in
use and the saved layouts are kept in `workspace.json` in pixl's data folder,
so the window reopens as it was left.

Window > Customize the tool bar picks which tools the tool bar shows and in
what order, one column or two, and groups related tools on one button: the
button shows the tool used last, and a right-click or a long press on it
opens a flyout of the others. Full puts every tool on a button of its own;
Compact groups the brushes, retouching tools, stamps, lines, shapes, fills,
selections, transforms and guides into one column. The arrangement is kept
in `toolbar.json` in pixl's data folder.

The Brushes panel is a library of named brushes: each keeps a shape (round,
square, or a custom mask), a size, opacity and hardness for the soft brush,
dithering, pixel-perfect, the stroke stabilizer and the pen's pressure
mapping. A click makes one the brush in use. Save keeps the current brush
under the name typed beside it; Selection makes a brush of the selected
pixels; PNG... makes one of an image's opaque pixels; Import... and
Export... share brushes as a `.pixlbrushes` pack. The Presets button at the
end of the tool options keeps a tool's whole set of options under a name and
puts them back with a click, picking the tool too. Beside it, the quick bar
holds the last colors and brushes painted with; a click takes one again.
Brushes, tool presets and the quick bar are kept in pixl's data folder
(`brushes.json`, `tool-presets.json`, `quickbar.json`), and the command
palette lists the brushes and presets as well.

File > Export presets keeps named export settings: the format (PNG frames,
an animated GIF, or a sprite sheet with its Aseprite-shape JSON), the scale,
which frames (all, the current one, one tag, or each tag), which layers (the
visible ones merged, or each layer alone), the sheet's layout (a row, a
column, a grid, or packed, with or without trimming), and the file
to write, relative to the document's folder, where `{name}`, `{tag}`,
`{frame}`, `{layer}` and `{scale}` fill in. File > Export all (Ctrl+Alt+E)
runs every preset marked for it on the open document, in the background, and
the status bar says how many files it wrote. The presets are kept in
`exports.json` in pixl's data folder and serve every document.

Ctrl+Shift+P opens the command palette: one search field over every command
a key can run, every tool, the layouts, the themes and the recent documents.
Typing narrows the list to the entries that have the typed letters in order,
the arrow keys pick one and Enter runs it. Each entry shows its keys, so the
palette also answers "what is the shortcut for this".

The timeline panel, below the canvas by default, lists every layer against
every frame, with the document's tags above the frame numbers.
A cel shows whether it holds pixels and joins its neighbors when they are
linked. Click a cel or a frame number to go there; drag or Shift-click to
select a range of cels or frames. The timeline adds empty frames, copies,
deletes and reverses frames, links the selected cels to the first one or
gives each its own pixels, clears cels, and sets the duration of the selected
frames; drag selected frame numbers to move the frames. Tags are made from
the selected frames and edited in their dialog (name, frames, color,
direction, repeats), or by dragging a tag's ends or middle; a right-click on
a cel, a frame number or a tag opens its menu. `,` and `.` (or Alt+Left and
Alt+Right) step through frames, Alt+Up and Alt+Down through layers.

Play runs the animation at each frame's duration: looping, once or
ping-pong, and inside the current frame's tag, with its direction and
repeats, while Loop tag is on. The frame rate playback reaches shows beside
the controls; a document too heavy to keep up plays slower rather than
skipping frames. The onion skin shows the frames before and after the
current one, as many as asked for, fading with distance, tinted by the
theme, under or over the current frame. The preview panel shows the
document at actual size, or fitted to the panel, and plays along; the Paint
layout hides it and Animate shows it beside the timeline.

The File menu makes a new project (Ctrl+N), opens and saves `.pixl`
documents (Ctrl+O, Ctrl+S, Ctrl+Shift+S), and exports the current frame as a
PNG (Ctrl+E) or the animation as a GIF. Open also reads Aseprite, PNG and GIF
files, as new documents that save as `.pixl`. A new project has a name, a
size (typed, or one click on a square, sprite or screen preset), RGBA or
indexed color, and a transparent or colored background. Replacing a document
with unsaved edits asks first. While
a saved document has unsaved edits, the editor writes its recovery copy beside
it every 30 seconds (the files settings change the interval or turn recovery
copies off), and opening a document whose recovery copy is newer offers to
recover it.

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
icons are pixl's own, drawn in `app/assets/icons/draw.py` as masks the theme
inks, and share pixl's license.

Edit > Settings (Ctrl+,) sets how the view moves (what the wheel does, zoom
speed, zooming around the pointer, scroll and pan key speeds, an inverted
wheel), how the pen paints (pressure on or off, the pressure curve with a
preview, the brush size at the lightest and the firmest touch), how the
canvas looks (checkerboard size and colors, the pixel grid's color and the
zoom it shows from, a new custom grid's cell, the color around the
document), the interface (theme, UI font, UI scale, tooltips), files
(autosave interval, recovery copies, the length of the recent list),
performance (the progressive redraw threshold, worker threads, the layer
buffer memory, the performance overlay) and every keyboard shortcut. Each
change applies at once. Shortcuts start from pixl's own keys or Aseprite's
or Photoshop's; a key already in use asks before it moves, and the menus show
the keys in force. The settings are kept in `settings.json` in pixl's data
folder, beside the recent list. UI scale grows the text and the sizes the
theme keeps as tokens; a few fixed sizes (list rows, swatches) stay as they
are.

The editor builds against Lumen 0.0.10, which is not on crates.io yet. Until it
is, building `pixl` needs `lumenui` and `lumenui-canvas` patched in from a Lumen
checkout, and CI fails on the app crate. A release build reads its
stylesheets, font, icons and `lumen.toml` from the directory it starts in, so
run it from `app/`: `cd app && ../target/release/pixl`.

A skeleton layer rigs a character so it can be posed instead of redrawn:
bones in a tree, with raster layers (or selected regions of them) bound to
one bone each as rigid parts. Rendering a pose draws every part at its
bone's place with RotSprite, so turned parts keep clean one-pixel outlines,
into frames of ordinary pixels to touch up.

The Rig panel works on the document's skeleton (New skeleton makes one).
In Bones mode a drag on the canvas draws a bone, starting from the selected
bone's end to chain one to it; dragging a joint or a binding's pivot moves
it, and Shift moves the selected bone's end. The panel lists the bones,
renames and reparents the selected one, and binds the active layer, or the
selected pixels of it, to it. The rig reads its parts from its rest frame,
which "Use this frame" sets. In Pose mode a drag turns a bone, its head
moves it, and with IK on the end of a bone bends it together with its
parent; Shift snaps turns to 15 degrees. Posing a frame other than the rest
frame draws the pose into that frame as it changes and keys it there; on
the rest frame only the bones move. Poses save to a library with
thumbnails, mirror left to right by bone name (`arm.L` and `arm.R`,
`leg_l` and `leg_r`), and reset to rest. A frame's key has an easing toward
the next key. Render to new frame adds a frame holding the pose; Render
keyed fills a range of frames from the keys, eased; Regenerate renders
every generated frame again after the rig changes. Bones are drawn over the
art in the theme's colors and are never part of it. Escape leaves either
mode, and picking a tool hands the canvas back to it. Outside the Rig layout
(Window menu) the panel shows only its header until a mode starts.

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

File > Export sprite sheet (Ctrl+Shift+E) lays the frames out on one PNG: in
a horizontal or vertical strip, a grid filled by rows or by columns, or
packed into as small an image as they fit. It takes every frame, a range
or a tag; padding between images and a border around the sheet; a whole
number scale; trimming each image to its pixels; and one image per layer
instead of per frame. Beside the PNG it writes the JSON Aseprite's export
writes, with the frames as a hash or an array, the tags, layers and slices,
so engines that read Aseprite sheets read these. The dialog previews the
sheet as its options change. File > Import sprite sheet (Ctrl+Shift+I) splits
a PNG into the frames of a new document, by cell size or by a count of
columns and rows, with an offset and padding, leaving out empty cells if
asked, and previews the cells over the image.

## Quick start

```sh
cargo run -p pixl                 # the start screen
cargo run -p pixl -- art.pixl     # straight into a document
cargo test
```

`cargo bench -p pixl-core --bench rig` times posing and rendering a
20-bone character, and `cargo bench -p pixl --bench pose -- gpu` (or `cpu`)
times a pose drag to the frame that shows it (`side=4096 layers=32` puts the
character in the view bench's document).

`pixl --perf-log frames.csv` writes one row per frame to a CSV file: the
frame's start, its time, the editor's work and compositing, the tiles
written, the layer buffers and their memory, the worker queue and the time
to the screen of the last stroke.

### Performance bench

```sh
cargo bench -p pixl --bench view -- gpu      # or cpu
cargo bench -p pixl --bench view -- gpu budget=2048 log=frames.csv
```

The view bench opens a 4096x4096 document of 32 layers with no window and
measures what a user waits for: the progressive redraw after opening it,
the time from a stroke to the frame that shows it on the top layer and on
the bottom one, the frame after a pan and after a zoom step, hiding a
layer, and exporting the frame as a PNG. `side=N` and `layers=N` change the
document, `budget=MiB` the layer buffer memory, and `log=FILE` writes the
per-frame CSV the `--perf-log` flag writes. Numbers move with the load on
the machine, so compare runs taken under the same load.

`cargo bench -p pixl --bench customize -- gpu` (or `cpu`) times, on the
same document, what the workspace, themes, command palette and export
presets cost each frame with a layout and a user theme in force, beside the
time from one frame to the next. They are meant to cost nothing a frame
would show.

On Linux the build links GTK 3, X11, xkbcommon and Wayland, so their
development packages must be installed.

## License

MIT, see [LICENSE](LICENSE).
