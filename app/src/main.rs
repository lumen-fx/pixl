use std::path::PathBuf;

/// `pixl` opens the start screen; `pixl <file>` opens the file.
fn main() -> lumenui::Result<()> {
    let file = std::env::args_os().nth(1).map(PathBuf::from);
    pixl::app(file).run()
}
