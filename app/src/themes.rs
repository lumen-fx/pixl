//! Themes: the four built-in token sets (dark, light, high contrast and
//! retro, each a class in the stylesheet) and the user's own themes, each a
//! built-in set with its color tokens overridden, kept as `.pixltheme`
//! files of plain CSS custom properties in the `themes` folder of pixl's
//! data folder. The theme editor edits the tokens with a live preview,
//! saves, deletes, imports and exports them.
//!
//! A user theme reaches the window as custom properties set on the root
//! element, so the stylesheet's rules read them the way they read the
//! built-in sets. No color lives in this file: the editor starts from what
//! the stylesheet resolves for each token.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lumenui::ecs::world::World;
use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::Rgba;
use pixl_core::native::write_atomic;

use crate::editor::Editor;
use crate::home::Home;
use crate::keymap::KeyCommands;
use crate::panels::escape;
use crate::settings::{Settings, Theme, UI_SCALES, UiFont};
use crate::store::Store;
use crate::view::{ViewColors, parse_hex};

/// The file extension of a theme file.
pub const EXTENSION: &str = "pixltheme";

/// The folder of pixl's data folder the user's themes are kept in.
pub const FOLDER: &str = "themes";

/// The color tokens a theme sets, in the order the editor lists them.
pub const TOKENS: [&str; 23] = [
    "surface-0",
    "surface-1",
    "surface-2",
    "surface-3",
    "surface-4",
    "surface-5",
    "edge",
    "edge-soft",
    "text",
    "text-dim",
    "text-faint",
    "accent",
    "accent-hover",
    "accent-press",
    "accent-soft",
    "on-accent",
    "canvas-bg",
    "checker-a",
    "checker-b",
    "pixel-grid",
    "marquee-a",
    "marquee-b",
    "preview",
];

/// The signal the editor's color field for `token` writes.
pub fn signal_of(token: &str) -> String {
    format!("th_{}", token.replace('-', "_"))
}

/// A color as `#rrggbbaa` (`#rrggbb` when opaque).
pub fn hex(c: Rgba) -> String {
    crate::tools::hex(c)
}

/// Reads a color as a color field writes it: `#rgb`, `#rgba`, `#rrggbb`,
/// `#rrggbbaa` or `color(srgb r g b / a)`.
pub fn parse_color(text: &str) -> Option<Rgba> {
    let text = text.trim();
    if let Some(inner) = text
        .strip_prefix("color(srgb")
        .and_then(|t| t.strip_suffix(')'))
    {
        let (rgb, alpha) = match inner.split_once('/') {
            Some((rgb, a)) => (rgb, a.trim().parse::<f32>().ok()?),
            None => (inner, 1.0),
        };
        let parts: Vec<f32> = rgb
            .split_whitespace()
            .map(|p| p.parse::<f32>().ok())
            .collect::<Option<_>>()?;
        let [r, g, b] = parts[..] else {
            return None;
        };
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        return Some(Rgba::new(byte(r), byte(g), byte(b), byte(alpha)));
    }
    let digits = text.strip_prefix('#')?;
    if digits.len() == 3 || digits.len() == 4 {
        let long: String = digits.chars().flat_map(|c| [c, c]).collect();
        return parse_hex(&format!("#{long}"));
    }
    parse_hex(text)
}

/// One of the user's themes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserTheme {
    pub name: String,
    /// The built-in set it starts from.
    pub base: Theme,
    pub font: Option<UiFont>,
    /// The UI scale in percent.
    pub scale: Option<u32>,
    /// The color tokens it sets, by name without the leading `--`.
    pub tokens: BTreeMap<String, Rgba>,
}

