# Development

## Building from source

```sh
cargo run -p pixl                 # the start screen
cargo run -p pixl -- art.pixl     # straight into a document
cargo run -p pixl-cli -- --help   # the command line exporter
cargo test
```

On Linux the build links GTK 3, X11, xkbcommon and Wayland, so install their
development packages first.

`app/build.rs` compiles the UI that `app/lumen.toml` names (the markup and
stylesheets under `app/src/`, with every `<include>` and `@import`) into the
executable, so a markup or style mistake fails `cargo build`. It uses the
`lumenc` of the same Lumen release: the one `LUMEN_LUMENC` names, else a
`lumenc` of that version on `PATH`, else a copy it downloads once into the
user cache.

The icons and font in `app/assets/` stay files. pixl reads them from the
folder beside the executable when that folder holds a `lumen.toml`, and from
`app/` in the source tree otherwise. The tool icons are drawn by
`app/assets/icons/draw.py` as masks the theme colors.

## Source tree

- `app` (`pixl`): the editor.
- `crates/document` (`pixl-document`): the document model, with no UI:
  layers, cels, frames, tags, slices, palettes, selections, undo history,
  compositing, transforms, reference images, rigs, tilesets, and the `.pixl`
  format.
- `crates/paint` (`pixl-paint`): the raster tools and filters.
- `crates/io` (`pixl-io`): other programs' formats (Aseprite, GIF, PNG,
  sprite sheets with Aseprite-style JSON) and the export presets.
- `crates/script` (`pixl-script`): the candela API, script runs, the scripts
  folder and extensions. The editor and pixl-cli both run scripts through it.
- `crates/cli` (`pixl-cli`): the command line exporter and script runner.

## Demo and tutorial documents

`app/tests/common/demo.rs` builds the rig demo and the tutorials' documents,
and a test checks that the files in `app/assets/demo/` match. After changing
one of them, or the `.pixl` format, write them again:

```sh
PIXL_WRITE_DEMO=1 cargo test -p pixl --test demo
```

## Performance

`pixl --perf-log frames.csv` writes one row per frame: its start and time,
the tick's work and the editor's part of it, compositing, tiles written,
layer buffers and their memory, the worker queue, and the time from the last
stroke to the screen. F3 shows the same figures in the editor.

The benchmarks run without a window. The `pixl` benches take `gpu` or `cpu`
to pick the renderer.

| Command | Measures |
| --- | --- |
| `cargo bench -p pixl --bench view -- gpu` | what a user waits for on a large layered document: the redraw after opening, a stroke on the top and bottom layer, pan and zoom, the same with a reference image and with a tilemap, hiding a layer, PNG export, and an idle tick |
| `cargo bench -p pixl --bench playback -- gpu` | stepping frames with and without onion skin, and the playback frame rate |
| `cargo bench -p pixl --bench heavy -- gpu` | edits too large for one frame, and the longest tick while each runs |
| `cargo bench -p pixl --bench pose -- gpu` | a pose drag to the frame that shows it |
| `cargo bench -p pixl --bench customize -- gpu` | the per-frame cost of layouts, themes, the command palette and export presets |
| `cargo bench -p pixl-document --bench rig` | posing and rendering a character |
| `cargo bench -p pixl-paint` | brushes, filters, gradients, palette and indexed conversion, transform resampling, selection edits, and compositing |

The view bench takes `side=N` and `layers=N` to change the document,
`budget=MiB` for the layer buffer memory, `overlay` to show the performance
overlay, `log=FILE` for the per-frame CSV, and `systems` to list what each
editor system costs. `playback` takes `sprite` or `full` and `frames=N`.
Numbers move with the load on the machine, so compare runs taken under the
same load.

### Performance gate

`cargo bench -p pixl --bench view -- cpu gate` (or `gpu gate`) checks the
run against the limits in `app/benches/gate.json` and fails when a median
passes its limit, naming the editor systems that cost the most there. The
limits leave room above what a desktop measures, to catch regressions rather
than grade the machine. Run the gate locally before you push.

`gate report` prints the same numbers without failing. CI runs
`cpu gate report`, because its runners are slower than the desktop the
limits were set on.

## Releases

A release starts from a manual run of the `release` workflow on main
(`gh workflow run release.yml -f version=X.Y.Z`), typing back the version in
the workspace `Cargo.toml`. It waits for CI on that commit, tags it, builds
the Windows, Linux and macOS packages, and publishes them as one
release, marked as the latest. Pushing a `v*` tag that matches the workspace version releases
too.

Each package holds the executable, `lumen-crash` (which writes a report and
shows a dialog if pixl crashes), `lumen.toml`, `LICENSE`, and
`assets/icons` and `assets/fonts`; the Windows zip also holds
`pixl-cli.exe`. On macOS the executable folder is `pixl.app/Contents/MacOS`,
where `lumen.toml` and `assets` link to `Contents/Resources`, and `LICENSE`
sits beside `pixl.app`.
