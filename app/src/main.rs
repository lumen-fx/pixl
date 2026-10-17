use std::path::PathBuf;

use pixl::perf::{Perf, PerfLog, take_perf_log};

/// `pixl` opens the start screen; `pixl <file>` opens the file.
/// `--perf-log <csv>` writes what each frame cost to a CSV file.
fn main() -> lumenui::Result<()> {
    let (log, rest) = take_perf_log(std::env::args_os().skip(1).collect());
    let file = rest.into_iter().next().map(PathBuf::from);
    let mut app = pixl::app(file);
    if let Some(path) = log {
        match PerfLog::create(&path) {
            Ok(log) => app = app.insert_resource(Perf::logging(log)),
            Err(e) => eprintln!("pixl: cannot write {}: {e}", path.display()),
        }
    }
    app.run()
}
