//! The pixl editor: the start screen, the window, its panels and the canvas
//! view, over the document model in `pixl-document`, the tools in
//! `pixl-paint` and the file formats in `pixl-io`.

pub mod documents;
pub mod editor;
pub mod files;
pub mod home;
pub mod ids;
pub mod menus;
pub mod panels;
pub mod perf;
pub mod rig;
pub mod settings;
pub mod tools;
pub mod tutorial;
pub mod view;
pub mod workspace;

use std::path::PathBuf;

use lumenui::lumen_core::signals;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{CanvasPlugin, CanvasSet};

pub use documents::{DocId, Documents};
pub use editor::{Editor, Tool};
pub use files::recents::Recents;
pub use home::Home;
pub use settings::themes::Themes;
pub use settings::{Look, Settings, SettingsStore};
pub use workspace::Workspace;

/// The largest document side the view shows: one canvas buffer per layer,
/// within the canvas module's per-buffer pixel cap.
pub const MAX_VIEW_SIDE: u32 = 8192;

/// pixl as the `pixl` program runs it, with its markup and stylesheets from
/// [`ui`] and the user's recent documents: `file` opens in the editor, and
/// with no file the start screen shows.
pub fn app(file: Option<PathBuf>) -> App {
    let (settings, store) = SettingsStore::user();
    starting(editor(ui()), Recents::user(), file)
        .insert_resource(settings)
        .insert_resource(store)
        .insert_resource(Workspace::user())
        .insert_resource(Themes::user())
        .insert_resource(files::exports::Exports::user())
        .insert_resource(tools::presets::Presets::user())
        .insert_resource(tools::toolbar::Toolbar::user())
}

/// `app` (an [`editor`]) starting with no document open: on the start
/// screen over `recents`, or opening `file` when there is one.
pub fn starting(app: App, recents: Recents, file: Option<PathBuf>) -> App {
    let home = match file {
        Some(path) => Home::opening(path),
        None => Home::start(),
    };
    app.insert_resource(recents)
        .insert_resource(home)
        .insert_resource(Documents::empty())
}

/// pixl's markup and stylesheets. A debug build reads them from `src/` with
/// hot reload. A release build embeds them with every `<include>` and
/// `@import` spliced in by the build script, so a shipped `pixl.exe` reads
/// nothing from `src/`.
pub fn ui() -> Source {
    #[cfg(debug_assertions)]
    let source = lumen_source!("src/main.lmn", "src/main.css");
    #[cfg(not(debug_assertions))]
    let source = Source::embedded_from(
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.lmn"),
        include_str!(concat!(env!("OUT_DIR"), "/main.lmn")),
        Some(include_str!(concat!(env!("OUT_DIR"), "/main.css"))),
    );
    source
}

