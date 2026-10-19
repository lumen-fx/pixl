//! Turning a command line into export presets and running them through
//! `pixl_io::export`, the code the editor's exports run.

use std::path::{Path, PathBuf};

use pixl_document::Document;
use pixl_io::export::{
    self, Data, FILE, Format, Frames, Layers, Preset, PresetFile, names_frame, plan, run_job,
    tokens,
};
use pixl_io::open::{is_imported, read_document};
use pixl_script::{Library, ScriptDoc, Session, run_file};

use crate::args::Args;

/// Why a run failed: the command line (exit code 2), the export (1), or a
/// script (1), with what the script printed before it stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Usage(String),
    Export(String),
    Script {
        out: Vec<String>,
        err: Vec<String>,
        error: String,
    },
}

/// What a run printed and wrote.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Lines for standard output (the lists).
    pub out: Vec<String>,
    /// Lines for standard error (warnings, and with `--verbose` each file).
    pub err: Vec<String>,
    pub written: usize,
}

fn usage(text: impl Into<String>) -> Failure {
    Failure::Usage(text.into())
}

/// The document's name: its file name without the extension.
fn title(path: &Path) -> String {
    path.file_stem().map_or_else(
        || "sprite".to_string(),
        |s| s.to_string_lossy().into_owned(),
    )
}

/// The frames the command line selects, before `--split-tags`.
fn frames(args: &Args, doc: &Document) -> Result<Frames, Failure> {
    let chosen = [
        args.tag.is_some(),
        args.frame_range.is_some(),
        args.oneframe,
    ];
    if chosen.iter().filter(|c| **c).count() > 1 {
        return Err(usage(
            "choose one of --tag, --frame-range and --oneframe".to_string(),
        ));
    }
    if let Some(tag) = &args.tag {
        if !doc.tags().iter().any(|t| t.name == *tag) {
            let names: Vec<&str> = doc.tags().iter().map(|t| t.name.as_str()).collect();
            return Err(Failure::Export(format!(
                "there is no tag named \"{tag}\" (tags: {})",
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                }
            )));
        }
        return Ok(Frames::Tag(tag.clone()));
    }
    if let Some((from, to)) = args.frame_range {
        if to >= doc.frame_count() {
            return Err(Failure::Export(format!(
                "--frame-range {from},{to} passes the last frame, {}",
                doc.frame_count() - 1
            )));
        }
        return Ok(Frames::Range { from, to });
    }
    if args.oneframe {
        return Ok(Frames::Range { from: 0, to: 0 });
    }
    Ok(Frames::All)
}

/// How many frames `frames` selects; a tag counts its own.
fn count(frames: &Frames, doc: &Document) -> usize {
    match frames {
        Frames::Range { from, to } => to - from + 1,
        Frames::Tag(t) => doc
            .tags()
            .iter()
            .find(|x| x.name == *t)
            .map_or(1, |x| x.to - x.from + 1),
        _ => doc.frame_count(),
    }
}

/// A `--save-as` name without tokens gets the ones Aseprite adds: the
/// layer and tag when they split, and the frame when each frame is a file,
/// counting on from a number the name ends in, else from 1.
fn default_pattern(path: &str, layers: bool, tags: bool, frames: bool) -> String {
    if path.contains('{') {
        return path.to_string();
    }
    let p = Path::new(path);
    let ext = p
        .extension()
        .map_or_else(String::new, |e| format!(".{}", e.to_string_lossy()));
    let stem = p
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let mut name = stem.clone();
    let mut frame = "{frame1}".to_string();
    let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if frames && digits > 0 {
        name.truncate(stem.len() - digits);
        frame = format!("{{frame{}}}", &stem[stem.len() - digits..]);
    }
    if layers {
        name.push_str(" ({layer})");
    }
    if tags {
        name.push_str(" #{tag}");
    }
    if frames {
        name.push_str(&frame);
    }
    let file = format!("{name}{ext}");
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => dir.join(file).to_string_lossy().into_owned(),
        None => file,
    }
}

/// The preset `--save-as` describes.
fn save_as_preset(args: &Args, doc: &Document, path: &str) -> Result<Preset, Failure> {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let format = match ext.as_str() {
        "png" => Format::Png,
        "gif" => Format::Gif,
        _ => {
            return Err(usage(format!(
                "--save-as writes .png frames or a .gif, not \"{path}\""
            )));
        }
    };
    if args.trim {
        return Err(usage("--trim applies to --sheet"));
    }
    let named: Vec<&str> = tokens(path).collect();
    let mut frames = frames(args, doc)?;
    let split_tags = args.split_tags || named.contains(&"tag");
    if split_tags && matches!(frames, Frames::All) {
        frames = Frames::EachTag;
    }
    let split_layers = args.split_layers || named.contains(&"layer");
    let many_frames = format == Format::Png && (count(&frames, doc) > 1 || split_tags);
    Ok(Preset {
        name: "--save-as".to_string(),
        format,
        frames,
        layers: if split_layers {
            Layers::Each
        } else {
            Layers::Visible
        },
        path: default_pattern(
            path,
            split_layers && !named.contains(&"layer"),
            split_tags && !named.contains(&"tag"),
            many_frames && !names_frame(path),
        ),
        ..common(args)
    })
}

