# Tools

The tool bar shows each tool as a pixel icon; its tooltip names the tool and
its key. Every tool keeps to the selection.

## The tools

- Paint: the pencil (pixel-perfect by default); a soft brush with opacity and
  hardness, or the same coverage laid down as a dither of whole pixels; the
  eraser; a spray; a shading brush that steps pixels along the palette; a
  dither brush; the clone stamp (Alt+click picks its source); a pattern stamp
  (any selection makes the pattern); smudge, and blur (which can also
  sharpen), both able to keep to the colors already there; and a brush that replaces the background color with
  the foreground.
- Shapes: line, Bezier curve, rectangle, ellipse, polygon and contour (a
  freehand shape filled on release), filled or outlined. Lines and curves can
  be pixel-perfect.
- Fill: the bucket, and linear, radial or conic gradients that step between
  two colors or along the palette, broken up by a Bayer matrix, blue noise or
  a pattern instead of blending. Escape stops a gradient that is still
  filling in and puts the layer back.
- Select: rectangle, ellipse, lasso, polygon lasso and magic wand (tolerance,
  contiguous or not, one layer or all, with or without diagonals). Shift
  adds and Alt subtracts; with every selection tool but the rectangle,
  Shift+Alt intersects.
- Transform: free transform (Ctrl+T), described below.
- Other: eyedropper, move, crop, text in bitmap fonts (pixl's own 5x7 font or
  any BDF font), measure, and slices.

## Tool options

The tool options bar holds the size, the brush shape (round, square, or a
custom brush made from the selection), the stroke stabilizer, pixel-perfect
(kept per tool) and symmetry (left and right, top and bottom, both, or
radial with any number of copies).

- `[` and `]` step the brush size.
- X swaps the foreground and background colors; the secondary button paints
  with the background color.
- Shift snaps lines to clean pixel steps and shapes to squares and circles.
- The pen's eraser end erases.

## Preview before you click

Before you click, the canvas shows see-through what the tool will do where
the pointer rests: the brush footprint at its size and shape for the pencil,
shapes and sized brushes, the eraser, smudge and blur footprint in the
theme's ghost color, and the area the bucket would fill. A shape shows
see-through while you drag it. On very large documents the bucket marks only
the pixel under the pointer. The theme's `--tool-ghost` token sets the ghost
color, and its alpha sets how see-through every preview draws.

## Dynamics

The brush tools (pencil, eraser, soft brush, shading, dither, clone and
pattern stamps, smudge, blur and replace color) have a Dynamics button in the
tool options. Its popup sets what pen pressure and pointer speed do to a
stroke:

- Size runs between two percentages of the tool's size.
- Angle runs between two angles. Square and custom brushes turn with
  RotSprite, so they keep clean one-pixel edges.
- Color (pencil only) runs from the background to the foreground color,
  blended or dithered so only those two colors appear, which keeps an indexed
  image on its palette.

Each input has a threshold below which it reads as nothing, one from which it
reads as full, and a curve between. A mouse presses at full pressure, so
mouse strokes use speed. Each tool keeps its own dynamics; the pencil and the
eraser start with pressure setting the size. The Settings > Pen options of
earlier versions became the pencil's and eraser's dynamics.

## Brushes, presets and the quick bar

The Brushes panel is a library of named brushes. Each keeps a shape, size,
soft brush opacity and hardness, dithering, pixel-perfect, the stabilizer
and dynamics. Click one to use it. Save keeps the current brush under the
name typed beside it, Selection makes a brush of the selected pixels, PNG...
makes one from an image's opaque pixels, and Import... and Export... share a
`.pixlbrushes` pack.

The Presets button at the end of the tool options keeps a tool's whole set of
options under a name and restores them, tool included, with a click. Beside
it, the quick bar holds the colors and brushes you painted with last.

Brushes, presets and the quick bar are kept in pixl's data folder, and the
command palette lists the brushes and presets too.

## Selections

The Select menu selects everything, nothing, the inverse, every pixel of the
foreground color, or the layer's content. It grows, shrinks or borders the
selection by a number of pixels, and saves selections to load back while the
document is open, replacing, adding to, subtracting from or intersecting the
current one.

## Free transform

Ctrl+T lifts the selection, or the whole layer, into a box with handles:

- Drag inside to move.
- Drag a corner or an edge to scale from the opposite side. Shift keeps the
  proportions, Alt scales from the middle, Ctrl on an edge skews.
- Drag outside to turn (Shift steps 15 degrees), or type an angle.
- Flip H and Flip V mirror it.

The preview resamples with RotSprite, clean edge or nearest neighbor. Enter
applies it as one undo step; Escape puts the pixels back. Large boxes fill in
tile by tile, and Enter waits for the last tiles.

## Clipboard

Copy and paste go through the system clipboard, so images move to and from
other programs.

- Copy (Ctrl+C) copies the selection of the active cel, or the whole cel
  with nothing selected. Cut (Ctrl+X) also clears it. Copy merged
  (Ctrl+Shift+C) copies what all the visible layers show.
- Paste (Ctrl+V) floats the image in the free transform box, where it was
  copied from when that place is in view, otherwise in the middle of the
  view. Enter keeps it as one undo step; Escape drops it.
- Paste as new layer (Ctrl+Shift+V) and Paste as new document (Ctrl+Alt+V).

Between pixl documents a copy keeps its exact pixels and its place. An
indexed copy keeps its palette indices when pasted on the same palette;
anything else pasted into an indexed document lands on the nearest palette
color. An Image or Filter command chosen while the paste box is open applies
the box first, as its own undo step.

## Image menu

- Crop to the selection, set the canvas size around an anchor, and resize the
  image with nearest, RotSprite or clean-edge sampling, so no new colors
  appear.
- Flip and turn the canvas.
- Convert to indexed color, on the current palette or one made from the
  image, with no dither, a Bayer matrix or Floyd-Steinberg; or back to RGBA.
- Replace one color with another in this frame or every frame.
- Load and save palettes as `.gpl`, `.pal`, `.hex` and `.ase`; sort the
  palette by hue, saturation, lightness or brightness; build a ramp around
  the foreground color with its hue turning from shadow to highlight; make a
  palette from the image; lock the painting colors to the palette.

Indexed conversion, palette from image and image resize run in the
background. The dialog stays open showing progress until the result lands as
one undo step; Cancel or Escape stops it.

## Filter menu

Hue and saturation, brightness and contrast, levels, curves, color balance,
posterize, invert, desaturate, gradient map, outline (inside or outside,
with or without diagonals), drop shadow, noise and pixelate.

Each filter previews on the canvas as its settings change and applies as one
undo step; Cancel puts the layer back. Large layers fill in tile by tile from
the middle of the view. Indexed layers stay on their palette.
