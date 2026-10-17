//! The pixl editor: the start screen, the window, its panels and the canvas
//! view, over the document model in `pixl-core`.

pub mod adjust;
pub mod aids;
pub mod editor;
pub mod exports;
pub mod files;
pub mod home;
pub mod image_menu;
pub mod input;
pub mod keymap;
pub mod layers;
pub mod menus;
pub mod palette;
pub mod panels;
pub mod perf;
pub mod presets;
pub mod preview;
pub mod progressive;
pub mod recents;
pub mod rig;
pub mod select;
pub mod settings;
pub mod settings_ui;
pub mod sheets;
pub mod slices;
pub mod store;
pub mod themes;
pub mod timeline;
pub mod toolbar;
pub mod toolbox;
pub mod tools;
pub mod transform_tool;
pub mod view;
pub mod workspace;

use std::path::PathBuf;

use lumenui::lumen_core::signals;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{CanvasPlugin, CanvasSet};

pub use editor::{Editor, Tool};
pub use home::Home;
pub use recents::Recents;
pub use settings::{Look, Settings, SettingsStore};
pub use themes::Themes;
pub use workspace::Workspace;

/// The largest document side the view shows: one canvas buffer per layer,
/// within the canvas module's per-buffer pixel cap.
pub const MAX_VIEW_SIDE: u32 = 8192;

/// pixl as the `pixl` program runs it, with its markup and stylesheets from
/// `src/` (read from disk with hot reload in a debug build, compiled in for
/// a release build) and the user's recent documents: `file` opens in the
/// editor, and with no file the start screen shows.
pub fn app(file: Option<PathBuf>) -> App {
    let (settings, store) = SettingsStore::user();
    starting(
        editor(lumen_source!("src/main.lmn", "src/main.css")),
        Recents::user(),
        file,
    )
    .insert_resource(settings)
    .insert_resource(store)
    .insert_resource(Workspace::user())
    .insert_resource(Themes::user())
    .insert_resource(exports::Exports::user())
    .insert_resource(presets::Presets::user())
    .insert_resource(toolbar::Toolbar::user())
}

/// `app` (an [`editor`]) starting on the start screen over `recents`, or on
/// `file` when there is one.
pub fn starting(app: App, recents: Recents, file: Option<PathBuf>) -> App {
    let home = match file {
        Some(path) => Home::opening(path),
        None => Home::start(),
    };
    app.insert_resource(recents).insert_resource(home)
}

/// The editor over another copy of the UI sources, open on a blank
/// document with no start screen, default settings and a recent list kept
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
        .insert_resource(keymap::Keymap::default())
        .insert_resource(keymap::KeyCommands::default())
        .insert_resource(settings_ui::SettingsDialog::default())
        .insert_resource(perf::Perf::default())
        .insert_resource(sheets::Sheets::default())
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
        .insert_signal("settings_open", false)
        .insert_resource(Editor::blank())
        .insert_resource(view::View::default())
        .insert_resource(view::ViewColors::default())
        .insert_resource(view::Preview::default())
        .insert_resource(view::Drawn::default())
        .insert_resource(view::Layers::default())
        .insert_resource(input::Navigation::default())
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
        .insert_resource(toolbox::Toolbox::default())
        .insert_resource(menus::Chosen::default())
        .insert_resource(select::Selections::default())
        .insert_resource(image_menu::ImageMenu::default())
        .insert_resource(adjust::Adjust::default())
        .insert_resource(aids::Aids::default())
        .insert_resource(transform_tool::FreeTransform::default())
        .insert_resource(panels::Panels::default())
        .insert_resource(files::Files::default())
        .insert_resource(Workspace::in_memory())
        .insert_signal("ws_open", false)
        .insert_signal("ws_name", "")
        .insert_signal("ws_message", "")
        .insert_resource(Themes::in_memory())
        .insert_signal("themes_open", false)
        .insert_signal("th_name", "")
        .insert_signal("th_message", "")
        .insert_resource(palette::Palette::default())
        .insert_resource(exports::Exports::in_memory())
        .insert_signal("ex_open", false)
        .insert_signal("ex_is_sheet", false)
        .insert_signal("ex_is_tag", false)
        .insert_signal("ex_hint", exports::PATH_HINT)
        .insert_signal("cp_open", false)
        .insert_signal("cp_query", "")
        .insert_resource(timeline::Timeline::default())
        .insert_resource(timeline::LayerTree::default())
        .insert_resource(preview::PreviewPane::default())
        .insert_resource(layers::LayerPanel::default())
        .insert_signal("lp_open", false)
        .insert_signal("lp_has_mask", false)
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
                    (
                        perf::tick_begin,
                        keymap::read_keys,
                        palette::palette_input,
                        settings_ui::settings_commands,
                        settings_ui::keys_page,
                        settings_ui::settings_form,
                        settings::look_switches,
                        settings::show_look,
                        sheets::sheet_commands,
                        view::read_view_colors,
                        view::measure_stage,
                        input::navigate,
                        files::file_commands,
                        workspace::workspace_input,
                        themes::theme_input,
                        exports::export_input,
                        files::new_form,
                    )
                        .chain(),
                    // The Select, Image, Filter and View menus.
                    (
                        menus::collect,
                        select::select_commands,
                        image_menu::image_commands,
                        adjust::filter_commands,
                        aids::aids_commands,
                        aids::aids_input,
                    )
                        .chain(),
                    // then the panels and the tools.
                    (
                        layers::layer_input,
                        panels::panel_clicks,
                        panels::history_keys,
                        panels::layer_controls,
                        panels::color_controls,
                        timeline::timeline_input,
                        preview::preview_controls,
                        tools::pick_tool,
                        tools::tool_options,
                        tools::use_tools,
                        toolbox::toolbox_options,
                        toolbox::use_toolbox,
                        transform_tool::use_transform,
                        perf::note_stroke,
                    )
                        .chain(),
                )
                    .chain(),
                // Then what the window shows of it.
                (
                    timeline::playback,
                    timeline::onion_to_view,
                    perf::composite_begin,
                    view::sync_layers,
                    view::draw_stage,
                    perf::composite_end,
                    aids::draw_rulers,
                    timeline::show_timeline,
                    preview::draw_preview,
                    tools::track_pointer,
                    panels::show_panels,
                    layers::show_layers,
                    files::autosave,
                    files::show_message,
                    (
                        workspace::show_workspace,
                        themes::show_themes,
                        palette::show_palette,
                        exports::show_exports,
                    ),
                    home::track_recents,
                    home::show_home,
                    (tools::show_tool, toolbox::show_toolbox).chain(),
                    tools::stage_cursor,
                    (
                        settings::save_settings,
                        settings_ui::pressure_preview,
                        settings_ui::menu_labels,
                        perf::show_perf,
                        sheets::sheet_forms,
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
    let app = presets::install(app, presets::Presets::in_memory());
    toolbar::install(app, toolbar::Toolbar::in_memory())
}

/// Asks for another tick soon. A system that edits the DOM or a signal after
/// the tick's readers ran uses it, so the change shows without waiting for
/// the next input event.
pub fn repaint_soon(wake: &Option<Res<WakeDeadline>>) {
    if let Some(wake) = wake {
        wake.request(std::time::Instant::now());
    }
}
