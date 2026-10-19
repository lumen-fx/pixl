# pixl

A pixel art editor for animation, tile work and sprite posing, built on Lumen.

## Why

pixl covers what an artist uses Aseprite and Photoshop for in one tool: layers,
frames and a timeline, tools that behave correctly at the pixel grid, and
pixel-art-aware transforms. It is also the flagship showcase for the Lumen
Rust SDK.

## Download

Windows (x86_64) builds are on the
[releases page](https://github.com/lumen-fx/pixl/releases) as pre-releases.
Unzip `pixl-<version>-windows-x86_64.zip` anywhere and run `pixl.exe`; keep
the files beside it, since pixl reads its icons and font from that folder at
start. `pixl-cli.exe` beside it exports documents from the command line (see
Command line). The executable is not signed, so Windows SmartScreen
may ask before it runs the first time. On Linux and macOS, build it from
source (see Quick start).

## Status

Early. The editor is the `pixl` program in `app/`, built with the Lumen Rust
SDK: a menu row, the tool bar, tool options, a tab for each open
document, the canvas, and the color, palette, layers, tileset, history,
timeline and preview panels. Under it, crates with no UI hold the document,
the tools and the file formats (see Layout).

Started without a file, pixl opens on its start screen: the documents opened
or saved most recently, each with a thumbnail of its first frame, its folder,
its size and when it last changed, with a field that filters them by name or
folder. A click opens one. A document that is no longer there shows as
missing, with a button that takes it off the list; one with a recovery copy
newer than its file offers to recover it. After the recent documents the
list shows the rig demo and the Getting started tutorial; each opens as an
untitled copy that never joins the recent documents. Beside the list are New project and Open. The list and the
thumbnails are kept in pixl's folder of the user data directory
(`~/.local/share/lumen/pixl` on Linux), never beside the documents.

Several documents stay open at once, each in a tab above the canvas, as in
Aseprite. New, Open, the start screen's list and the tutorials each open a
document in a tab of its own; opening a file that is already open brings
its tab to the front. A tab shows the file's name, with a `*` while it has
unsaved edits, and its tooltip gives the full path. Each document keeps its
own undo history, selection and saved selections, view (zoom, pan, turn and
tiling), frame, layer, onion skin, guides, Rig panel mode and recovery copy;
the colors, the tool and its options, the settings, the palettes, the
brushes and the layouts belong to the window and stay as they are from one
tab to the next. Ctrl+Tab and Ctrl+Shift+Tab step through the tabs, a drag
moves a tab along the row, and Ctrl+W, a middle click or the tab's `x`
closes one, asking first when it has unsaved edits. A right-click on a tab
offers Close, Close others, Close to the right and Copy path. The Home tab
at the start of the row shows the start screen over the open documents;
closing the last document returns to it. Closing the window while
documents have unsaved edits asks once, naming them. While something is
under way on the document in front (a stroke, a shape, a free transform, a
filter preview, an image edit), the tabs wait until it is applied or
dropped.

The canvas view zooms from 1 to 6400 percent around the pointer (the wheel,
or `+`, `-`, `0` to fit and `1` for 100 percent) and pans with the middle
button, a drag with the space bar held, or W, A, S and D held down (Shift
moves faster). Pixels stay sharp at every zoom and a checkerboard shows
through transparency. View > Pixel grid (Ctrl+Shift+') outlines each pixel
once the view is zoomed in far enough (800 percent unless the settings say
otherwise); it is off until you turn it on, and stays as you leave it. Each layer is its own pixel buffer that Lumen's renderer
composites with the layer's blend mode and opacity, on the GPU when the app
renders there; what the renderer cannot composite the way `pixl-document` does
(add and subtract, clipping, masks, groups) is composited by `pixl-document` on the
CPU instead, and so are the bottom layers of a document whose layers would
not fit the layer buffer memory set under Performance in the settings. More
buffers make strokes on lower layers cheaper and every frame of the renderer
dearer, so the default holds eight buffers of 4096x4096. Only the document in
front is drawn; the documents behind other tabs keep their buffers while
all of them fit that memory together, so switching back to one draws at
once, and past it the tabs left longest ago give theirs up and draw anew
when they come back. Large redraws run on
worker threads and show tile by tile as they finish.

F3 shows the performance overlay over the canvas: frames per second, the
frame time (average, 95th percentile and longest over the last three
seconds) with a graph of it, the time from the last stroke to the screen,
the tick's work in the last frame (the editor's systems, Lumen's styling and
layout), the editor's own part of it and the compositing, the tiles it
wrote, the layer buffers and their memory, the document's tiles and the
undo history in memory, an estimate of the GPU memory, the worker threads
and their queue, and the renderer. Lumen does not tell an app when a frame
reached the screen, so frame times run from one frame's tick to the next.

The tools run on `pixl-paint`, and the tool bar shows each as a pixel icon
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
  noise or a pattern instead of blending. A gradient over a large canvas
  fills in tile by tile on worker threads, nearest where the drag ended
  first; Escape stops it and puts the layer back.
- Select: rectangle, ellipse, lasso, polygon lasso and magic wand
  (tolerance, contiguous or not, one layer or all, diagonals or not). Shift
  adds, Alt subtracts, both intersect.
- Transform: free transform (Ctrl+T) lifts the selection or the layer into
  a box with handles: drag inside to move, a corner or an edge to scale from
  the opposite side (Shift keeps the proportions, Alt scales from the
  middle, Ctrl on an edge skews), outside to turn (Shift steps 15 degrees),
  or type an angle; Flip H and Flip V mirror it. The preview resamples with
  RotSprite, clean edge or nearest neighbor, and Enter applies it as one
  undo step while Escape puts the pixels back. The preview of a large box
  fills in tile by tile on worker threads, from its middle out; Enter
  waits for its last tiles.
- Other: eyedropper, move, crop, text in bitmap fonts (pixl's own 5x7 font,
  or any BDF font), measure, and slices.

The size slider, the brush shape (round, square, or a custom brush made from
the selection), the stroke stabilizer, pixel-perfect (kept per tool) and
symmetry (mirror left and right, top and bottom, both, or radial with any
number of copies) sit in the tool options bar. [ and ] step the brush size. X swaps the foreground and
background colors, the secondary button paints with the background color,
and Shift snaps lines to clean pixel steps and shapes to squares and
circles. Every tool keeps to the selection. The pen's eraser end erases.

The brush tools (the pencil, eraser, soft brush, shading, dither, clone and
pattern stamps, smudge, blur and replace color) have a Dynamics button in
the tool options. Its popup sets what pen pressure and the pointer's speed
do to a stroke: each of the brush size, the brush angle and, for the pencil,
the color can follow pressure or speed. Size runs between two percentages of
the tool's size; angle between two angles, turning square and custom brushes
with RotSprite so a turned brush keeps clean one-pixel edges; color runs from
the background color to the foreground color, blended, or dithered with a
Bayer matrix, blue noise or the pattern stamp's pattern so only those two
colors appear, which keeps an indexed image on its palette. Each input has a
threshold below which it reads as nothing, one from which it reads as full,
and a curve between, drawn beside its sliders. A mouse presses at full
pressure, so mouse strokes use speed. Each tool keeps its own dynamics in the
settings; the pencil and the eraser start with pressure setting the size.
The pen settings of earlier versions (Settings > Pen) are replaced by the
pencil's and the eraser's dynamics, which take over their values.

The Select menu selects everything, nothing, the inverse, every pixel of
the foreground color or of the layer's content, grows, shrinks or borders the
selection by a number of pixels, and saves selections to load again later
while the document is open, replacing, adding to, subtracting from or intersecting the
current one.

The Edit menu copies and pastes through the system clipboard, as Aseprite
does. Copy (Ctrl+C) puts the selection of the active cel, or the cel's
content with nothing selected, on the clipboard as an image, so other
programs can paste it; Cut (Ctrl+X) also clears it, as one undo step; Copy
merged (Ctrl+Shift+C) copies what all the visible layers show there. Paste
(Ctrl+V) floats the clipboard's image in the free transform box, where it
was copied from when that place is in view and in the middle of the view
otherwise; move, scale and turn it with the box's handles, then Enter keeps
it as one undo step and Escape drops it. Paste as new layer (Ctrl+Shift+V)
puts it on a new layer above the active one, and Paste as new document
(Ctrl+Alt+V) opens it in a tab of its own. An image copied in another
program, such as a screenshot, pastes the same way. Between pixl documents a
copy keeps its exact pixels and its place; an indexed copy keeps its palette
indices when pasted on the same palette, and anything else pasted on an
indexed document lands on the nearest palette color, as an imported image
does.

The Image menu crops to the selection, sets the canvas size around an anchor,
resizes the image (nearest, RotSprite or clean-edge sampling, so no new
colors appear), flips and turns the canvas, converts to indexed color (on the
current palette or a new one made from the image, with no dither, a Bayer
matrix or Floyd-Steinberg) or back to RGBA, and replaces one color with
another in this frame or every frame. It also loads and saves palettes as
`.gpl`, `.pal`, `.hex` and `.ase`, sorts the palette by hue, saturation,
lightness or brightness, builds a ramp around the foreground color with its
hue turning from shadow to highlight, makes a palette from the image, and
locks the painting colors to the palette. Indexed color, a palette from the
image and the image size work on threads of their own: the dialog stays
open, saying how far the work is, until the result lands as one undo step,
and Cancel or Escape stops it.

The Filter menu has hue and saturation, brightness and contrast, levels,
curves, color balance, posterize, invert, desaturate, gradient map, outline
(inside or outside, with or without diagonals), drop shadow, noise and
pixelate. Each previews on the canvas while its settings change and applies
as one undo step; Cancel puts the layer back. A filter works through the
image tile by tile on worker threads, nearest the middle of the view first,
so a large layer fills in as it goes and a change of setting stops the work
at once. An empty frame of the layer has nothing to filter. Indexed layers stay on their palette.

The View menu shows rulers, which guides drag out of and back into; shows
the pixel grid; snaps to the grid, the guides and the slices; draws a grid
of any cell size and offset, square or isometric; repeats the document left and right and above
and below for seamless tiles; turns the view in 15 degree steps and mirrors
it; and shows the slices. The symmetry axes are marked in the rulers and
move by their marks. Guides last while the document is open and the grid
while the editor runs; neither is saved with the document.

The panels on the right show the document. The color panel edits the
foreground or the background color; a click on either swatch picks which,
and the one being edited is outlined. Pick the color on a saturation and
value square beside a hue strip, or on a hue wheel around the square, with
an alpha strip below; the choice of picker is kept in the settings. Sliders
set the color in RGB, HSV or HSL (also kept), each with a field to type the
value, and a hex field takes `#rrggbb` or `#rrggbbaa`. A drag previews in
the panel and sets the color once, when the button comes up. The arrow keys
step what is under the pointer (saturation and value on the square, the
hue on the strip or the wheel, alpha on its strip) or the value of the
focused field; Shift steps ten at a time. A gray keeps the hue it was
reached from, so moving through it does not reset the picker. In an indexed
document the color stays as picked and painting lands on the nearest
palette entry. The palette sets the foreground with a click and the
background with a right-click; the history panel lists every edit and jumps
to any of them. Ctrl+Z undoes, Ctrl+Shift+Z or Ctrl+Y redoes.

The layers are listed in the timeline below the canvas, one row per layer
against the frames, as in Aseprite. Each row carries the layer's eye, a lock
button that locks or unlocks everything, its name and, for a group, a fold
button; above the rows, New adds a layer, Delete removes the active one and
Props opens its properties. The Layers panel lists the same layers with
their color tags, clipping and mask marks, the active layer's opacity and
blend mode, and buttons to group, copy and move layers up and down. It is
hidden in the built-in layouts; Window > Panels and layouts shows it. Clicks
and drags work the same in both lists.

Layers nest in groups: a new layer goes into the active group, a group's
fold button hides what it holds, and a layer dragged onto another row lands
above or below it, or into a group dropped on its middle. A double-click on a
row opens the layer's properties. A mask hides the layer where it is dark:
Add mask starts painting on the mask instead of the pixels, the mask button
in the Layers panel's row or Layer > Edit mask or pixels switches between the
two, and the mask can be turned off, applied to the pixels or deleted. A
clipped layer shows only where the layer below it does. A reference image
(Layer > Reference image) is any image pixl opens, kept at its full
resolution and drawn smoothly at screen resolution over the canvas, not on
its pixel grid, so a high-resolution photo stays sharp at any zoom. It
arrives at half opacity in the middle of the canvas, scaled down to fit when
it is larger, and sits in the layer stack like any layer: above the art or
below it. While its layer is active a box with handles surrounds it, and
every tool but the eyedropper places it: drag inside the box to move it, a
corner to scale it keeping its proportions (Shift frees them), an edge to
stretch one side, or outside the box to turn it (Shift steps 15 degrees);
Alt scales from the middle, and each drag is one undo step. The layer's
properties set its center, size and angle by number. It is never painted
on, never exported, and the eyedropper and the other tools read the art
alone. The Layer menu, also opened by a right-click on a row,
duplicates, merges down, flattens, clips, locks pixels, alpha or position,
and opens the layer's properties: name, opacity, blend mode, color tag,
clipping, locks and the mask.

A tilemap layer builds a map from a tileset, as in Aseprite: a list of
tiles of one size, numbered from 1, with tile 0 the empty tile. Each cell
of the layer's grid shows one tile, plain or flipped across, down or along
the diagonal, so a level or a pattern is drawn once and placed many times.
Layer > New tilemap layer makes one, with a new tileset of a tile size you
choose or one the document already has; tilesets are shared, so several
layers can draw from one. Layer > Convert to tilemap cuts a layer's pixels
into tiles, one tile for every distinct cell, and Convert to normal layer
turns a tilemap back into plain pixels. While a tilemap layer is active, its
grid of cells shows over the canvas.

On a tilemap layer, Tab switches the tools between Pixels and Tiles (the
button at the left of the tool options does too). In Tiles mode the pencil
and the brush place the picked tile in each cell they cross, the eraser and
the secondary button empty cells, the bucket fills the joined cells that
hold the same tile, the rectangle fills a block of cells, and the
eyedropper picks the tile under the pointer, flips included. In Pixels mode
every tool paints as on any layer, and when the stroke ends the draw mode
beside the Tiles button says what happens to the tileset: Manual changes the
tiles under the stroke, so every place they show changes with them, and
drops what was painted on empty cells; Auto reuses a tile that already
looks like the edited cell, changes a tile in place when that cell is its
only use, adds a tile otherwise, and removes the tiles the edit left
unused; Stack reuses or adds tiles and never changes or removes one. A
paste in Pixels mode lands in the tileset the same way when Enter keeps it.
The draw mode is the window's, kept as you switch documents.

The Tileset panel shows the active tilemap layer's tiles, the empty one
first. A click picks the tile the tools place and switches to Tiles mode;
the wheel scrolls a long tileset. Earlier and Later move the picked tile,
Copy duplicates it, Delete removes it and empties the cells that showed it,
and Unused removes every tile no cell shows. Each is one undo step, and
moving tiles never changes what the map shows.

Click an eye to show or hide its layer; a hidden group hides everything in
it. Alt+click an eye to see only that layer, with the groups holding it, and
Alt+click it again to bring back what was showing before. Drag down the
column of eyes to show or hide each layer you pass, the way the first eye
went. Ctrl+click a row to add it to the selection or take it out, and
Shift+click to select the rows from the active layer to it; the active layer,
the last row clicked, is still the one the tools paint on. The Layer menu's
Show only selected, Show all layers and Hide selected act on the selection;
its other commands act on the active layer. Each visibility change is an
undo step.

Panels sit in docks left, right and below the canvas. Drag the gutter beside
a dock to resize it. Each panel's header collapses it to its title or hides
it, and a right-click on the header moves it to another dock or up and down
its dock. Window > Panels and layouts lists every panel with its dock, its
order and whether it shows, and switches between layouts: the built-in Paint,
Animate, Rig and Tiles (the Tileset panel beside the colors), and any the
user saves under a name of their own. Window >
Reset layout puts the current layout back as it was saved. The arrangement in
use and the saved layouts are kept in `workspace.json` in pixl's data folder,
so the window reopens as it was left; an arrangement saved while the Layers
panel showed keeps showing it until you switch layouts.

Window > Customize the tool bar picks which tools the tool bar shows and in
what order, one column or two, and groups related tools on one button: the
button shows the tool used last, and a right-click or a long press on it
opens a flyout of the others. Full puts every tool on a button of its own;
Compact groups the brushes, retouching tools, stamps, lines, shapes, fills,
selections, transforms and guides into one column. The arrangement is kept
in `toolbar.json` in pixl's data folder.

The Brushes panel is a library of named brushes: each keeps a shape (round,
square, or a custom mask), a size, opacity and hardness for the soft brush,
dithering, pixel-perfect, the stroke stabilizer and the dynamics, which
picking the brush gives the tool in use; brushes saved by an older pixl
bring their pressure mapping as dynamics. A click makes one the brush in
use. Save keeps the current brush under the name typed beside it; Selection makes a brush of the selected
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
`{frame}`, `{layer}` and `{scale}` fill in. `{frame}` counts from 0, padded
to the last frame's digits; `{frame1}` or `{frame001}` count from that
number, padded to its digits. File > Export all (Ctrl+Alt+E)
runs every preset marked for it on the open document, in the background, and
the status bar says how many files it wrote. The presets are kept in
`exports.json` in pixl's data folder and serve every document, and
`pixl-cli --preset NAME` runs them without the editor.

Ctrl+Shift+P opens the command palette: one search field over every command
a key can run, every tool, the layouts, the themes, the open documents and
the recent ones.
Typing narrows the list to the entries that have the typed letters in order,
the arrow keys pick one and Enter runs it. Each entry shows its keys, so the
palette also answers "what is the shortcut for this".

The timeline panel, below the canvas by default, lists every layer against
every frame, with the document's tags above the frame numbers. Its layer
rows are the layer list described above.
A cel shows whether it holds pixels and joins its neighbors when they are
linked; the frames a rig rendered are marked, on their frame numbers and on
the cels of the skeleton and the layers bound to it, as the ones Regenerate
draws again. Click a cel or a frame number to go there; drag or Shift-click to
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
theme, under or over the current frame, and steps aside while the animation
plays. The preview panel shows the
document at actual size, or fitted to the panel, and plays along; the Paint
layout hides it and Animate shows it beside the timeline.

The File menu makes a new project (Ctrl+N), opens and saves `.pixl`
documents (Ctrl+O, Ctrl+S, Ctrl+Shift+S), and exports the current frame as a
PNG (Ctrl+E) or the animation as a GIF. Open also reads Aseprite, PNG and GIF
files, as new documents that save as `.pixl`. A new project has a name, a
size (typed, or one click on a square, sprite or screen preset), RGBA or
indexed color, and a transparent or colored background. While
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
wheel), how the canvas looks (checkerboard size and colors, whether the pixel grid shows,
its color and the zoom it shows from, a new custom grid's cell, the color
around the
document), the interface (theme, UI font, UI scale, tooltips), files
(autosave interval, recovery copies, the length of the recent list),
performance (the progressive redraw threshold, worker threads, the layer
buffer memory, the performance overlay) and every keyboard shortcut. Each
change applies at once. Shortcuts start from pixl's own keys or Aseprite's
or Photoshop's; a key already in use asks before it moves, and the menus show
the keys in force. Apply and Cancel (Enter and Escape) finish or drop the
shape or transform under way and leave the rig's modes; while no tool is
busy their keys run whatever else they are bound to, such as Enter playing
the animation under Aseprite's keys. The settings are kept in `settings.json` in pixl's data
folder, beside the recent list. UI scale grows the text and the sizes the
theme keeps as tokens; a few fixed sizes (list rows, swatches) stay as they
are.

