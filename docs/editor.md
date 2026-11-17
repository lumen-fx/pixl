# The editor

The window, the documents in it, and how to arrange and set up pixl. For the
tools themselves see [Tools](tools.md); for layers and frames see
[Layers, animation and rigs](layers-and-animation.md).

## Start screen

Started without a file, pixl opens on its start screen. It lists the
documents you opened or saved most recently, each with a thumbnail of its
first frame, its folder, its size and when it last changed; the field above
filters them by name or folder. A click opens one. A document that is no
longer there shows as missing, with a button that takes it off the list; one
with a recovery copy newer than its file offers to recover it.

After the recent documents come the rig demo and the Getting started
tutorial. Each opens as an untitled copy and never joins the recent list.
Beside the list are New project and Open.

The list and the thumbnails live in pixl's data folder
(`~/.local/share/lumen/pixl` on Linux), never beside your documents.

## Tabs

Each open document has a tab above the canvas, as in Aseprite. Opening a file
that is already open brings its tab to the front. A tab shows the file name,
with `*` while it has unsaved edits; its tooltip gives the full path.

- Ctrl+Tab and Ctrl+Shift+Tab step through the tabs; drag a tab to reorder.
- Ctrl+W, a middle click or the tab's `x` closes it, asking first when it has
  unsaved edits.
- Right-click a tab for Close, Close others, Close to the right and Copy
  path.
- The Home tab at the start of the row shows the start screen. Closing the
  last document returns to it.
- Closing the window with unsaved documents asks once, naming them.

Each document keeps its own undo history, selections, view, current frame
and layer, onion skin, guides, Rig panel mode and recovery copy. The colors,
the tool and its options, settings, palettes, brushes and layouts belong to
the window and carry across tabs.

While a stroke, shape, free transform, filter preview or image edit is under
way, the tabs wait until it is applied or dropped.

## Canvas view

- Zoom: the wheel zooms around the pointer; `+` and `-` step, `0` fits the
  document and `1` shows it at 100 percent.
- Pan: the middle button, a drag with Space held, or W, A, S and D held down
  (Shift moves faster).