impl UserTheme {
    /// The theme as a `.pixltheme` file: a comment naming it, then one CSS
    /// rule of custom properties that a stylesheet could also use as is.
    pub fn to_css(&self) -> String {
        let mut out = format!(
            "/* pixl theme: {} */\n.pixl {{\n  --pixl-base: {};\n",
            self.name.replace("*/", ""),
            self.base.class()
        );
        if let Some(font) = self.font {
            let font = match font {
                UiFont::Pixel => "pixel",
                UiFont::Plain => "plain",
            };
            out.push_str(&format!("  --pixl-font: {font};\n"));
        }
        if let Some(scale) = self.scale {
            out.push_str(&format!("  --pixl-scale: {scale};\n"));
        }
        for token in TOKENS {
            if let Some(c) = self.tokens.get(token) {
                out.push_str(&format!("  --{token}: {};\n", hex(*c)));
            }
        }
        out.push_str("}\n");
        out
    }

    /// Reads a theme file. The name comes from its `pixl theme:` comment,
    /// else `fallback`. Properties it does not know are skipped.
    pub fn parse(text: &str, fallback: &str) -> std::result::Result<UserTheme, String> {
        let name = text
            .find("pixl theme:")
            .map(|i| &text[i + "pixl theme:".len()..])
            .and_then(|rest| rest.split("*/").next())
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(fallback)
            .to_string();
        let mut theme = UserTheme {
            name,
            base: Theme::Dark,
            font: None,
            scale: None,
            tokens: BTreeMap::new(),
        };
        let mut found = false;
        for decl in text.split(';') {
            let Some(at) = decl.find("--") else {
                continue;
            };
            let Some((prop, value)) = decl[at + 2..].split_once(':') else {
                continue;
            };
            let (prop, value) = (prop.trim(), value.trim());
            match prop {
                "pixl-base" => {
                    theme.base = Theme::from_class(value).ok_or(format!("unknown base {value}"))?;
                    found = true;
                }
                "pixl-font" => {
                    theme.font = match value {
                        "pixel" => Some(UiFont::Pixel),
                        "plain" => Some(UiFont::Plain),
                        _ => None,
                    }
                }
                "pixl-scale" => {
                    theme.scale = value.parse().ok().filter(|s| UI_SCALES.contains(s));
                }
                _ => {
                    if let (Some(token), Some(c)) =
                        (TOKENS.iter().find(|t| **t == prop), parse_color(value))
                    {
                        theme.tokens.insert((*token).to_string(), c);
                        found = true;
                    }
                }
            }
        }
        if !found {
            return Err("no pixl theme tokens in the file".to_string());
        }
        Ok(theme)
    }

    /// The file this theme is kept in, in `dir`.
    fn file_in(&self, dir: &Path) -> PathBuf {
        let stem: String = self
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        dir.join(format!("{}.{EXTENSION}", stem.trim()))
    }
}

/// The editor's working copy.
#[derive(Clone, Debug, Default)]
struct Draft {
    tokens: BTreeMap<String, Rgba>,
    /// The tokens as the color fields were last given them, to tell the
    /// user's edits from the editor's own writes.
    shown: BTreeMap<String, Rgba>,
    /// The user changed a token since the draft was read.
    edited: bool,
}

/// The user's themes and the editor's state.
#[derive(Resource)]
pub struct Themes {
    store: Store,
    pub list: Vec<UserTheme>,
    /// The custom properties set on the root element now.
    applied: BTreeMap<String, String>,
    draft: Option<Draft>,
    /// Bumped on every change to the themes or the draft.
    revision: u64,
    /// What the root element's properties were last set for.
    applied_for: Option<(Option<String>, u64, bool)>,
    /// Read the draft from the window after this many more ticks: a new
    /// set of classes takes a tick to land and another to cascade.
    read_in: Option<u8>,
    shown_list: Option<String>,
}