The editor builds against Lumen 0.0.10 from crates.io.

A skeleton layer rigs a character so it can be posed instead of redrawn:
bones in a tree, with raster layers (or selected regions of them) bound to
one bone each as rigid parts. Rendering a pose draws every part at its
bone's place with RotSprite, so turned parts keep clean one-pixel outlines,
into frames of ordinary pixels to touch up.

The Rig panel works on the document's skeleton (New skeleton makes one). In
Bones mode a drag on the canvas draws a bone, joined to the selected bone
when there is one; starting the drag on the selected bone's end joins the
new bone there. Dragging a joint or a binding's pivot moves it, and Shift
moves the selected bone's end. The line under the panel's header says what a
drag does in the current mode. The panel lists the bones, renames and
reparents the selected one, and binds the active layer, or the selected
pixels of it, to it. The rig reads its parts from its rest frame, which "Use
this frame" sets. In Pose mode a drag turns a bone and the bones joined to
it, its head moves it, and with IK on the end of a bone bends it together
with its parent; Shift snaps turns to 15 degrees. Posing a frame other than
the rest frame draws the pose into that frame as it changes and keys it
there; on the rest frame only the bones move. Poses save to a library with
thumbnails, mirror left to right by bone name (`arm.L` and `arm.R`, `leg_l`
and `leg_r`), and reset to rest. A frame's key has an easing toward the next
key. Render to new frame adds a frame holding the pose; Render keyed fills a
range of frames from the keys, eased; Regenerate renders every generated
frame again after the rig changes. Bones are drawn over the art in the
theme's colors and are never part of it. Escape leaves either mode, and
picking a tool hands the canvas back to it. Outside the Rig layout (Window
menu) the panel shows only its header until a mode starts; Bones or Pose
with no skeleton yet unfolds it to New skeleton.

