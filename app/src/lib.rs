//! The pixl editor: the window, its panels and the canvas view, over the
//! document model in `pixl-core`.

pub mod settings;

use lumenui::prelude::*;
use lumenui_canvas::CanvasPlugin;

pub use settings::Look;

/// The editor, with its markup and stylesheets from `src/`: read from disk
/// with hot reload in a debug build, compiled in for a release build.
pub fn app() -> App {
    editor(lumen_source!("src/main.lmn", "src/main.css"))
}

/// The editor over another copy of the UI sources. A directory source reads
/// `lumen.toml` beside its `src/`, which is how a test runs the editor with
/// its own settings.
pub fn editor(source: Source) -> App {
    App::new()
        .add_plugins(LumenDefaultPlugins.with_source(source))
        .add_plugin(CanvasPlugin::default())
        .insert_resource(Look::default())
        .add_systems(
            TickStage::Systems,
            (settings::look_switches, settings::show_look)
                .chain()
                .after(ClickSet::Producers),
        )
}