impl Themes {
    /// The themes in `store`'s themes folder.
    pub fn load(store: Store) -> Self {
        let mut list = Vec::new();
        if let Some(dir) = store.path(FOLDER)
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            let mut paths: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == EXTENSION))
                .collect();
            paths.sort();
            for path in paths {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Ok(text) = std::fs::read_to_string(&path)
                    && let Ok(theme) = UserTheme::parse(&text, &stem)
                    && !list.iter().any(|t: &UserTheme| t.name == theme.name)
                {
                    list.push(theme);
                }
            }
        }
        Self {
            store,
            list,
            applied: BTreeMap::new(),
            draft: None,
            revision: 0,
            applied_for: None,
            read_in: None,
            shown_list: None,
        }
    }

    pub fn user() -> Self {
        Self::load(Store::user())
    }

    pub fn in_memory() -> Self {
        Self::load(Store::in_memory())
    }

    pub fn get(&self, name: &str) -> Option<&UserTheme> {
        self.list.iter().find(|t| t.name == name)
    }

    /// Adds `theme`, replacing one of its name, and writes its file.
    pub fn put(&mut self, theme: UserTheme) -> std::result::Result<(), String> {
        if let Some(dir) = self.store.path(FOLDER) {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            write_atomic(&theme.file_in(&dir), theme.to_css().as_bytes())
                .map_err(|e| e.to_string())?;
        }
        match self.list.iter_mut().find(|t| t.name == theme.name) {
            Some(t) => *t = theme,
            None => self.list.push(theme),
        }
        self.shown_list = None;
        self.revision += 1;
        Ok(())
    }

    /// Deletes the theme `name` and its file.
    pub fn remove(&mut self, name: &str) -> bool {
        let Some(i) = self.list.iter().position(|t| t.name == name) else {
            return false;
        };
        let theme = self.list.remove(i);
        if let Some(dir) = self.store.path(FOLDER) {
            let _ = std::fs::remove_file(theme.file_in(&dir));
        }
        self.shown_list = None;
        self.revision += 1;
        true
    }
}

/// Puts the user theme `theme` in force, with its base, font and scale.
fn use_theme(settings: &mut Settings, theme: &UserTheme) {
    let i = &mut settings.interface;
    i.theme = theme.base;
    i.user_theme = Some(theme.name.clone());
    if let Some(font) = theme.font {
        i.font = font;
    }
    if let Some(scale) = theme.scale {
        i.scale = scale;
    }
}

/// The theme the editor's state describes, named `name`.
fn draft_theme(draft: &Draft, settings: &Settings, name: &str) -> UserTheme {
    UserTheme {
        name: name.to_string(),
        base: settings.interface.theme,
        font: Some(settings.interface.font),
        scale: Some(settings.interface.scale),
        tokens: draft.tokens.clone(),
    }
}

/// Keeps the editor's work: as the user theme in force, else as a new
/// theme named `name` (or "Custom").
fn keep_edits(themes: &mut Themes, settings: &mut Settings, name: &str) -> Option<String> {
    let draft = themes.draft.clone().filter(|d| d.edited)?;
    let name = settings
        .interface
        .user_theme
        .clone()
        .or_else(|| Some(name.trim().to_string()).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| "Custom".to_string());
    let theme = draft_theme(&draft, settings, &name);
    let result = themes.put(theme.clone());
    use_theme(settings, &theme);
    if let Some(d) = &mut themes.draft {
        d.edited = false;
    }
    Some(match result {
        Ok(()) => format!("Saved {name}"),
        Err(e) => format!("Could not save {name}: {e}"),
    })
}