- View > Pixel grid (Ctrl+Shift+') outlines each pixel once the view is
  zoomed in far enough. It is off until you turn it on.

Pixels stay sharp at every zoom, and a checkerboard shows through
transparency.

The View menu also has:

- Rulers. Drag guides out of them and back into them. The symmetry axes are
  marked in the rulers and move by their marks.
- Snapping to the grid, the guides and the slices.
- A custom grid of any cell size and offset, square or isometric.
- Tiled view, which repeats the document left and right and above and below
  for seamless tiles.
- Turning the view in 15 degree steps, and mirroring it.
- Showing the slices.

Guides last while the document is open and the custom grid while pixl runs;
neither is saved with the document.

## Panels and layouts

Panels sit around the canvas, each under a tab.

- Drag a tab onto another panel to share its place, near one of its edges to
  sit beside it, or over empty canvas to float it.
- Move to new window in a tab's right-click menu opens the panel in a window
  of its own; Dock back in that window's tab menu returns it.
- A tab's close button, Close in its menu, or Delete on a focused tab hides
  the panel.
- Drag the gutter between two panels to resize them. The canvas gives way;
  the panels and the tool bar do not.

Window > Panels and layouts lists every panel and whether it shows. From
there you can place a panel Left, Right or Bottom, pop it out into a Window
of its own and dock it back, or fold it to its tab. It also switches layouts: the
built-in Paint, Animate, Rig and Tiles, and any you save under your own name.
Window > Reset layout returns the current layout to how it was saved, and
Window > Show all panels shows the hidden ones.

pixl reopens the way you left it. The arrangement and your layouts are kept
in `dock-layout.json` in pixl's data folder. An older `workspace.json` there
is taken over once and removed.

### Tool bar

Window > Customize the tool bar picks which tools show, in what order, in one
column or two. Related tools can share a button: the button shows the tool
used last, and a right-click or long press opens the others. Full gives every
tool its own button; Compact groups them into one column. The arrangement is
kept in `toolbar.json`.

## Command palette

Ctrl+Shift+P opens one search field over every command a key can run, every
tool, the layouts, themes, scripts and extension commands, and the open and
recent documents. Type letters in order to narrow the list, pick with the
arrow keys and press Enter. Each entry shows its shortcut.

## Color, palette and history

The color panel edits the foreground or the background color; click a swatch
to choose which. Pick on a saturation and value square beside a hue strip, or
on a hue wheel around the square, with an alpha strip below. Sliders set the
color in RGB, HSV or HSL, each with a field to type into, and the hex field
takes `#rrggbb` or `#rrggbbaa`. The arrow keys step the value under the
pointer or in the focused field; Shift steps by ten. A gray keeps the hue it
was reached from. pixl remembers your choice of picker and slider model.

In an indexed document the color stays as picked, and painting lands on the
nearest palette entry.

Click a palette swatch to set the foreground, right-click for the background.
The history panel lists every edit and jumps to any of them. Ctrl+Z undoes;
Ctrl+Shift+Z or Ctrl+Y redoes.

## Settings

Edit > Settings (Ctrl+,) covers:

- Navigation: what the wheel does, zoom speed, zooming around the pointer, scroll
  and pan speeds, inverted wheel.
- Canvas: checkerboard size and colors, the pixel grid and the zoom it shows
  from, the custom grid's cell, the color around the document.
- Interface: theme, UI font, UI scale, tooltips.
- Files: autosave interval, recovery copies, the length of the recent list.
- Performance: the progressive redraw threshold, worker threads, layer buffer
  memory, the performance overlay.
- Keyboard shortcuts.

Changes apply at once. Shortcuts start from pixl's own keys, Aseprite's or
Photoshop's; assigning a key already in use asks before moving it, and the
menus show the keys in force. Settings are kept in `settings.json` in pixl's
data folder.

Apply and Cancel (Enter and Escape) finish or drop the shape or transform
under way and leave the rig's modes. While no tool is busy, those keys run
whatever else they are bound to, such as Enter playing the animation under
Aseprite's keys.

UI scale grows the text and the sizes the theme keeps as tokens; a few fixed
sizes, such as list rows and swatches, stay as they are.

## Themes

pixl's theme is CSS over its own tokens. The default is a dark set of neutral
grays so the colors in your art read true; light, high contrast and retro
sets are built from the same tokens.

Window > Themes picks a set and edits its color tokens (surfaces, text,
accent, canvas background, checkerboard, grid, selection and tool preview
colors) with a live preview, along with the UI font and scale, and saves the
result as your own theme. A theme is a `.pixltheme` file of plain CSS custom
properties in the `themes` folder of pixl's data folder; Import and Export
share them.

The UI font is Pixelify Sans; a setting switches the UI to the platform's
sans-serif font instead.

## Tutorials and the rig demo

The Help menu's tutorials walk through one workflow each:

- Getting started: pick the pencil and a color, draw, erase, undo and redo,
  zoom and pan, save.
- Layers: add, name and paint a layer, reorder it, hide one and show one
  alone, fade it and merge it down.
- Selections: select a box, move what it holds, deselect, lasso and add to it
  with Shift, paint inside the selection.
- Animation: add frames, turn on the onion skin, draw a bounce, time a frame,
  play, open the sprite sheet export.
- Palettes: pick a swatch, build a ramp, paint and shade along it, convert to
  indexed color.
- Rig: draw bones for a character, bind its parts, pose two frames, render
  the frames between and play.

Each tutorial opens its own document as an untitled copy, switches to the
layout it uses, and shows a card over the canvas listing the steps. Each step
points at where to drag or outlines what to click, and ticks itself off when
done. Closing the card keeps your place; the tutorial's Help item shows it
again. The card follows its document's tab.

Help > Open rig demo opens a character rigged and keyed as a walk cycle;
press Play to watch it.

## Performance overlay

F3 shows the performance overlay over the canvas: frame rate, frame times
with a graph, the time from the last stroke to the screen, where the frame's
work went, layer buffer and tile memory, worker threads and the renderer.
Frame times run from one frame's tick to the next.

pixl composites layers on the GPU where it can and on worker threads where it
cannot, and redraws large areas tile by tile. The layer buffer memory under
Settings > Performance trades cheaper strokes on lower layers against the
cost of every frame. Only the document in front is drawn; documents in other
tabs keep their buffers while they fit in that memory, so switching back
draws at once.
