# Files and export

## Opening and saving

File > New project (Ctrl+N) asks for a name, a size (typed, or a square,
sprite or screen preset), RGBA or indexed color, and a transparent or colored
background. Open (Ctrl+O) reads `.pixl`, Aseprite, PNG and GIF files. Save
(Ctrl+S) and Save as (Ctrl+Shift+S) write `.pixl`; a document opened from
another format saves as a new `.pixl` file.

While a saved document has unsaved edits, pixl writes a recovery copy beside
it on an interval. Settings > Files changes the interval or turns recovery
copies off. Opening a document whose recovery copy is newer offers to
recover it.

## The .pixl format

A `.pixl` file is one versioned, compressed file that stores each distinct
tile once. Reference images are embedded at full resolution with their place
over the canvas, so a file carries everything it shows. Saves are atomic.

A file from a newer pixl opens in an older one when the newer data is
optional; what the older version cannot read is skipped with a warning.

- Format 1.2 added full-resolution reference images. A pixl that reads 1.1
  opens such a file without its references.
- Format 1.3 added tilesets and tilemap layers. A pixl that reads 1.2 opens
  each tilemap layer as an ordinary layer showing the same pixels.

## Aseprite

`.ase` and `.aseprite` files open with their layers and groups, linked
cels, RGBA and indexed color, the palette, tags, slices, tilesets and
tilemap layers with each cell's tile and flips. An Aseprite
reference layer opens as a reference at its cel's size and place.

What pixl cannot read, such as tilesets Aseprite keeps in another file, is
listed as a warning rather than dropped silently. The editor saves
documents as `.pixl` only.

## PNG and GIF

File > Export PNG (Ctrl+E) writes the current frame as a PNG; Export GIF
writes the animation with each frame's duration. One palette covers the
whole GIF; past 256 colors it is reduced by median cut, without dithering.
GIFs open with their frames, timing and disposal applied.

Tilemap layers export as the pixels they show, like any layer.

## Sprite sheets

File > Export sprite sheet (Ctrl+Shift+E) lays the frames out on one PNG:

- As a horizontal or vertical strip, a grid filled by rows or columns, or
  packed as tightly as they fit.
- Every frame, a range or a tag; padding between images and a border around
  the sheet; a whole-number scale; trimming each image to its pixels; one
  image per layer instead of per frame.

Beside the PNG it writes the JSON Aseprite's export writes, with frames as a
hash or an array, plus the tags, layers and slices, so engines that read
Aseprite sheets read these. The dialog previews the sheet as options change.

File > Import sprite sheet (Ctrl+Shift+I) splits a PNG into the frames of a
new document by cell size or by a count of columns and rows, with an offset
and padding, optionally skipping empty cells, and previews the cells over
the image.

## Tilesets

File > Export tileset writes the active tilemap layer's tileset (or the
document's first) as one PNG, in rows of up to 16 tiles with the empty tile 0
first, so a tile's number in a map is its place on the sheet. A JSON file
beside it gives each tile's rectangle, the way sprite sheet JSON names
frames, with the tileset's name, tile size, count, columns and base index.

## Export presets

File > Export presets keeps named export settings:

- Format: PNG frames, an animated GIF, or a sprite sheet with its
  Aseprite-style JSON.
- Scale, which frames (all, the current one, one tag, or each tag), and
  which layers (the visible ones merged, or each layer alone).
- The sheet layout: a row, a column, a grid, or packed, with or without
  trimming.
- The file to write, relative to the document's folder, with `{name}`,
  `{tag}`, `{frame}`, `{layer}` and `{scale}` filled in. `{frame}` counts
  from 0, padded to the last frame's digits; `{frame1}` or `{frame001}` count
  from that number, padded to its digits.

File > Export all (Ctrl+Alt+E) runs every preset marked for it on the open
document in the background, and the status bar says how many files it wrote.
Presets are kept in `exports.json` in pixl's data folder and serve every
document. `pixl-cli --preset NAME` runs them without the editor; see
[Command line](command-line.md).