/// The Themes command and the editor's buttons.
#[allow(clippy::too_many_arguments)]
pub fn theme_input(
    mut themes: ResMut<Themes>,
    mut settings: ResMut<Settings>,
    mut editor: ResMut<Editor>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    ids: Query<&LumenId>,
    (home, keys): (Res<Home>, Res<KeyCommands>),
    mut commands: Commands,
    mut signals: Signals,
) {
    let mut chosen: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    if keys.ran("window-themes", false) {
        chosen.push("window-themes".to_string());
    }
    chosen.extend(
        clicks
            .read()
            .filter_map(|c| ids.get(c.entity).ok().map(|id| id.0.clone())),
    );
    if home.open {
        return;
    }
    for id in chosen {
        match id.as_str() {
            "window-themes" => {
                themes.draft = None;
                themes.read_in = Some(0);
                themes.shown_list = None;
                let name = settings.interface.user_theme.clone().unwrap_or_default();
                signals.set("th_name", name);
                signals.set("th_message", String::new());
                signals.set("themes_open", true);
            }
            "th-close" => {
                let name = signals.get::<String>("th_name").unwrap_or_default();
                if let Some(note) = keep_edits(&mut themes, &mut settings, &name) {
                    editor.message = Some(note);
                }
                themes.draft = None;
                signals.set("themes_open", false);
            }
            "th-save" => {
                let name = signals.get::<String>("th_name").unwrap_or_default();
                let name = name.trim().to_string();
                let note = if name.is_empty() {
                    "Name the theme first".to_string()
                } else if let Some(draft) = themes.draft.clone() {
                    let theme = draft_theme(&draft, &settings, &name);
                    match themes.put(theme.clone()) {
                        Ok(()) => {
                            use_theme(&mut settings, &theme);
                            if let Some(d) = &mut themes.draft {
                                d.edited = false;
                            }
                            format!("Saved {name}")
                        }
                        Err(e) => format!("Could not save {name}: {e}"),
                    }
                } else {
                    String::new()
                };
                signals.set("th_message", note);
            }
            "th-revert" => {
                themes.draft = None;
                themes.read_in = Some(0);
                signals.set("th_message", "Back to the saved theme".to_string());
            }
            "th-delete" => {
                let note = match settings.interface.user_theme.clone() {
                    Some(name) if themes.remove(&name) => {
                        settings.interface.user_theme = None;
                        themes.draft = None;
                        themes.read_in = Some(2);
                        format!("Deleted {name}")
                    }
                    _ => "A built-in theme stays".to_string(),
                };
                signals.set("th_message", note);
            }
            "th-export" => {
                if let Some(draft) = themes.draft.clone() {
                    let name = settings
                        .interface
                        .user_theme
                        .clone()
                        .unwrap_or_else(|| settings.interface.theme.label().to_string());
                    let text = draft_theme(&draft, &settings, &name).to_css();
                    commands.queue(
                        FileDialog::save()
                            .filter("pixl theme", &[EXTENSION])
                            .file_name(format!("{name}.{EXTENSION}"))
                            .then(move |world: &mut World, paths| {
                                if let Some(path) = paths.into_iter().next() {
                                    let note = match write_atomic(&path, text.as_bytes()) {
                                        Ok(()) => format!("Exported {}", path.display()),
                                        Err(e) => format!("Could not export: {e}"),
                                    };
                                    world
                                        .resource_mut::<PropertyStore>()
                                        .set(PropertyKey::global("th_message"), note.into());
                                }
                            }),
                    );
                }
            }
            "th-import" => {
                commands.queue(FileDialog::open().filter("pixl theme", &[EXTENSION]).then(
                    |world: &mut World, paths| {
                        if let Some(path) = paths.into_iter().next() {
                            let note = import(world, &path);
                            world
                                .resource_mut::<PropertyStore>()
                                .set(PropertyKey::global("th_message"), note.into());
                        }
                    },
                ));
            }
            other => {
                if let Some(k) = other
                    .strip_prefix("th-pick-")
                    .and_then(|k| k.parse::<usize>().ok())
                {
                    pick(&mut themes, &mut settings, k, &mut signals);
                }
            }
        }
    }
}