/// The editor over another copy of the UI sources, open on a blank
/// document in one tab with no start screen, default settings and a recent list kept
/// in memory. A
/// directory source reads `lumen.toml` beside its `src/`, which is how a
/// test runs the editor with its own settings.
pub fn editor(source: Source) -> App {
    let side = u64::from(MAX_VIEW_SIDE);
    let app = App::new()
        .add_plugins(LumenDefaultPlugins.with_source(source))
        // A buffer per layer: room for many, each up to the largest side.
        .add_plugin(CanvasPlugin::with_caps(1 << 20, side * side, 4096))
        .insert_resource(Look::default())
        .insert_resource(Settings::default())
        .insert_resource(SettingsStore::default())
        .insert_resource(settings::keymap::Keymap::default())
        .insert_resource(settings::keymap::KeyCommands::default())
        .insert_resource(settings::dialog::SettingsDialog::default())
        .insert_resource(perf::Perf::default())
        .insert_resource(perf::SystemTimes::default())
        .insert_resource(files::sheets::Sheets::default())
        .insert_signal("sheet_export_open", false)
        .insert_signal("sheet_import_open", false)
        .insert_signal("sx_layout", "Horizontal strip")
        .insert_signal("sx_frames", "All frames")
        .insert_signal("sx_json", "Hash")
        .insert_signal("sx_columns", "8")
        .insert_signal("sx_rows", "8")
        .insert_signal("sx_from", "1")
        .insert_signal("sx_to", "")
        .insert_signal("sx_padding", "0")
        .insert_signal("sx_border", "0")
        .insert_signal("sx_scale", "1")
        .insert_signal("sx_trim", false)
        .insert_signal("sx_by_layer", false)
        .insert_signal("si_mode", "Cell size")
        .insert_signal("si_w", "32")
        .insert_signal("si_h", "32")
        .insert_signal("si_cols", "4")
        .insert_signal("si_rows", "1")
        .insert_signal("si_ox", "0")
        .insert_signal("si_oy", "0")
        .insert_signal("si_px", "0")
        .insert_signal("si_py", "0")
        .insert_signal("si_ms", "100")
        .insert_signal("si_skip", false)
        .insert_signal("perf_open", false)
        .insert_signal("img_busy", false)
        .insert_signal("img_progress", "")
        .insert_signal("settings_open", false)
        .insert_resource(Editor::blank())
        .insert_resource(Documents::default())
        .insert_signal("discard_title", "Close without saving?")
        .insert_signal("discard_text", "")
        .insert_signal("discard_yes", "Close without saving")
        .insert_resource(view::View::default())
        .insert_resource(view::ViewColors::default())
        .insert_resource(view::Preview::default())
        .insert_resource(view::Drawn::default())
        .insert_resource(view::Layers::default())
        .insert_resource(view::input::Navigation::default())
        .insert_resource(Home::default())
        .insert_resource(Recents::in_memory())
        .insert_signal("new_name", "untitled")
        .insert_signal("new_width", "64")
        .insert_signal("new_height", "64")
        .insert_signal("new_mode", "RGBA")
        .insert_signal("new_background", "Transparent")
        .insert_signal("new_bg_hex", "#ffffff")
        .insert_signal("new_bg_color", false)
        .insert_signal("home_filter", "")
        .insert_signal("home_can_return", false)
        .insert_resource(tools::Gesture::default())
        .insert_resource(tools::toolbox::Toolbox::default())
        .insert_resource(menus::Chosen::default())
        .insert_resource(menus::select::Selections::default())
        .insert_resource(menus::image::ImageMenu::default())
        .insert_resource(menus::adjust::Adjust::default())
        .insert_resource(view::aids::Aids::default())
        .insert_resource(tools::transform::FreeTransform::default())
        .insert_resource(panels::Panels::default())
        .insert_resource(panels::color::ColorPanel::default())
        .insert_resource(files::Files::default())
        .insert_resource(Workspace::in_memory())
        .insert_signal("ws_open", false)
        .insert_signal("ws_name", "")
        .insert_signal("ws_message", "")
        .insert_resource(Themes::in_memory())
        .insert_signal("themes_open", false)
        .insert_signal("th_name", "")
        .insert_signal("th_message", "")
        .insert_resource(menus::palette::Palette::default())
        .insert_resource(files::exports::Exports::in_memory())
        .insert_signal("ex_open", false)
        .insert_signal("ex_is_sheet", false)
        .insert_signal("ex_is_tag", false)
        .insert_signal("ex_hint", files::exports::PATH_HINT)
        .insert_signal("cp_open", false)
        .insert_signal("cp_query", "")
        .insert_resource(panels::timeline::Timeline::default())
        .insert_resource(panels::timeline::LayerTree::default())
        .insert_resource(panels::preview::PreviewPane::default())
        .insert_resource(panels::layers::LayerPanel::default())
        .insert_resource(tools::reference::ReferenceBox::default())
        .insert_signal("lp_open", false)
        .insert_signal("lp_has_mask", false)
        .insert_signal("lp_is_reference", false)
        .insert_signal("lp_name", "")
        .insert_signal("lp_opacity", "100")
        .insert_signal("lp_blend", "Normal")
        .insert_signal("tag_open", false)
        .insert_signal("tag_name", "")
        .insert_signal("tag_from", "1")
        .insert_signal("tag_to", "1")
        .insert_signal("tag_direction", "Forward")
        .insert_signal("tag_repeat", "0")
        .add_systems(
            TickStage::Systems,
            (
                // What the user did this tick, in the order it applies:
                // the keys, settings and windows first,
                (
                    timed!(
                        perf::tick_begin,
                        settings::keymap::read_keys,
                        menus::palette::palette_input,
                        settings::dialog::settings_commands,
                        settings::dialog::keys_page,
                        settings::dialog::settings_form,
                        settings::look_switches,
                        settings::show_look,
                        files::sheets::sheet_commands,
                        view::read_view_colors,
                        view::measure_stage,
                        view::input::navigate,
                        files::file_commands,
                        documents::tab_input,
                        workspace::workspace_input,
                        settings::themes::theme_input,
                        files::exports::export_input,
                        files::new_form,
                    )
                    .chain(),
                    // A document brought to the front, opened or closed.
                    documents::apply_tabs,
                    // The Select, Image, Filter and View menus.
                    timed!(
                        menus::collect,
                        menus::select::select_commands,
                        menus::image::image_commands,
                        menus::adjust::filter_commands,
                        view::aids::read_aid_colors,
                        view::aids::follow_grid_setting,
                        view::aids::aids_commands,
                        view::aids::aids_input,
                    )
                    .chain(),
                    // then the panels and the tools.
                    timed!(
                        panels::layers::layer_input,
                        panels::panel_clicks,
                        panels::history_keys,
                        panels::layer_controls,
                        panels::color::color_input,
                        panels::timeline::timeline_input,
                        panels::preview::preview_controls,
                        tools::pick_tool,
                        tools::tool_options,
                        tools::use_tools,
                        tools::toolbox::toolbox_options,
                        tools::toolbox::use_toolbox,
                        tools::transform::use_transform,
                        tools::reference::place_reference,
                        perf::note_stroke,
                    )
                    .chain(),
                )
                    .chain(),
                // Then what the window shows of it.
                (
                    timed!(
                        panels::timeline::playback,
                        panels::timeline::onion_to_view,
                        perf::composite_begin,
                        view::sync_layers,
                        view::draw_stage,
                        perf::composite_end,
                        view::aids::draw_rulers,
                        panels::timeline::show_timeline,
                        panels::preview::draw_preview,
                        tools::track_pointer,
                        panels::show_panels,
                        panels::color::show_color,
                        panels::layers::show_layers,
                        files::autosave,
                        files::show_message,
                    )
                    .chain(),
                    timed!(
                        workspace::show_workspace,
                        settings::themes::show_themes,
                        menus::palette::show_palette,
                        files::exports::show_exports,
                    ),
                    timed!(home::track_recents, home::show_home, documents::show_tabs).chain(),
                    timed!(tools::show_tool, tools::toolbox::show_toolbox).chain(),
                    timed!(tools::stage_cursor),
                    timed!(
                        settings::save_settings,
                        settings::dialog::menu_labels,
                        perf::show_perf,
                        files::sheets::sheet_forms,
                    )
                    .chain(),
                )
                    .chain(),
            )
                .chain()
                .after(ClickSet::Producers)
                .after(signals::push_slider_to_signal)
                .after(signals::push_toggle_to_signal)
                .after(signals::push_textinput_to_signal)
                .before(signals::apply_text_bindings)
                .before(signals::apply_checked_bindings)
                .before(signals::apply_value_bindings)
                .before(CanvasSet::Encode),
        )
        .add_systems(
            TickStage::A11ySync,
            perf::tick_end.after(lumenui::lumen_core::render_world::roll_up_frame_dirty),
        );
    let app = rig::install(app);
    let app = panels::tileset::install(app);
    let app = tutorial::install(app);
    let app = tools::presets::install(app, tools::presets::Presets::in_memory());
    let app = tools::dynamics::install(app);
    tools::toolbar::install(app, tools::toolbar::Toolbar::in_memory())
}

/// Puts `class` on `node` or takes it off. A class edit restyles the node
/// and everything under it, so the node is touched only when the class
/// changes.
pub fn set_class(node: dom::Node, class: &str, on: bool) {
    if node.has_class(class) != on {
        if on {
            node.add_class(class);
        } else {
            node.remove_class(class);
        }
    }
}

/// Asks for another tick soon. A system that edits the DOM or a signal after
/// the tick's readers ran uses it, so the change shows without waiting for
/// the next input event.
pub fn repaint_soon(wake: &Option<Res<WakeDeadline>>) {
    if let Some(wake) = wake {
        wake.request(std::time::Instant::now());
    }
}
