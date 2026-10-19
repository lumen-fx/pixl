//! The command line: Aseprite's batch flags, read in any order, and pixl's
//! own preset flags.

use std::path::PathBuf;

use pixl_io::export::Sheet;
use pixl_io::sheet::JsonFrames;

/// What the command line asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub help: bool,
    pub version: bool,
    pub verbose: bool,
    pub input: Option<PathBuf>,
    pub save_as: Option<String>,
    pub sheet: Option<String>,
    pub data: Option<String>,
    pub json_frames: JsonFrames,
    pub sheet_type: Option<Sheet>,
    pub columns: Option<u32>,
    pub rows: Option<u32>,
    pub border: u32,
    pub padding: u32,
    pub layers: Vec<String>,
    pub ignore_layers: Vec<String>,
    pub all_layers: bool,
    pub split_layers: bool,
    pub split_tags: bool,
    pub tag: Option<String>,
    pub frame_range: Option<(usize, usize)>,
    pub oneframe: bool,
    pub scale: Option<u32>,
    pub trim: bool,
    pub list_tags: bool,
    pub list_layers: bool,
    pub presets: Vec<String>,
    pub all_presets: bool,
    pub presets_file: Option<PathBuf>,
    pub list_presets: bool,
    pub script: Option<PathBuf>,
    /// `--script-param key=value`, in order.
    pub script_params: Vec<(String, String)>,
}

/// Aseprite flags pixl-cli knows and does not do; naming one is an error
/// rather than an export that quietly differs.
const UNSUPPORTED: [&str; 24] = [
    "--shell",
    "--filename-format",
    "--trim-sprite",
    "--trim-by-grid",
    "--crop",
    "--slice",
    "--split-slices",
    "--split-grid",
    "--ignore-empty",
    "--merge-duplicates",
    "--inner-padding",
    "--extrude",
    "--sheet-width",
    "--sheet-height",
    "--list-layer-hierarchy",
    "--list-slices",
    "--color-mode",
    "--dithering-algorithm",
    "--dithering-matrix",
    "--palette",
    "--shrink-to",
    "--tagname-format",
    "--play-subtags",
    "--export-tileset",
];

/// A command line pixl-cli cannot read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage(pub String);

fn number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, Usage> {
    value
        .trim()
        .parse()
        .map_err(|_| Usage(format!("{flag} takes a whole number, not \"{value}\"")))
}