/// Reads the theme file at `path` into the user's themes and puts it in
/// force.
pub fn import(world: &mut World, path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let theme = match std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|text| UserTheme::parse(&text, &stem))
    {
        Ok(theme) => theme,
        Err(e) => return format!("Could not import {}: {e}", path.display()),
    };
    let name = theme.name.clone();
    let result = world.resource_mut::<Themes>().put(theme.clone());
    use_theme(&mut world.resource_mut::<Settings>(), &theme);
    let mut themes = world.resource_mut::<Themes>();
    themes.draft = None;
    themes.read_in = Some(2);
    match result {
        Ok(()) => format!("Imported {name}"),
        Err(e) => format!("Imported {name} for now; could not keep it: {e}"),
    }
}

/// A click on the editor's theme list: the built-in sets, then the user's.
fn pick(themes: &mut Themes, settings: &mut Settings, k: usize, signals: &mut Signals) {
    if let Some(theme) = Theme::ALL.get(k) {
        settings.interface.theme = *theme;
        settings.interface.user_theme = None;
        signals.set("th_name", String::new());
    } else if let Some(theme) = themes.list.get(k - Theme::ALL.len()).cloned() {
        use_theme(settings, &theme);
        signals.set("th_name", theme.name.clone());
    } else {
        return;
    }
    themes.draft = None;
    themes.read_in = Some(2);
    themes.shown_list = None;
    signals.set("th_message", String::new());
}

/// The editor's theme list.
fn list_markup(themes: &Themes, settings: &Settings) -> String {
    let mut out = String::new();
    let user = settings.interface.user_theme.as_deref();
    for (k, theme) in Theme::ALL.iter().enumerate() {
        let current = if user.is_none() && settings.interface.theme == *theme {
            " current"
        } else {
            ""
        };
        out.push_str(&format!(
            r#"<row id="th-pick-{k}" class="th-theme{current}"><label text="{}"/><spacer/><label class="th-theme-kind" text="built in"/></row>"#,
            theme.label()
        ));
    }
    for (j, theme) in themes.list.iter().enumerate() {
        let k = Theme::ALL.len() + j;
        let current = if user == Some(theme.name.as_str()) {
            " current"
        } else {
            ""
        };
        out.push_str(&format!(
            r#"<row id="th-pick-{k}" class="th-theme{current}"><label text="{}"/><spacer/><label class="th-theme-kind" text="{}"/></row>"#,
            escape(&theme.name),
            theme.base.label()
        ));
    }
    out
}

/// Each token's value as the stylesheet resolves it now.
fn read_tokens() -> Option<BTreeMap<String, Rgba>> {
    TOKENS
        .iter()
        .map(|t| {
            let c = dom::get_by_id(&format!("tok-{t}"))?
                .computed_style_of("bg")
                .as_deref()
                .and_then(parse_hex)?;
            Some(((*t).to_string(), c))
        })
        .collect()
}

/// Paints the editor's preview with the draft's tokens.
fn paint_preview(tokens: &BTreeMap<String, Rgba>) {
    let get = |t: &str| tokens.get(t).map(|c| hex(*c));
    let style = |id: &str, prop: &str, token: &str| {
        if let (Some(node), Some(v)) = (dom::get_by_id(id), get(token)) {
            node.set_style(prop, &v);
        }
    };
    let border = |id: &str, token: &str| {
        if let (Some(node), Some(v)) = (dom::get_by_id(id), get(token)) {
            node.set_style("border", &format!("1px solid {v}"));
        }
    };
    style("tp-window", "bg", "surface-0");
    border("tp-window", "edge");
    style("tp-panel", "bg", "surface-2");
    border("tp-panel", "edge");
    style("tp-header", "bg", "surface-3");
    style("tp-title", "text-color", "text-dim");
    style("tp-text", "text-color", "text");
    style("tp-dim", "text-color", "text-dim");
    style("tp-faint", "text-color", "text-faint");
    style("tp-button", "bg", "surface-3");
    border("tp-button", "edge");
    style("tp-button-text", "text-color", "text");
    style("tp-accent", "bg", "accent");
    border("tp-accent", "accent-press");
    style("tp-accent-text", "text-color", "on-accent");
    style("tp-tint", "bg", "accent-soft");
    style("tp-tint-text", "text-color", "text");
    style("tp-canvas", "bg", "canvas-bg");
    for c in 0..8 {
        // Two rows of four, alternating.
        let token = if (c + c / 4) % 2 == 0 {
            "checker-a"
        } else {
            "checker-b"
        };
        style(&format!("tp-c{c}"), "bg", token);
    }
    style("tp-grid", "bg", "pixel-grid");
    border("tp-sel", "marquee-a");
    style("tp-shape", "bg", "preview");
}

