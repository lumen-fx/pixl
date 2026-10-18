//! Splices pixl's markup and stylesheets into one file each, for a release
//! build to embed.
//!
//! A release build embeds `src/main.lmn` and `src/main.css`, but Lumen reads
//! what they name, every `<include>` and `@import`, from beside the
//! executable at startup. Both are spliced in as text before parsing, so the
//! same splice done here leaves nothing for the runtime to read: a shipped
//! `pixl.exe` needs only `lumen.toml` and `assets/` beside it. A debug build
//! keeps reading `src/` from disk, for hot reload.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let src = Path::new("src");
    println!("cargo:rerun-if-changed=src");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let markup = splice(&src.join("main.lmn"), &Directive::INCLUDE, &mut Vec::new());
    let css = splice(&src.join("main.css"), &Directive::IMPORT, &mut Vec::new());
    fs::write(out.join("main.lmn"), markup).expect("write the spliced markup");
    fs::write(out.join("main.css"), css).expect("write the spliced stylesheet");
}

/// A directive that names another file, the way Lumen's front-end reads it.
struct Directive {
    open: &'static str,
    close: &'static str,
}

impl Directive {
    const INCLUDE: Directive = Directive {
        open: "<include src=\"",
        close: "\"/>",
    };
    const IMPORT: Directive = Directive {
        open: "@import \"",
        close: "\";",
    };
}

/// `path`'s text with every directive replaced by the named file's text, the
/// named path relative to the file that names it. `stack` is the chain of
/// files being spliced, so a cycle fails the build instead of recursing.
fn splice(path: &Path, directive: &Directive, stack: &mut Vec<PathBuf>) -> String {
    if stack.iter().any(|p| p == path) {
        panic!("{} names itself through {:?}", path.display(), stack);
    }
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    stack.push(path.to_path_buf());
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find(directive.open) {
        let after = &rest[start + directive.open.len()..];
        let Some(end) = after.find(directive.close) else {
            panic!("{}: unterminated {}", path.display(), directive.open);
        };
        out.push_str(&rest[..start]);
        out.push_str(&splice(&dir.join(&after[..end]), directive, stack));
        rest = &after[end + directive.close.len()..];
    }
    out.push_str(rest);
    stack.pop();
    out
}