/// The settings every command line preset shares.
fn common(args: &Args) -> Preset {
    Preset {
        scale: args.scale.unwrap_or(1),
        only_layers: args.layers.clone(),
        ignore_layers: args.ignore_layers.clone(),
        all_layers: args.all_layers,
        in_all: false,
        ..Preset::default()
    }
}

/// The preset `--sheet` describes.
fn sheet_preset(args: &Args, doc: &Document, path: &str) -> Result<Preset, Failure> {
    if args.split_tags {
        return Err(usage("--split-tags applies to --save-as"));
    }
    let sheet = args.sheet_type.unwrap_or(if args.columns.is_some() {
        export::Sheet::Grid
    } else if args.rows.is_some() {
        export::Sheet::Columns
    } else {
        export::Sheet::Horizontal
    });
    let frames = frames(args, doc)?;
    // A grid without a count is as near square as the frames make it.
    let square = ((count(&frames, doc) as f64).sqrt().ceil() as u32).max(1);
    Ok(Preset {
        name: "--sheet".to_string(),
        format: Format::Sheet,
        frames,
        layers: if args.split_layers {
            Layers::Split
        } else {
            Layers::Visible
        },
        sheet,
        columns: args.columns.unwrap_or(square),
        rows: args.rows.unwrap_or(square),
        padding: args.padding,
        border: args.border,
        trim: args.trim,
        data: args.data.clone().map_or(Data::None, Data::At),
        json_frames: args.json_frames,
        path: path.to_string(),
        ..common(args)
    })
}

/// Where the presets are read from.
fn presets_path(args: &Args) -> Option<PathBuf> {
    args.presets_file
        .clone()
        .or_else(|| export::user_dir().map(|d| d.join(FILE)))
}

fn read_presets(args: &Args) -> Result<Vec<Preset>, Failure> {
    match presets_path(args) {
        Some(path) => PresetFile::read(&path).map_err(Failure::Export),
        None => Ok(Preset::starters()),
    }
}

/// Runs `preset` on `doc`, writing under `dir`.
fn run_preset(
    doc: &Document,
    name: &str,
    preset: &Preset,
    dir: &Path,
    outcome: &mut Outcome,
    verbose: bool,
) -> Result<(), Failure> {
    let jobs = plan(doc, name, 0, preset, dir).map_err(Failure::Export)?;
    if jobs.is_empty() {
        return Err(Failure::Export(format!(
            "{} writes nothing for this document",
            preset.name
        )));
    }
    for job in &jobs {
        outcome.written += run_job(doc, name, preset, job).map_err(Failure::Export)?;
        if verbose {
            outcome.err.push(format!("wrote {}", job.path.display()));
            if let Some(data) = &job.data {
                outcome.err.push(format!("wrote {}", data.display()));
            }
        }
    }
    Ok(())
}

/// Carries out a command line.
pub fn run(args: &Args) -> Result<Outcome, Failure> {
    let mut outcome = Outcome::default();
    if args.list_presets {
        let presets = read_presets(args)?;
        for p in &presets {
            outcome.out.push(p.name.clone());
        }
        if args.input.is_none() {
            return Ok(outcome);
        }
    }
    if let Some(script) = &args.script {
        return run_script(args, script, outcome);
    }
    let Some(input) = &args.input else {
        return Err(usage("name a document to export"));
    };
    if !wants_output(args) && !args.list_tags && !args.list_layers {
        return Err(usage(
            "say what to write: --save-as, --sheet, --preset or --all-presets",
        ));
    }
    if args.data.is_some() && args.sheet.is_none() {
        return Err(usage("--data writes a sheet's JSON; give --sheet too"));
    }
    let opened = read_document(input).map_err(Failure::Export)?;
    for w in &opened.warnings {
        outcome
            .err
            .push(format!("warning: {}: {w}", input.display()));
    }
    let name = title(input);
    export_document(args, input, &opened.doc, &name, outcome)
}

fn wants_output(args: &Args) -> bool {
    args.save_as.is_some() || args.sheet.is_some() || !args.presets.is_empty() || args.all_presets
}