/// Puts `want` on the root element as custom properties, changing only
/// what differs from what is there.
fn set_root_tokens(
    applied: &mut BTreeMap<String, String>,
    want: &BTreeMap<String, String>,
) -> bool {
    if applied == want {
        return false;
    }
    let Some(root) = dom::get_by_id("app") else {
        return false;
    };
    for name in applied.keys() {
        if !want.contains_key(name) {
            root.remove_style(&format!("--{name}"));
        }
    }
    for (name, value) in want {
        if applied.get(name) != Some(value) {
            root.set_style(&format!("--{name}"), value);
        }
    }
    *applied = want.clone();
    true
}

/// Keeps the user theme on the root element, and runs the editor: reads
/// the tokens into its fields, takes the user's edits into the draft, the
/// preview and the window, and lists the themes.
pub fn show_themes(
    mut themes: ResMut<Themes>,
    mut settings: ResMut<Settings>,
    mut colors: ResMut<ViewColors>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let open = signals.get::<bool>("themes_open") == Some(true);
    let mut busy = false;
    let editing = open && themes.draft.as_ref().is_some_and(|d| d.edited);
    let key = (
        settings.interface.user_theme.clone(),
        themes.revision,
        editing,
    );
    if themes.applied_for.as_ref() != Some(&key) {
        // The editor's edits show while it is open; otherwise the user
        // theme in force does.
        let want: BTreeMap<String, String> = match &themes.draft {
            Some(d) if editing => d.tokens.iter().map(|(k, c)| (k.clone(), hex(*c))).collect(),
            _ => settings
                .interface
                .user_theme
                .as_deref()
                .and_then(|n| themes.get(n))
                .map(|t| t.tokens.iter().map(|(k, c)| (k.clone(), hex(*c))).collect())
                .unwrap_or_default(),
        };
        let mut applied = std::mem::take(&mut themes.applied);
        if set_root_tokens(&mut applied, &want) {
            colors.reread();
            busy = true;
        }
        themes.applied = applied;
        themes.applied_for = Some(key);
    }

    if !open {
        if busy {
            crate::repaint_soon(&wake);
        }
        return;
    }
    if let Some(n) = themes.read_in {
        if n > 0 {
            themes.read_in = Some(n - 1);
            busy = true;
        } else if let Some(mut tokens) = read_tokens() {
            // The user theme's own values stand for its tokens.
            if let Some(theme) = settings
                .interface
                .user_theme
                .as_deref()
                .and_then(|n| themes.get(n))
            {
                tokens.extend(theme.tokens.clone());
            }
            for (token, c) in &tokens {
                signals.set(&signal_of(token), hex(*c));
            }
            let font = match settings.interface.font {
                UiFont::Pixel => "Pixel",
                UiFont::Plain => "Plain",
            };
            signals.set("th_font", font.to_string());
            signals.set("th_scale", format!("{}%", settings.interface.scale));
            paint_preview(&tokens);
            themes.draft = Some(Draft {
                shown: tokens.clone(),
                tokens,
                edited: false,
            });
            themes.read_in = None;
            busy = true;
        } else {
            busy = true;
        }
    }
    if let Some(draft) = themes.draft.as_mut() {
        let mut changed = false;
        for token in TOKENS {
            let Some(value) = signals
                .get::<String>(&signal_of(token))
                .as_deref()
                .and_then(parse_color)
            else {
                continue;
            };
            if draft.shown.get(token) != Some(&value) {
                draft.shown.insert(token.to_string(), value);
                draft.tokens.insert(token.to_string(), value);
                draft.edited = true;
                changed = true;
            }
        }
        if changed {
            paint_preview(&draft.tokens);
            themes.revision += 1;
            busy = true;
        }
        let font = match signals.get::<String>("th_font").as_deref() {
            Some("Plain") => UiFont::Plain,
            _ => UiFont::Pixel,
        };
        if settings.interface.font != font {
            settings.interface.font = font;
        }
        if let Some(scale) = signals
            .get::<String>("th_scale")
            .and_then(|s| s.trim_end_matches('%').parse::<u32>().ok())
            .filter(|s| UI_SCALES.contains(s))
            && settings.interface.scale != scale
        {
            settings.interface.scale = scale;
        }
    }
    let markup = list_markup(&themes, &settings);
    if themes.shown_list.as_deref() != Some(markup.as_str())
        && let Some(list) = dom::get_by_id("th-list")
    {
        list.set_inner_markup(&markup);
        themes.shown_list = Some(markup);
        busy = true;
    }
    if busy {
        crate::repaint_soon(&wake);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> UserTheme {
        let mut tokens = BTreeMap::new();
        tokens.insert("accent".to_string(), Rgba::new(255, 0, 128, 255));
        tokens.insert("accent-soft".to_string(), Rgba::new(255, 0, 128, 0x33));
        UserTheme {
            name: "Neon nights".to_string(),
            base: Theme::Contrast,
            font: Some(UiFont::Plain),
            scale: Some(125),
            tokens,
        }
    }

    #[test]
    fn a_theme_file_reads_back() {
        let theme = sample();
        let text = theme.to_css();
        assert!(text.contains("--accent: #ff0080;"));
        assert!(text.contains("--accent-soft: #ff008033;"));
        assert_eq!(UserTheme::parse(&text, "other").unwrap(), theme);
    }

    #[test]
    fn a_plain_stylesheet_reads_as_a_theme() {
        let theme = UserTheme::parse(
            ".pixl.mine { --text: #fff; --edge: color(srgb 1 0 0 / 0.5); --unknown: 3; }",
            "mine",
        )
        .unwrap();
        assert_eq!(theme.name, "mine");
        assert_eq!(theme.base, Theme::Dark);
        assert_eq!(theme.tokens["text"], Rgba::WHITE);
        assert_eq!(theme.tokens["edge"], Rgba::new(255, 0, 0, 128));
        assert_eq!(theme.tokens.len(), 2);
        assert!(UserTheme::parse("body { bg: #000; }", "x").is_err());
    }

    #[test]
    fn colors_read_in_every_form_a_color_field_writes() {
        assert_eq!(parse_color("#abc"), Some(Rgba::opaque(0xaa, 0xbb, 0xcc)));
        assert_eq!(
            parse_color("#abcd"),
            Some(Rgba::new(0xaa, 0xbb, 0xcc, 0xdd))
        );
        assert_eq!(
            parse_color("color(srgb 0 1 0)"),
            Some(Rgba::opaque(0, 255, 0))
        );
        assert_eq!(
            parse_color("color(srgb 0 0 1 / 0.2)"),
            Some(Rgba::new(0, 0, 255, 51))
        );
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn themes_are_kept_in_their_folder_and_deleted_with_it() {
        let dir = std::env::temp_dir().join(format!("pixl-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut themes = Themes::load(Store::at(&dir));
        themes.put(sample()).unwrap();
        let again = Themes::load(Store::at(&dir));
        assert_eq!(again.list, vec![sample()]);
        assert!(themes.remove("Neon nights"));
        assert!(Themes::load(Store::at(&dir)).list.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