/// Reads the arguments after the program name.
pub fn parse(argv: impl IntoIterator<Item = String>) -> Result<Args, Usage> {
    let mut a = Args::default();
    let mut it = argv.into_iter();
    let mut inputs = Vec::new();
    while let Some(arg) = it.next() {
        if !arg.starts_with('-') || arg == "-" {
            inputs.push(PathBuf::from(arg));
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let value = |it: &mut dyn Iterator<Item = String>| -> Result<String, Usage> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => it
                    .next()
                    .ok_or_else(|| Usage(format!("{flag} needs a value"))),
            }
        };
        match flag.as_str() {
            "-h" | "--help" => a.help = true,
            "--version" => a.version = true,
            "-v" | "--verbose" => a.verbose = true,
            // Every run is a batch run; the flag is there for pipelines
            // written for Aseprite.
            "-b" | "--batch" => {}
            "--save-as" => a.save_as = Some(value(&mut it)?),
            "--sheet" => a.sheet = Some(value(&mut it)?),
            "--data" => a.data = Some(value(&mut it)?),
            "--format" => {
                a.json_frames = match value(&mut it)?.as_str() {
                    "json-hash" => JsonFrames::Hash,
                    "json-array" => JsonFrames::Array,
                    other => {
                        return Err(Usage(format!(
                            "--format is json-hash or json-array, not \"{other}\""
                        )));
                    }
                }
            }
            "--sheet-type" => {
                a.sheet_type = Some(match value(&mut it)?.as_str() {
                    "horizontal" => Sheet::Horizontal,
                    "vertical" => Sheet::Vertical,
                    "rows" => Sheet::Grid,
                    "columns" => Sheet::Columns,
                    "packed" => Sheet::Packed,
                    other => {
                        return Err(Usage(format!(
                            "--sheet-type is horizontal, vertical, rows, columns or packed, not \"{other}\""
                        )));
                    }
                })
            }
            "--sheet-pack" => a.sheet_type = Some(Sheet::Packed),
            "--sheet-columns" => a.columns = Some(number(&flag, &value(&mut it)?)?),
            "--sheet-rows" => a.rows = Some(number(&flag, &value(&mut it)?)?),
            "--border-padding" => a.border = number(&flag, &value(&mut it)?)?,
            "--shape-padding" => a.padding = number(&flag, &value(&mut it)?)?,
            "--layer" => a.layers.push(value(&mut it)?),
            "--ignore-layer" => a.ignore_layers.push(value(&mut it)?),
            "--all-layers" => a.all_layers = true,
            "--split-layers" => a.split_layers = true,
            "--split-tags" => a.split_tags = true,
            "--tag" | "--frame-tag" => a.tag = Some(value(&mut it)?),
            "--frame-range" => {
                let v = value(&mut it)?;
                let (from, to) = v
                    .split_once(',')
                    .ok_or_else(|| Usage(format!("--frame-range takes from,to, not \"{v}\"")))?;
                let (from, to): (usize, usize) = (number(&flag, from)?, number(&flag, to)?);
                if to < from {
                    return Err(Usage(format!("--frame-range {v} ends before it starts")));
                }
                a.frame_range = Some((from, to));
            }
            "--oneframe" => a.oneframe = true,
            "--scale" => {
                let v = value(&mut it)?;
                let scale: u32 = v
                    .trim()
                    .strip_suffix(".0")
                    .unwrap_or(v.trim())
                    .parse()
                    .ok()
                    .filter(|s| (1..=64).contains(s))
                    .ok_or_else(|| {
                        Usage(format!(
                            "--scale takes a whole factor from 1 to 64, not \"{v}\": pixel art scales by whole pixels"
                        ))
                    })?;
                a.scale = Some(scale);
            }
            "--trim" => a.trim = true,
            "--list-tags" => a.list_tags = true,
            "--list-layers" => a.list_layers = true,
            "--preset" => a.presets.push(value(&mut it)?),
            "--all-presets" => a.all_presets = true,
            "--presets" => a.presets_file = Some(PathBuf::from(value(&mut it)?)),
            "--list-presets" => a.list_presets = true,
            "--script" => a.script = Some(PathBuf::from(value(&mut it)?)),
            "--script-param" => {
                let v = value(&mut it)?;
                let (k, val) = v
                    .split_once('=')
                    .ok_or_else(|| Usage(format!("--script-param takes key=value, not \"{v}\"")))?;
                a.script_params.push((k.to_string(), val.to_string()));
            }
            f if UNSUPPORTED.contains(&f) => {
                return Err(Usage(format!("pixl-cli does not support {f}")));
            }
            f => return Err(Usage(format!("unknown option {f}"))),
        }
    }
    if inputs.len() > 1 {
        return Err(Usage(
            "pixl-cli reads one document at a time; run it once per file".into(),
        ));
    }
    a.input = inputs.pop();
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Result<Args, Usage> {
        parse(line.split_whitespace().map(str::to_string))
    }

    #[test]
    fn aseprite_flags_read_in_any_order() {
        let a = args(
            "-b --split-layers hero.aseprite --sheet out.png --data=out.json --format json-array --sheet-type rows --sheet-columns 4 --scale 2 --tag walk",
        )
        .unwrap();
        assert_eq!(a.input, Some(PathBuf::from("hero.aseprite")));
        assert!(a.split_layers);
        assert_eq!(a.sheet.as_deref(), Some("out.png"));
        assert_eq!(a.data.as_deref(), Some("out.json"));
        assert_eq!(a.json_frames, JsonFrames::Array);
        assert_eq!(a.sheet_type, Some(Sheet::Grid));
        assert_eq!(a.columns, Some(4));
        assert_eq!(a.scale, Some(2));
        assert_eq!(a.tag.as_deref(), Some("walk"));
    }

    #[test]
    fn a_bad_command_line_says_what_is_wrong() {
        assert!(
            args("--frame-range 3,1 a.pixl")
                .unwrap_err()
                .0
                .contains("before")
        );
        assert!(args("--scale 1.5 a.pixl").unwrap_err().0.contains("whole"));
        assert_eq!(
            args("--trim-sprite a.pixl").unwrap_err().0,
            "pixl-cli does not support --trim-sprite"
        );
        assert!(args("--nope").unwrap_err().0.contains("unknown option"));
        assert!(
            args("a.pixl b.pixl")
                .unwrap_err()
                .0
                .contains("one document")
        );
        assert!(
            args("a.pixl --save-as")
                .unwrap_err()
                .0
                .contains("needs a value")
        );
        assert_eq!(args("--scale 3.0 a.pixl").unwrap().scale, Some(3));
        assert_eq!(args("--frame-range 1,4").unwrap().frame_range, Some((1, 4)));
        assert!(
            args("--script s.cdl --script-param nope")
                .unwrap_err()
                .0
                .contains("key=value")
        );
    }

    #[test]
    fn script_flags_read_like_aseprites() {
        let a =
            args("hero.pixl --script-param color=red --script outline.cdl --script-param n=2=3")
                .unwrap();
        assert_eq!(a.script, Some(PathBuf::from("outline.cdl")));
        assert_eq!(
            a.script_params,
            [
                ("color".to_string(), "red".to_string()),
                ("n".to_string(), "2=3".to_string())
            ]
        );
    }
}