Help > Open rig demo, also listed on the start screen, opens a character
rigged and keyed as an eight-frame walk; press Play to watch it.

The Help menu's tutorials walk through pixl one workflow at a time:

- Getting started: pick the pencil and a color, draw, erase, undo and redo,
  zoom and pan, save.
- Layers: in the timeline's layer rows, add a layer, name it, paint on it,
  drag it under another, hide a layer and show one alone, fade it and merge
  it down.
- Selections: select a box, move what it holds, deselect, lasso and add to
  the lasso with Shift, paint inside the selection.
- Animation: add frames, turn on the onion skin, draw a bounce, time a
  frame, play, open the sprite sheet export.
- Palettes: pick a swatch, build a ramp, paint and shade along it, convert
  the picture to indexed color.
- Rig: a plainer copy of the demo's character, its arm held out and no
  skeleton. Draw a body bone, draw the head and the arm from the body's end
  so they join it, bind each part's layer to its bone, raise the arm on one
  frame and turn the body on another, which carries the head and arm along,
  then render the frames between and play.

Each tutorial opens a small document of its own as an untitled copy, so what
pixl ships is never changed, switches to the layout it works in, and shows a
card over the canvas listing its steps. Each step says where to drag or
outlines the control to click next, and ticks itself off once the editor
shows it done. Closing the card keeps the place, and the tutorial's Help item
shows it again. The tutorial stays with its document's tab: its card shows
while that tab is in front. The start screen
lists Getting started after the rig demo.

