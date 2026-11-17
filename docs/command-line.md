# Command line

`pixl-cli` exports documents without the editor, for game build pipelines
and CI machines with no display or GPU. It writes PNG frames, GIFs, and
sprite sheets with Aseprite-style JSON from `.pixl`, `.aseprite`, `.ase`,
`.png` and `.gif` files, through the same code as the editor's exports. It
reads Aseprite's batch flags, so a pipeline built on `aseprite -b` switches by
changing the program name. It also runs the editor's export presets and
candela scripts.

```sh
pixl-cli -b hero.aseprite --sheet hero.png --data hero.json --sheet-type packed --trim
pixl-cli hero.pixl --split-tags --save-as build/hero.gif --scale 4
pixl-cli hero.pixl --tag walk --save-as 'frames/{title}-{frame001}.png'
pixl-cli hero.pixl --split-layers --sheet layers.png --data layers.json
pixl-cli hero.pixl --preset "Sprite sheet"     # one of the editor's presets
pixl-cli hero.pixl --all-presets               # what File > Export all runs
pixl-cli hero.pixl --list-tags
pixl-cli hero.pixl --script outline.cdl --script-param color=#202020 --save-as out.png
pixl-cli --script make-tiles.cdl              # a script that makes its own documents
```

`pixl-cli --help` lists every option.

## Exit codes

- 0: everything was written.
- 1: an export or a script failed: a file could not be read or written, a
  layer, tag or preset does not exist, or the script raised an error.
- 2: the command line is wrong. The reason goes to standard error.

Output has no colors. `-v` names each file written, on standard error.

## Presets

`--preset NAME` and `--all-presets` read the editor's `exports.json` (a new
user's starter presets when there is none), or the file `--presets FILE`
names, and write beside the document as they do in the editor. `--save-as`
and `--sheet` write where they say, relative to the working folder. See
[Export presets](files.md#export-presets).

## Scripts

`--script FILE` runs a candela script with the document as its
`app::document()`; the export flags then export what the script left.
`--script-param K=V` passes a value the script reads with `app::param`. A
script's prints go to standard output and its `app::alert`s to standard
error, and nothing is saved unless the script saves. See
[Scripting](scripting.md).

## Aseprite flags

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
| `--script FILE` | runs a candela script (`.cdl`) |
| `--script-param K=V` | same |

A `--save-as` name without tokens gets the ones Aseprite adds: ` ({layer})`
when layers split, ` #{tag}` when tags split, and the frame number when each
frame is a file, counting on from a number the name ends in, else from 1.
Sheet JSON names its frames `{title} {frame}`, without Aseprite's file
extension.

## Limitations

- One document per run, so an atlas of several sprites takes one sheet per
  sprite.
- Aseprite options pixl-cli does not do stop the run with exit code 2 rather
  than export something different. They include `--shell`,
  `--filename-format`, `--ignore-empty`, `--merge-duplicates`,
  `--inner-padding`, `--crop`, the slice and grid splits, `--extrude`, color
  mode and palette changes, and fractional scales.
- The release packages include `pixl-cli` on Windows only; on Linux and
  macOS, build it from source (see [Development](development.md)).
