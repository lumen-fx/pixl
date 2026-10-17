//! What the rendered tests share: one app at a time, a copy of the app
//! directory per renderer, and screenshots on request.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use lumenui::Frame;

/// Lumen keeps one app's DOM per process, so the renders take turns.
static ONE_APP_AT_A_TIME: Mutex<()> = Mutex::new(());

pub fn turn() -> MutexGuard<'static, ()> {
    ONE_APP_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// An app directory that reads the editor's own `src/` and `assets/` and
/// renders on `backend` (`"cpu"` or `"gpu"`), with no introspection port.
pub fn app_dir(backend: &str) -> PathBuf {
    let app = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("app-{backend}"));
    std::fs::create_dir_all(&dir).expect("make the app directory");
    for part in ["src", "assets"] {
        let link = dir.join(part);
        // Tests of one binary start in parallel: another may have just
        // made the link.
        if !link.exists()
            && let Err(e) = link_dir(&app.join(part), &link)
            && !link.exists()
        {
            panic!("link the app sources: {e}");
        }
    }
    let toml = std::fs::read_to_string(app.join("lumen.toml")).expect("read lumen.toml");
    let toml = format!("{toml}\n[render]\nbackend = \"{backend}\"\n\n[mcp]\nport = 0\n");
    // Another test in this process may be starting an app over this file:
    // leave it alone when it already says this, and replace it whole
    // otherwise.
    let path = dir.join("lumen.toml");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(toml.as_str()) {
        let tmp = dir.join(format!("lumen.toml.{:?}", std::thread::current().id()));
        std::fs::write(&tmp, toml).expect("write lumen.toml");
        std::fs::rename(&tmp, &path).expect("write lumen.toml");
    }
    dir
}

#[cfg(unix)]
fn link_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

#[cfg(windows)]
fn link_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(from, to)
}

/// Saves `frame` as `<name>.png` under `PIXL_SHOTS` when that is set.
pub fn save(frame: &Frame, name: &str) {
    if let Some(dir) = std::env::var_os("PIXL_SHOTS") {
        frame
            .save_png(Path::new(&dir).join(format!("{name}.png")))
            .expect("write the screenshot");
    }
}

pub mod drive;