Documents save as `.pixl` files: one versioned, chunked, compressed file that
stores each distinct tile once. Reference images are embedded at their full
resolution with their place over the canvas, so a file carries everything it
shows. Saves are atomic, and autosave keeps a recovery copy beside the
document. A file from a newer pixl opens in an older one when the newer data
is optional; what the older version cannot read is skipped with a warning.
Format 1.2 added full-resolution reference images: a pixl that reads format
1.1 opens such a file without its references, and references in older files
open where they were, one image pixel to a canvas pixel. Format 1.3 added
tilesets and tilemap layers: a pixl that reads format 1.2 opens each tilemap
layer as an ordinary layer showing the same pixels, without its tileset.

Aseprite `.ase` and `.aseprite` files open and save as a first-class format:
layers and groups, linked cels, RGBA and indexed color, the palette, tags,
slices, tilesets and tilemap layers carry over, with each cell's tile and
flips. An Aseprite reference layer opens as a reference at the cel's own
size and place. What one side cannot hold (pixl's layer masks and clipping,
tilesets Aseprite keeps in another file) is listed as a warning instead of
dropped silently. Saving as Aseprite leaves reference layers out, as every export
does, and lists each one.

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

Tilemap layers export as the pixels they show, in PNG frames, GIFs and
sprite sheets like any layer. File > Export tileset writes the active
tilemap layer's tileset (or the document's first) as one PNG, rows of up to
16 tiles with the empty tile 0 first, so a tile's number in a map is its
place on the sheet, and a JSON file beside it naming each tile's rectangle
the way sprite sheet JSON names frames, with the tileset's name, tile size,
count, columns and base index.

