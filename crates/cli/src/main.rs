//! `pixl-cli`: exports pixl, Aseprite, PNG and GIF documents without the
//! editor, for build pipelines. It reads Aseprite's batch flags, so a
//! pipeline written for `aseprite -b` runs by changing the program name,
//! and runs the export presets the editor keeps.
//!
//! Exit codes: 0 when everything was written, 1 when an export failed
//! (a file could not be read or written, a layer, tag or preset does not
//! exist), 2 when the command line cannot be read.

mod args;
mod run;

use std::io::Write;
use std::process::ExitCode;

const HELP: &str = "\
pixl-cli: export pixl, Aseprite, PNG and GIF documents without the editor

Usage: pixl-cli [OPTIONS] DOCUMENT

DOCUMENT is a .pixl, .aseprite, .ase, .png or .gif file. Options read in any
order; the Aseprite ones mean what they mean to `aseprite -b`.

Output:
  --save-as FILE         PNG frames or a GIF; FILE takes tokens (below)
  --sheet FILE           a sprite sheet PNG
  --data FILE            the sheet's JSON, in Aseprite's shape
  --format FORMAT        json-hash (default) or json-array
  --sheet-type TYPE      horizontal (default), vertical, rows, columns, packed
  --sheet-pack           the same as --sheet-type packed
  --sheet-columns N      columns of a rows sheet
  --sheet-rows N         rows of a columns sheet
  --border-padding N     transparent pixels around the sheet
  --shape-padding N      transparent pixels between sheet frames
  --trim                 crop each sheet frame to its pixels

What to export:
  --layer NAME           only this layer (repeat for more; group/layer paths)
  --ignore-layer NAME    leave this layer out (repeatable)
  --all-layers           hidden layers too
  --split-layers         each layer on its own
  --split-tags           each tag to its own --save-as file
  --tag NAME             only the frames of this tag
  --frame-range A,B      only frames A to B, counted from 0
  --oneframe             only the first frame
  --scale N              enlarge by a whole factor, 1 to 64

Presets (the editor's File > Export presets):
  --preset NAME          run this preset (repeatable)
  --all-presets          run every preset marked for Export all
  --presets FILE         read presets from FILE, not the editor's exports.json
  --list-presets         print the presets' names

Lists, one name per line on standard output:
  --list-layers          the document's layers
  --list-tags            the document's tags

Other:
  -b, --batch            accepted for Aseprite pipelines; every run is a batch
  -v, --verbose          name each file written, on standard error
  -h, --help             this text
      --version          the version

File name tokens: {name} or {title} the document's name, {tag}, {layer},
{scale}, {frame} (from 0, padded to the last frame's digits), {frame1} or
{frame001} (from that number, padded to its digits), {tagframe} (counted from
the first frame written). Presets write beside the document; --save-as and
--sheet write where FILE says.

Exit codes: 0 written, 1 an export failed, 2 the command line is wrong.
";

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let args = match args::parse(argv) {
        Ok(a) => a,
        Err(args::Usage(e)) => {
            let _ = writeln!(
                stderr,
                "pixl-cli: {e}\nRun pixl-cli --help for the options."
            );
            return ExitCode::from(2);
        }
    };
    if args.help {
        let _ = stdout.write_all(HELP.as_bytes());
        return ExitCode::SUCCESS;
    }
    if args.version {
        let _ = writeln!(stdout, "pixl-cli {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    match run::run(&args) {
        Ok(outcome) => {
            for line in &outcome.err {
                let _ = writeln!(stderr, "pixl-cli: {line}");
            }
            for line in &outcome.out {
                let _ = writeln!(stdout, "{line}");
            }
            if args.verbose {
                let _ = writeln!(stderr, "pixl-cli: {} files written", outcome.written);
            }
            ExitCode::SUCCESS
        }
        Err(run::Failure::Usage(e)) => {
            let _ = writeln!(
                stderr,
                "pixl-cli: {e}\nRun pixl-cli --help for the options."
            );
            ExitCode::from(2)
        }
        Err(run::Failure::Export(e)) => {
            let _ = writeln!(stderr, "pixl-cli: {e}");
            ExitCode::from(1)
        }
    }
}
