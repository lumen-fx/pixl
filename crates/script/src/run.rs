//! Running one script over a session.

use std::cell::RefCell;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use candela::{Diagnostic, Engine};

use crate::host;
use crate::session::Session;

/// candela's standard library modules that need no native library, as
/// candela 0.1.5 ships them (Apache-2.0; see `candela/LICENSE`).
const STD: [(&str, &str); 9] = [
    ("assert", include_str!("../candela/std/assert.cdl")),
    ("convert", include_str!("../candela/std/convert.cdl")),
    ("json", include_str!("../candela/std/json.cdl")),
    ("list", include_str!("../candela/std/list.cdl")),
    ("map", include_str!("../candela/std/map.cdl")),
    ("option", include_str!("../candela/std/option.cdl")),
    ("result", include_str!("../candela/std/result.cdl")),
    ("set", include_str!("../candela/std/set.cdl")),
    ("string", include_str!("../candela/std/string.cdl")),
];

/// The candela side of the API, after its host block.
const API: &str = include_str!("pixl.cdl");

/// The `pixl` module a script imports: the host block, then the API.
pub fn api_source() -> String {
    format!("{}\n{API}", host::host_block())
}

/// The folder scripts import `pixl` and `std/...` from. candela reads
/// imports from files, so the library is written out once per pixl build,
/// into a folder named for its contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    dir: PathBuf,
}

impl Library {
    /// The library under `root`, written there unless it already is.
    pub fn at(root: &Path) -> io::Result<Self> {
        let api = api_source();
        let mut hash = Fnv::default();
        hash.write(api.as_bytes());
        for (name, text) in STD {
            hash.write(name.as_bytes());
            hash.write(text.as_bytes());
        }
        let dir = root.join(format!("script-lib-{:016x}", hash.0));
        let done = dir.join(".complete");
        if !done.is_file() {
            std::fs::create_dir_all(dir.join("std"))?;
            std::fs::write(dir.join("pixl.cdl"), &api)?;
            for (name, text) in STD {
                std::fs::write(dir.join("std").join(format!("{name}.cdl")), text)?;
            }
            std::fs::write(&done, b"")?;
        }
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// 64-bit FNV-1a, for naming the library folder after what it holds.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Why a script stopped early.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    pub message: String,
    /// candela's short name for it, such as `host_fn_error` or
    /// `unexpected_token`.
    pub code: String,
    /// The file it happened in.
    pub file: String,
    /// The line, counted from 1, when the error points at one.
    pub line: Option<usize>,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: {}", self.file, self.message),
            None if self.file.is_empty() => write!(f, "{}", self.message),
            None => write!(f, "{}: {}", self.file, self.message),
        }
    }
}

/// What a run left: the session as the script left it, what it printed,
/// and the error that stopped it, if one did.
#[derive(Debug)]
pub struct Outcome {
    pub session: Session,
    pub output: String,
    pub error: Option<ScriptError>,
}

/// Compiles and runs the script at `path` (its text is `source`) over
/// `session`. Errors in the script come back in the outcome, never as a
/// panic.
pub fn run(source: &str, path: &Path, session: Session, lib: &Library) -> Outcome {
    let shared = Rc::new(RefCell::new(session));
    let mut engine = Engine::new().with_lib_dir(lib.dir());
    host::register(&mut engine, &shared);
    let filename = path.display().to_string();
    let was = candela_vm::captured_output::set_capturing(true);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        engine.compile(source, &filename).map(drop)
    }));
    candela_vm::captured_output::set_capturing(was);
    let output = candela_vm::captured_output::CAPTURED_OUTPUT.with(|o| o.take());
    drop(engine);
    let error = match result {
        Ok(Ok(())) => None,
        Ok(Err(d)) => Some(script_error(&d, path, source, lib)),
        Err(_) => Some(ScriptError {
            message: "the script stopped pixl's script engine".into(),
            code: "panic".into(),
            file: filename,
            line: None,
        }),
    };
    let session = Rc::try_unwrap(shared)
        .map(RefCell::into_inner)
        .unwrap_or_else(|rc| rc.borrow().clone());
    let error = match error {
        Some(e) if session.cancel.is_cancelled() => Some(ScriptError {
            message: "the script was stopped".into(),
            code: "cancelled".into(),
            ..e
        }),
        e => e,
    };
    Outcome {
        session,
        output,
        error,
    }
}

/// Runs the script file at `path`.
pub fn run_file(path: &Path, session: Session, lib: &Library) -> Outcome {
    match std::fs::read_to_string(path) {
        Ok(source) => run(&source, path, session, lib),
        Err(e) => Outcome {
            session,
            output: String::new(),
            error: Some(ScriptError {
                message: format!("could not read the script: {e}"),
                code: "fs_error".into(),
                file: path.display().to_string(),
                line: None,
            }),
        },
    }
}

fn script_error(d: &Diagnostic, path: &Path, source: &str, lib: &Library) -> ScriptError {
    // An error raised in a function of the API points into the library.
    // candela reports one place, so the script's own line is lost there;
    // the message names the call instead.
    // candela names an imported file by its canonical path, which differs
    // from the library folder's own name where the temporary folder is a
    // link (macOS) or takes a `\\?\` prefix (Windows).
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let in_lib = !d.filename.is_empty()
        && canonical(Path::new(&d.filename)).starts_with(canonical(lib.dir()));
    let text = if d.filename.is_empty() || Path::new(&d.filename) == path {
        Some(source.to_string())
    } else if in_lib {
        None
    } else {
        std::fs::read_to_string(&d.filename).ok()
    };
    let line = text.filter(|_| d.span.end > 0).map(|t| {
        t.as_bytes()[..d.span.start.min(t.len())]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1
    });
    let file = if d.filename.is_empty() || in_lib {
        path.display().to_string()
    } else {
        d.filename.clone()
    };
    let mut message = d.message.trim();
    if let Some(rest) = message.strip_prefix("Host function ")
        && let Some((_, why)) = rest.split_once(" failed: ")
    {
        message = why;
    }
    ScriptError {
        message: message.to_string(),
        code: d.code.clone(),
        file,
        line,
    }
}