## Command line

`pixl-cli` exports documents without the editor, for game build pipelines
and CI machines with no display or GPU: PNG frames, GIFs, and sprite sheets
with their Aseprite-shape JSON, from `.pixl`, `.aseprite`, `.ase`, `.png` and
`.gif` files. It writes what the editor's exports write for the same
settings, through the same code. It reads Aseprite's batch flags, so a
pipeline built on `aseprite -b` switches by changing the program name, and
it runs the editor's export presets.

```sh
pixl-cli -b hero.aseprite --sheet hero.png --data hero.json --sheet-type packed --trim
pixl-cli hero.pixl --split-tags --save-as build/hero.gif --scale 4
pixl-cli hero.pixl --tag walk --save-as 'frames/{title}-{frame001}.png'
pixl-cli hero.pixl --split-layers --sheet layers.png --data layers.json
pixl-cli hero.pixl --preset "Sprite sheet"     # one of the editor's presets
pixl-cli hero.pixl --all-presets               # what File > Export all runs
pixl-cli hero.pixl --list-tags
```

`pixl-cli --help` lists every option. It exits with 0 when everything was
written, 1 when an export failed (a file that cannot be read or written, a
layer, tag or preset that does not exist), and 2 when the command line is
wrong, with the reason on standard error. Its output has no colors. Presets
come from the editor's `exports.json` (a new user's starter presets when there
is none), or from the file `--presets FILE` names, and write beside the
document as they do in the editor; `--save-as` and `--sheet` write where they
say, relative to the working folder.

