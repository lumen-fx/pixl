# Scripting

Scripts automate what would take many clicks: recolor every frame, build a
palette from the art, export each layer for an engine, generate a document
from data. pixl runs scripts written in
[candela](https://github.com/lumen-fx/candela), a small statically typed
language, against an API of documents, layers, cels, frames, tags, the
palette, the selection and tilesets. Aseprite's Lua scripts do not run in
pixl.

```rust
// Outline the active layer on every frame, in the color the run is given.
import "pixl" as app;

fn main() {
    let doc = app::document();
    let color = app::hex_color(app::param("color", "#000000"));
    for cel in doc.active_layer().cels() {
        if !cel.is_empty() {
            cel.outline(color, true);
        }
    }
}
```

## Running scripts

File > Scripts lists the `.cdl` files in pixl's `scripts` folder (and one
folder down), the examples pixl ships, and the commands of installed
extensions. Run runs the one picked, Rescan reads the folder again, Open
folder shows it in the file manager, and Run a script file... runs any `.cdl`
file. Every script and extension command is in the command palette too.

A run works on the open documents and shows what it printed when it ends.
Each document it changed takes the whole run as one undo step, named after
the script; documents it creates or opens appear in tabs of their own. A run
that raises an error changes no document and names the error, with the line
when the error is in the script itself.

A long run shows a Stop button, which stops the script at its next call into
pixl. A stopped script that makes no call within a second is let go: it runs
on in the background and changes nothing.

Relative paths a script names are read from the folder of the document in
front (from the scripts folder when it has no file), and in `pixl-cli` from
the working folder. To run scripts without the editor, see
[Command line](command-line.md).

## Extensions

An extension is a folder in pixl's `extensions` folder with an
`extension.json` naming the commands it adds, each a script with the
parameters it runs with, and any palettes it brings:

```json
{
  "name": "Outlines",
  "version": "1.0.0",
  "description": "Outline art in black",
  "commands": [
    { "id": "black", "label": "Outline in black", "script": "outline.cdl",
      "params": { "color": "#000000" } }
  ],
  "palettes": [ { "name": "Ink", "file": "ink.gpl" } ]
}
```

Install extension... in the Scripts dialog takes a `.zip` or
`.pixl-extension` holding that folder and asks before unpacking it. Remove
extension deletes the one picked.

## Trust

Scripts are programs. candela can read and write files and call native
libraries, so a script or extension can do anything a program you run can.
pixl does not ask before a script touches a file, and a script that calls
`exit` with a nonzero code ends pixl. Run scripts only from sources you
trust.

## API reference

The API is documented in
[`crates/script/src/pixl.cdl`](../crates/script/src/pixl.cdl), the module
scripts import. The examples pixl ships are in
[`crates/script/scripts`](../crates/script/scripts).

candela's standard library modules that need no native library (`list`,
`map`, `set`, `string`, `option`, `result`, `json`, `convert`, `assert`)
import as usual; `math`, `time`, `random` and `hash` are not available.