/// Runs `--script` over the document, when one is named, then exports
/// what the script left with any export flags given beside it.
fn run_script(args: &Args, script: &Path, mut outcome: Outcome) -> Result<Outcome, Failure> {
    if !script
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cdl"))
    {
        return Err(usage(format!(
            "pixl runs candela scripts (.cdl), not {}; Aseprite's Lua scripts do not run in pixl",
            script.display()
        )));
    }
    if args.data.is_some() && args.sheet.is_none() {
        return Err(usage("--data writes a sheet's JSON; give --sheet too"));
    }
    if (wants_output(args) || args.list_tags || args.list_layers) && args.input.is_none() {
        return Err(usage("the export flags export a document; name one"));
    }
    let mut session = Session {
        base: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        params: args.script_params.iter().cloned().collect(),
        ..Session::default()
    };
    if let Some(input) = &args.input {
        let opened = read_document(input).map_err(Failure::Export)?;
        for w in &opened.warnings {
            outcome
                .err
                .push(format!("warning: {}: {w}", input.display()));
        }
        let mut doc = ScriptDoc::new(None, opened.doc);
        if is_imported(input) {
            doc.name = Some(title(input));
        } else {
            doc.path = Some(input.clone());
        }
        session.add(doc);
    }
    let lib = script_library().map_err(Failure::Export)?;
    let ran = run_file(script, session, &lib);
    outcome.out.extend(ran.output.lines().map(str::to_string));
    outcome.err.extend(ran.session.alerts.iter().cloned());
    if let Some(e) = ran.error {
        return Err(Failure::Script {
            out: outcome.out,
            err: outcome.err,
            error: e.to_string(),
        });
    }
    let Some(input) = &args.input else {
        return Ok(outcome);
    };
    let doc = &ran.session.docs[0];
    export_document(args, input, &doc.doc, &title(input), outcome)
}

/// Where scripts import `pixl` and candela's standard library from: pixl's
/// folder of the user data directory, or the temporary folder without one.
fn script_library() -> Result<Library, String> {
    let root = export::user_dir().unwrap_or_else(std::env::temp_dir);
    Library::at(&root).map_err(|e| {
        format!(
            "could not write the script library to {}: {e}",
            root.display()
        )
    })
}

/// Lists and writes what the command line asks of `doc`, read from `input`.
fn export_document(
    args: &Args,
    input: &Path,
    doc: &Document,
    name: &str,
    mut outcome: Outcome,
) -> Result<Outcome, Failure> {
    let name = name.to_string();
    if args.list_layers {
        for (_, l) in doc.walk() {
            outcome.out.push(l.props.name.clone());
        }
    }
    if args.list_tags {
        for t in doc.tags() {
            outcome.out.push(t.name.clone());
        }
    }
    if let Some(path) = &args.save_as {
        let preset = save_as_preset(args, doc, path)?;
        run_preset(
            doc,
            &name,
            &preset,
            Path::new(""),
            &mut outcome,
            args.verbose,
        )?;
    }
    if let Some(path) = &args.sheet {
        let preset = sheet_preset(args, doc, path)?;
        run_preset(
            doc,
            &name,
            &preset,
            Path::new(""),
            &mut outcome,
            args.verbose,
        )?;
    }
    if !args.presets.is_empty() || args.all_presets {
        let presets = read_presets(args)?;
        let mut chosen: Vec<&Preset> = Vec::new();
        for wanted in &args.presets {
            match presets.iter().find(|p| p.name == *wanted) {
                Some(p) => chosen.push(p),
                None => {
                    let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
                    return Err(Failure::Export(format!(
                        "there is no preset named \"{wanted}\" (presets: {})",
                        names.join(", ")
                    )));
                }
            }
        }
        if args.all_presets {
            chosen.extend(presets.iter().filter(|p| p.in_all));
        }
        // Presets write beside the document, as the editor's do.
        let dir = input.parent().map_or_else(PathBuf::new, Path::to_path_buf);
        for preset in chosen {
            run_preset(doc, &name, preset, &dir, &mut outcome, args.verbose)?;
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_gets_aseprite_s_tokens() {
        assert_eq!(
            default_pattern("out.png", false, false, true),
            "out{frame1}.png"
        );
        assert_eq!(
            default_pattern("frame001.png", false, false, true),
            "frame{frame001}.png"
        );
        assert_eq!(
            default_pattern("build/out.png", true, true, true),
            Path::new("build")
                .join("out ({layer}) #{tag}{frame1}.png")
                .to_string_lossy()
        );
        assert_eq!(default_pattern("a.gif", false, true, false), "a #{tag}.gif");
        assert_eq!(
            default_pattern("{tag}.gif", false, true, false),
            "{tag}.gif"
        );
    }
}