| Aseprite | pixl-cli |
| --- | --- |
| `-b`, `--batch` | accepted; every run is a batch run |
| `--save-as FILE` | same, for `.png` frames or a `.gif` |
| `--sheet FILE`, `--data FILE` | same; no JSON without `--data` |
| `--format json-hash`, `json-array` | same |
| `--sheet-type horizontal`, `vertical`, `rows`, `columns`, `packed` | same |
| `--sheet-pack` | same as `--sheet-type packed` |
| `--sheet-columns N`, `--sheet-rows N` | same |
| `--border-padding N`, `--shape-padding N` | same |
| `--trim` | same, for `--sheet` |
| `--layer NAME`, `--ignore-layer NAME`, `--all-layers` | same; names or group paths such as `body/arm` |
| `--split-layers` | same; with `--sheet`, every layer on the one sheet |
| `--split-tags` | same, for `--save-as` |
| `--tag NAME`, `--frame-tag NAME` | same |
| `--frame-range A,B` | same, counted from 0 |
| `--oneframe` | same |
| `--scale N` | whole factors only |
| `--list-layers`, `--list-tags` | same |
| `{title}`, `{name}`, `{tag}`, `{layer}`, `{frame1}`, `{frame001}`, `{tagframe}` | same, except `{name}` is the document's name without its extension, like `{title}` |
| `{frame}` | padded to the last frame's digits; `{frame0}` is Aseprite's unpadded `{frame}` |
| (none) | `--preset NAME`, `--all-presets`, `--presets FILE`, `--list-presets` |

