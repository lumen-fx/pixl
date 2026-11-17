# Layers, animation and rigs

## Layers

The timeline below the canvas lists the layers, one row per layer against
the frames, as in Aseprite. Each row has the layer's eye, a lock button, its
name and, for a group, a fold button. New, Delete and Props above the rows
add a layer, remove the active one and open its properties.

The Layers panel lists the same layers with their color tags, clipping and
mask marks, the active layer's opacity and blend mode, and buttons to group,
copy and reorder layers. The built-in layouts hide it; show it from Window >
Panels and layouts. Clicks and drags work the same in both lists.

- Groups: a new layer goes into the active group. Drag a row onto another to
  place it above or below, or onto a group's middle to put it inside.
- Masks hide the layer where they are dark. Add mask starts painting on the
  mask; the mask button in the Layers panel or Layer > Edit mask or pixels
  switches back. A mask can be turned off, applied or deleted.
- Clipping shows a layer only where the layer below it does.
- Properties (double-click a row): name, opacity, blend mode, color tag,
  clipping, locks and the mask.

The Layer menu, also on a right-click on a row, duplicates, merges down,
flattens, clips, and locks pixels, alpha or position.

### Visibility and selecting layers

- Click an eye to show or hide its layer; a hidden group hides its contents.
- Alt+click an eye to show only that layer; Alt+click again to restore.
- Drag down the column of eyes to set each layer you pass the way the first
  went.
- Ctrl+click a row to add or remove it from the selection; Shift+click
  selects a range. The tools paint on the last row clicked.
- Layer > Show only selected, Show all layers and Hide selected act on the
  selection.

Each visibility change is an undo step.

## Reference images

Layer > Reference image adds any image pixl opens as a reference. It keeps
its full resolution and draws smoothly over the canvas, not on the pixel
grid, so a photo stays sharp at any zoom. It arrives at half opacity in the
middle of the canvas, scaled down to fit, and sits in the layer stack like
any layer.

While its layer is active, a box with handles surrounds it and every tool
except the eyedropper places it: drag inside to move, a corner to scale
keeping proportions (Shift frees them), an edge to stretch, or outside to
turn (Shift steps 15 degrees). Alt scales from the middle. The layer's
properties set its center, size and angle by number.

A reference is never painted on or exported, and the tools read the art
without it.

## Tilemaps

A tilemap layer draws from a tileset: a list of tiles of one size, numbered
from 1, with tile 0 empty. Each cell of the layer's grid shows one tile,
plain or flipped, so a level or pattern is drawn once and placed many times.
Tilesets are shared, so several layers can draw from one.

- Layer > New tilemap layer makes one, with a new tileset or one the document
  already has.
- Layer > Convert to tilemap cuts a layer into tiles, one per distinct cell.
  Convert to normal layer turns it back into pixels.

On a tilemap layer, Tab (or the button at the left of the tool options)
switches between Pixels and Tiles mode.

- Tiles mode: the pencil and brush place the picked tile in each cell, the
  eraser and secondary button empty cells, the bucket fills joined cells
  holding the same tile, the rectangle fills a block, and the eyedropper
  picks a tile with its flips.
- Pixels mode: tools paint as on any layer. When a stroke or paste ends, the
  draw mode decides what happens to the tileset. Manual changes the tiles
  under the stroke everywhere they appear. Auto reuses a matching tile,
  edits a tile in place when that cell is its only use, otherwise adds one,
  and removes tiles left unused. Stack reuses or adds tiles and never changes
  or removes one.

The Tileset panel shows the active tilemap's tiles, the empty one first. A
click picks a tile and switches to Tiles mode. Earlier and Later move it,
Copy duplicates it, Delete removes it and empties its cells, and Unused
removes every tile no cell shows. Moving tiles never changes what the map
shows.

## Timeline

The timeline shows every layer against every frame, with the document's tags
above the frame numbers. A cel shows whether it holds pixels and joins its
neighbors when they are linked. Frames a rig rendered are marked.

- Click a cel or frame number to go there; drag or Shift+click to select a
  range.
- Add empty frames, copy, delete and reverse frames, link selected cels to
  the first or unlink them, clear cels, and set frame durations. Drag frame
  numbers to move frames.
- Tags are made from selected frames and edited in their dialog (name,
  frames, color, direction, repeats) or by dragging their ends or middle.
- Right-click a cel, frame number or tag for its menu.
- `,` and `.` (or Alt+Left and Alt+Right) step frames; Alt+Up and Alt+Down
  step layers.

## Playback and onion skin

Play runs the animation at each frame's duration, looping, once or
ping-pong. With Loop tag on it plays inside the current frame's tag, with
the tag's direction and repeats. The frame rate reached shows beside the
controls; a document too heavy to keep up plays slower rather than skipping
frames.

The onion skin shows frames before and after the current one, fading with
distance, tinted by the theme, under or over the current frame. It steps
aside during playback.

The preview panel shows the document at actual size or fitted, and plays
along. The Animate layout shows it beside the timeline.

## Rigs

A skeleton layer rigs a character so you pose it instead of redrawing it:
bones in a tree, with layers or selected regions of them bound to one bone
each as rigid parts. Rendering a pose draws every part at its bone's place
with RotSprite, so turned parts keep clean outlines, into frames of ordinary
pixels you can touch up.

The Rig panel works on the document's skeleton; New skeleton makes one. The
line under the panel's header says what a drag does in the current mode.

- Bones mode: drag on the canvas to draw a bone, joined to the selected bone;
  start on the selected bone's end to join it there. Drag a joint or a
  binding's pivot to move it; Shift moves the selected bone's end. The panel
  renames and reparents bones and binds the active layer, or its selected
  pixels, to the selected bone. The rig reads its parts from the rest frame,
  which Use this frame sets.
- Pose mode: drag a bone to turn it and its children, drag its head to move
  it, and with IK on drag a bone's end to bend it with its parent. Shift
  snaps turns to 15 degrees. Posing any frame but the rest frame draws and
  keys the pose there.

Poses save to a library with thumbnails, mirror left to right by bone name
(`arm.L` and `arm.R`, `leg_l` and `leg_r`), and reset to rest. Each key has
an easing toward the next. Render to new frame adds a frame with the pose,
Render keyed fills a range of frames from the keys, and Regenerate renders
every generated frame again after the rig changes.

Bones draw over the art in the theme's colors and are never part of it.
Escape leaves either mode, and picking a tool hands the canvas back. Outside
the Rig layout the panel stays folded to its header until a mode starts.
