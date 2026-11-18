# pixl

A pixel art editor for sprites, animation, tile work and character rigs.

## Why pixl

pixl puts what a pixel artist reaches for in Aseprite and Photoshop into one
tool: layers, frames and a timeline, tools that respect the pixel grid, and
transforms that keep clean one-pixel edges. It opens Aseprite files, exports sprite sheets that engines already read, and runs the same exports
from the command line for build pipelines. It is built on
[Lumen](https://github.com/lumen-fx/lumen) and is its flagship Rust SDK app.

## Download

Download the latest build from the
[releases page](https://github.com/lumen-fx/pixl/releases/latest). Each package holds
pixl and the files it reads at start; keep them together.

- Windows (x86_64): unzip `pixl-<version>-windows-x86_64.zip` and run
  `pixl.exe`. `pixl-cli.exe` beside it is the command line exporter.
- Linux (x86_64): unpack `pixl-<version>-linux-x86_64.tar.gz` and run `pixl`.
  It needs GTK 3, which desktop distributions install by default.
- macOS (Apple silicon): unzip `pixl-<version>-macos-arm64.zip` and move
  `pixl.app` to Applications.

The builds are not signed. On Windows, SmartScreen asks before `pixl.exe`
runs the first time. On macOS, allow `pixl.app` under System Settings >
Privacy & Security > Open Anyway, or run
`xattr -dr com.apple.quarantine pixl.app` once.

## Quick start

Run pixl. The start screen lists your recent documents, the rig demo and the
Getting started tutorial; pick New project to begin. The Help menu has short
tutorials for layers, selections, animation, palettes and rigging, each on a
document of its own.

Ctrl+Shift+P opens the command palette, which finds any command, tool or
document by name and shows its shortcut.

## What it does

- Pixel tools: a pixel-perfect pencil, brushes with pressure and speed
  dynamics, shading along the palette, dithered fills and gradients, shapes,
  selections and text in bitmap fonts. The canvas previews what a tool will
  do before you click.
- Transforms for pixel art: free transform, resize and brush rotation use
  RotSprite or clean-edge sampling, so no stray colors appear.
- Layers: groups, blend modes, masks, clipping and high-resolution reference
  images.
- Animation: a timeline of layers against frames, linked cels, tags,
  per-frame timing, onion skin and a live preview.
- Tilemaps: draw levels and patterns from a shared tileset, in the same
  shape as Aseprite's tilemap layers.
- Rigging: bind parts of a character to a skeleton, pose it, and render the
  poses into ordinary frames to touch up.
- Color: RGBA and indexed documents, palette import and export, ramps,
  palettes made from the art, and filters that stay on the palette.
- Files: `.pixl`, Aseprite, PNG and GIF, sprite sheets with Aseprite-style
  JSON, and named export presets.
- Workspace: several documents in tabs, dockable panels, saved layouts,
  themes, and keyboard shortcuts in pixl's, Aseprite's or Photoshop's scheme.
- Scripting: automate edits and exports with
  [candela](https://github.com/lumen-fx/candela) scripts and extensions.
- `pixl-cli` exports documents without the editor and accepts Aseprite's
  batch flags.

## Known limitations

- pixl is in early development.
- Builds are unsigned, and macOS builds are for Apple silicon only.
- `pixl-cli` ships only in the Windows package; on Linux and macOS, build it
  from source.
- pixl saves documents as `.pixl`; it opens Aseprite files but does not
  save them.
- Aseprite's Lua scripts do not run in pixl.

## Documentation

- [Editor](docs/editor.md): start screen, tabs, the canvas view, panels and
  layouts, color, settings and themes.
- [Tools](docs/tools.md): the tools, brushes, selections, clipboard,
  transforms, and the Image and Filter menus.
- [Layers, animation and rigs](docs/layers-and-animation.md): layers,
  tilemaps, the timeline, playback and skeletons.
- [Files and export](docs/files.md): file formats, sprite sheets and export
  presets.
- [Scripting](docs/scripting.md): scripts, extensions and the API.
- [Command line](docs/command-line.md): `pixl-cli` and its Aseprite-compatible
  flags.
- [Development](docs/development.md): building from source, the source tree,
  tests and benchmarks.

## License

MIT, see [LICENSE](LICENSE). The candela standard library modules in
`crates/script/candela/std` are candela's, under the Apache License 2.0; see
`crates/script/candela/LICENSE` and `NOTICE`. The UI font is Pixelify Sans,
under the SIL Open Font License (`app/assets/fonts/OFL.txt`).