A `--save-as` name without tokens gets the ones Aseprite adds: ` ({layer})`
when layers split, ` #{tag}` when tags split, and the frame number when each
frame is a file, counting on from a number the name ends in, else from 1.
Sheet JSON names its frames `{title} {frame}`, without Aseprite's file
extension.

pixl-cli reads one document per run, so an atlas of several sprites takes
one sheet per sprite. Aseprite options it does not do (scripts,
`--filename-format`, `--ignore-empty`, `--merge-duplicates`,
`--inner-padding`, `--crop`, slices, color mode and palette changes, and
fractional scales) stop the run with exit code 2 rather than export
something different.

## Layout

- `crates/document` (`pixl-document`): the document model, with no UI:
  layers, cels, frames, tags and slices, palettes and their files,
  selections, undo history, compositing, transforms, reference images,
  skeleton rigs, tilesets and tilemap layers, and the native `.pixl` format.
- `crates/paint` (`pixl-paint`): the raster tools and filters: the pencil,
  brushes and inks, fills, gradients, shapes and text, and the adjustments
  and effects.
- `crates/io` (`pixl-io`): other programs' formats: Aseprite files, GIF,
  PNG, and sprite sheets with Aseprite-compatible JSON, and the export
  presets.
- `crates/cli` (`pixl-cli`): the command line exporter.
- `app` (`pixl`): the editor.

## Quick start

```sh
cargo run -p pixl                 # the start screen
cargo run -p pixl -- art.pixl     # straight into a document
cargo run -p pixl-cli -- --help   # the command line exporter
cargo test
```

A release build (`cargo build --release -p pixl`) compiles all of the markup
and stylesheets in: `app/build.rs` splices every `<include>` and `@import`
into one markup file and one stylesheet, which the executable embeds. A debug
build reads `src/` from disk instead, for hot reload. The icons and font in
`assets/` stay files, read from the folder beside the executable when that
folder holds a `lumen.toml`, and from `app/` in the source tree otherwise. The
Windows zip is that folder: `pixl.exe`, `pixl-cli.exe`, `lumen.toml`,
`LICENSE`, and `assets/icons` and `assets/fonts`. Pushing a `v*` tag that
matches the workspace version builds it on CI and publishes it as a
pre-release.

