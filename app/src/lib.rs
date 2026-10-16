//! The pixl editor: the window, its panels and the canvas view, over the
//! document model in `pixl-core`.

pub mod editor;
pub mod input;
pub mod progressive;
pub mod settings;
pub mod view;

use lumenui::prelude::*;
use lumenui_canvas::{CanvasPlugin, CanvasSet};

pub use editor::{Editor, Tool};
pub use settings::Look;

/// The largest document side the view shows: one canvas buffer per layer,
/// within the canvas module's per-buffer pixel cap.
pub const MAX_VIEW_SIDE: u32 = 8192;

/// The editor, with its markup and stylesheets from `src/`: read from disk
/// with hot reload in a debug build, compiled in for a release build.
pub fn app() -> App {
    editor(lumen_source!("src/main.lmn", "src/main.css"))
}

/// The editor over another copy of the UI sources. A directory source reads
/// `lumen.toml` beside its `src/`, which is how a test runs the editor with
/// its own settings.
pub fn editor(source: Source) -> App {
    let side = u64::from(MAX_VIEW_SIDE);
    App::new()
        .add_plugins(LumenDefaultPlugins.with_source(source))
        // A buffer per layer: room for many, each up to the largest side.
        .add_plugin(CanvasPlugin::with_caps(1 << 20, side * side, 4096))
        .insert_resource(Look::default())
        .insert_resource(Editor::blank())
        .insert_resource(view::View::default())
        .insert_resource(view::ViewColors::default())
        .insert_resource(view::Preview::default())
        .insert_resource(view::Drawn::default())
        .insert_resource(view::Layers::default())
        .insert_resource(input::Navigation::default())
        .add_systems(
            TickStage::Systems,
            (
                settings::look_switches,
                settings::show_look,
                view::read_view_colors,
                view::measure_stage,
                input::navigate,
                view::sync_layers,
                view::draw_stage,
            )
                .chain()
                .after(ClickSet::Producers)
                .before(CanvasSet::Encode),
        )
}