The rig demo and the tutorials' documents are built by
`app/tests/common/demo.rs`, and a test checks the files shipped in
`app/assets/demo/` match. After changing one, or the `.pixl` format, write
them again with `PIXL_WRITE_DEMO=1 cargo test -p pixl --test demo`.

`cargo bench -p pixl-document --bench rig` times posing and rendering a
20-bone character, and `cargo bench -p pixl --bench pose -- gpu` (or `cpu`)
times a pose drag to the frame that shows it (`side=4096 layers=32` puts the
character in the view bench's document).

`cargo bench -p pixl-paint --bench edits` times the brushes, the filters, the
gradient, palette and indexed conversion, free transform resampling and
selection grow and border on a 4096x4096 document of 32 layers.

`pixl --perf-log frames.csv` writes one row per frame to a CSV file: the
frame's start, its time, the tick's work, the editor's systems' part of it,
the compositing, the tiles written, the layer buffers and their memory, the
worker queue and the time to the screen of the last stroke.

### Performance bench

```sh
cargo bench -p pixl --bench view -- gpu      # or cpu
cargo bench -p pixl --bench view -- gpu budget=2048 log=frames.csv
```

The view bench opens a 4096x4096 document of 32 layers with no window and
measures what a user waits for: the progressive redraw after opening it,
the time from a stroke to the frame that shows it on the top layer and on
the bottom one, the frame after a pan and after a zoom step, the same with
a 4000x3000 photo as a reference layer in the middle of the stack, the same
again with a tilemap layer of 256x256 cells of 16x16 tiles on top, being
edited with its grid showing, hiding a layer, and exporting the frame as a
PNG; and what a tick costs when nothing
changes, the editor's systems alone and the whole tick. `side=N` and
`layers=N` change the document, `budget=MiB` the layer buffer memory,
`overlay` shows the performance overlay while it runs, `log=FILE` writes the
per-frame CSV the `--perf-log` flag writes, and `systems` lists what each of
the editor's systems cost a tick in each part of the run. Numbers move with
the load on the machine, so compare runs taken under the same load.

`cargo bench -p pixl --bench view -- cpu gate` (or `gpu gate`) also checks
the run against `app/benches/gate.json`: the editor's work on an idle tick,
the whole idle tick, a stroke on the top and on the bottom layer, a pan, and
a pan and a zoom step with the reference and with the tilemap, each a
median against a limit for the renderer. A number past its limit
fails the run and names the editor's systems that cost the most where it was
measured. The limits leave room above what a desktop measures, to catch
regressions rather than to grade the machine. Run the gate locally before
you push; it enforces the limits. `gate report` prints the same numbers
without failing on them. CI runs `cpu gate report`, because its runners are
slower than the desktop the limits were set on: the CI log shows every
number, and a number past its limit does not fail the build.

`cargo bench -p pixl --bench playback -- gpu` (or `cpu`) times stepping to
the next frame, with the onion skin and without, until the frame first shows
and until all of it is drawn, and the frame rate playback reaches at 12
frames per second and as fast as it goes. It plays two 4096x4096 documents
of 32 layers: `sprite` (the default), where a few layers redraw a figure
each frame and the rest are linked, and `full`, where every cel is its own
full image; `frames=N`, `side=N` and `layers=N` change them.

`cargo bench -p pixl --bench heavy -- gpu` (or `cpu`) runs the edits too
large for one frame on the same document (a gradient over the top layer, a
free transform turning the whole top cel with RotSprite, a palette made from
the image, and indexed color with Floyd-Steinberg) and gives, for each, the
time until it lands as an undo step and the longest tick while it runs,
with the editor's systems that cost the most in it: how long the editor
stops answering.

`cargo bench -p pixl --bench customize -- gpu` (or `cpu`) times, on the
same document, what the workspace, themes, command palette and export
presets cost each frame with a layout and a user theme in force, beside the
time from one frame to the next. They are meant to cost nothing a frame
would show.

On Linux the build links GTK 3, X11, xkbcommon and Wayland, so their
development packages must be installed.

## License

MIT, see [LICENSE](LICENSE).
