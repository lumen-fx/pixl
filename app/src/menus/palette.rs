//! The command palette (Ctrl+Shift+P): one search field over every command
//! a key can run, every tool, the workspace layouts and themes, the brushes
//! and tool presets, the open documents and the recent ones. Typing filters by the letters in order; the arrow keys
//! pick and Enter runs, as a click on a result does.
//!
//! A command runs the way its key would, through the keymap's commands; a
//! layout, a theme or a recent document runs the way its menu item or list
//! entry would.

use lumenui::ecs::system::{ParamSet, SystemParam};
use lumenui::lumen_core::input::{Key, MenuClicked, NamedKey};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;

use crate::documents::{Documents, palette_entries};
use crate::editor::Editor;
use crate::files::recents::{Recents, key};
use crate::home::Home;
use crate::panels::escape;
use crate::settings::Theme;
use crate::settings::keymap::{KeyCommands, Keymap, Kind, commands};
use crate::settings::themes::Themes;
use crate::tools::presets::Presets;
use crate::workspace::Workspace;

/// The command that opens the palette.
pub const OPEN: &str = "command-palette";

/// The most results the list shows.
pub const SHOWN: usize = 40;

/// How one entry runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Run {
    /// A keymap command, by id.
    Command(String),
    /// A menu item or list entry, by id.
    Item(String),
}

/// One thing the palette can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    /// The kind of thing it is: a command's group, "Layout", "Theme",
    /// "Document" or "Recent".
    pub group: String,
    /// The keys that run it, as text.
    pub keys: String,
    pub run: Run,
}

/// How well `query` matches `text`: its letters in order, case aside,
/// scoring runs of letters and letters that start a word higher, and a
/// match at the very start highest. `None` when they are not all there.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut total = 0;
    let mut at = 0;
    let mut last: Option<usize> = None;
    for q in query {
        let i = (at..text.len()).find(|&i| text[i] == q)?;
        let word_start = i == 0 || !text[i - 1].is_alphanumeric();
        total += 1;
        if word_start {
            total += 8;
        }
        if last.is_some_and(|l| l + 1 == i) {
            total += 5;
        }
        if i == 0 {
            total += 10;
        }
        // Letters far apart count for less.
        total -= (i - last.map_or(0, |l| l + 1)).min(10) as i32;
        last = Some(i);
        at = i + 1;
    }
    Some(total)
}

/// The entries matching `query`, best first; ties keep their order.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let mut hits: Vec<(i32, usize, &Entry)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let label = score(query, &e.label);
            let full = score(query, &format!("{} {}", e.group, e.label)).map(|s| s - 4);
            label.max(full).map(|s| (s, i, e))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, _, e)| e).collect()
}

/// Everything the palette lists now.
pub fn entries(
    keymap: &Keymap,
    workspace: &Workspace,
    themes: &Themes,
    presets: &Presets,
    open: Vec<Entry>,
    recents: &Recents,
) -> Vec<Entry> {
    let mut out: Vec<Entry> = commands()
        .into_iter()
        .filter(|c| c.kind == Kind::Press && c.id != OPEN)
        .map(|c| {
            let keys = if keymap.chords(&c.id).is_empty() {
                String::new()
            } else {
                keymap.describe(&c.id)
            };
            Entry {
                label: c.label,
                group: c.group.to_string(),
                keys,
                run: Run::Command(c.id),
            }
        })
        .collect();
    for name in workspace.names() {
        out.push(Entry {
            label: format!("{name} layout"),
            group: "Layout".to_string(),
            keys: String::new(),
            run: Run::Item(format!("window-layout-{name}")),
        });
    }
    let built_in = Theme::ALL.iter().map(|t| t.label().to_string());
    let user = themes.list.iter().map(|t| t.name.clone());
    for (k, name) in built_in.chain(user).enumerate() {
        out.push(Entry {
            label: format!("{name} theme"),
            group: "Theme".to_string(),
            keys: String::new(),
            run: Run::Item(format!("th-pick-{k}")),
        });
    }
    for (i, b) in presets.brushes.iter().enumerate() {
        out.push(Entry {
            label: format!("{} brush", b.name),
            group: "Brush".to_string(),
            keys: String::new(),
            run: Run::Item(format!("brush-pick-{i}")),
        });
    }
    for (i, p) in presets.tools.iter().enumerate() {
        let tool = p.tool().map_or("", |t| t.label());
        out.push(Entry {
            label: format!("{} ({tool})", p.name),
            group: "Preset".to_string(),
            keys: String::new(),
            run: Run::Item(format!("tp-use-{i}")),
        });
    }
    out.extend(open);
    for recent in &recents.entries {
        let name = recent
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        out.push(Entry {
            label: name,
            group: "Recent".to_string(),
            keys: recent
                .path
                .parent()
                .map_or_else(String::new, |p| p.display().to_string()),
            run: Run::Item(format!("recent-{}", key(&recent.path))),
        });
    }
    out
}

/// The palette while it is open.
#[derive(Resource, Default)]
pub struct Palette {
    /// What it lists, gathered when it opened.
    entries: Vec<Entry>,
    /// The results for the query, as indexes into `entries`.
    results: Vec<usize>,
    selected: usize,
    /// What the list last showed.
    shown: Option<String>,
    /// The query the results are for.
    query: Option<String>,
}

/// What the palette gathers its entries from.
#[derive(SystemParam)]
pub struct Sources<'w> {
    keymap: Res<'w, Keymap>,
    workspace: Res<'w, Workspace>,
    themes: Res<'w, Themes>,
    presets: Res<'w, Presets>,
    recents: Res<'w, Recents>,
    home: Res<'w, Home>,
    documents: Res<'w, Documents>,
    editor: Res<'w, Editor>,
}

/// Opens the palette, moves through it and runs what it picks. Runs right
/// after the keymap, so what it runs reaches the systems after it on the
/// same tick.
#[allow(clippy::too_many_arguments)]
pub fn palette_input(
    mut palette: ResMut<Palette>,
    mut keys: ResMut<KeyCommands>,
    mut presses: MessageReader<KeyPressed>,
    mut clicks: MessageReader<ClickEvent>,
    mut menu: ParamSet<(MessageReader<MenuClicked>, MessageWriter<MenuClicked>)>,
    ids: Query<&LumenId>,
    sources: Sources,
    mut signals: Signals,
) {
    let open = signals.get::<bool>("cp_open") == Some(true);
    let asked = keys.ran(OPEN, false) || menu.p0().read().any(|m| m.id == "window-palette");
    if asked && !open && !sources.home.open {
        palette.entries = entries(
            &sources.keymap,
            &sources.workspace,
            &sources.themes,
            &sources.presets,
            palette_entries(&sources.documents, &sources.editor),
            &sources.recents,
        );
        palette.query = None;
        palette.shown = None;
        palette.selected = 0;
        signals.set("cp_query", String::new());
        signals.set("cp_open", true);
        presses.clear();
        clicks.clear();
        return;
    }
    if !open {
        presses.clear();
        clicks.clear();
        return;
    }
    let mut chosen = None;
    for press in presses.read() {
        match press.key {
            Key::Named(NamedKey::ArrowDown) => {
                if palette.selected + 1 < palette.results.len() {
                    palette.selected += 1;
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                palette.selected = palette.selected.saturating_sub(1);
            }
            Key::Named(NamedKey::Enter) => chosen = Some(palette.selected),
            Key::Named(NamedKey::Escape) => signals.set("cp_open", false),
            _ => {}
        }
    }
    for click in clicks.read() {
        if let Some(i) = ids
            .get(click.entity)
            .ok()
            .and_then(|id| id.0.strip_prefix("cp-item-"))
            .and_then(|i| i.parse::<usize>().ok())
        {
            chosen = Some(i);
        }
    }
    let Some(entry) = chosen
        .and_then(|i| palette.results.get(i))
        .and_then(|&k| palette.entries.get(k))
        .cloned()
    else {
        return;
    };
    signals.set("cp_open", false);
    match entry.run {
        Run::Command(id) => keys.pressed.push((id, false)),
        Run::Item(id) => {
            menu.p1().write(MenuClicked { id });
        }
    }
}

/// Keeps the palette's results following the query.
pub fn show_palette(
    mut palette: ResMut<Palette>,
    signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    if signals.get::<bool>("cp_open") != Some(true) {
        return;
    }
    let query = signals.get::<String>("cp_query").unwrap_or_default();
    if palette.query.as_deref() != Some(query.as_str()) {
        let hits: Vec<usize> = search(&palette.entries, &query)
            .into_iter()
            .take(SHOWN)
            .map(|e| {
                palette
                    .entries
                    .iter()
                    .position(|x| std::ptr::eq(x, e))
                    .expect("a hit is an entry")
            })
            .collect();
        palette.results = hits;
        palette.selected = 0;
        palette.query = Some(query);
    }
    let mut markup = String::new();
    for (i, &k) in palette.results.iter().enumerate() {
        let e = &palette.entries[k];
        let selected = if i == palette.selected {
            " selected"
        } else {
            ""
        };
        markup.push_str(&format!(
            r#"<row id="cp-item-{i}" class="cp-item{selected}"><label class="cp-label" text="{}"/><spacer/><label class="cp-keys" text="{}"/><label class="cp-group" text="{}"/></row>"#,
            escape(&e.label),
            escape(&e.keys),
            escape(&e.group),
        ));
    }
    if palette.results.is_empty() {
        markup = r#"<label class="cp-empty" text="Nothing matches"/>"#.to_string();
    }
    if palette.shown.as_deref() != Some(markup.as_str())
        && let Some(list) = dom::get_by_id("cp-list")
    {
        list.set_inner_markup(&markup);
        palette.shown = Some(markup);
        crate::repaint_soon(&wake);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, group: &str) -> Entry {
        Entry {
            label: label.to_string(),
            group: group.to_string(),
            keys: String::new(),
            run: Run::Command(label.to_string()),
        }
    }

    #[test]
    fn letters_in_order_match_and_others_do_not() {
        assert!(score("sva", "Save as").is_some());
        assert!(score("SAVE", "Save as").is_some());
        assert!(score("sx", "Save as").is_none());
        assert!(score("vas", "Save").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn a_word_start_and_a_run_beat_scattered_letters() {
        let start = score("exp", "Export PNG").unwrap();
        let inside = score("exp", "Next page").unwrap_or(i32::MIN);
        assert!(start > inside);
        assert!(score("sa", "Save as").unwrap() > score("sa", "Show all panels").unwrap());
    }

    #[test]
    fn search_puts_the_best_first_and_reads_the_group_too() {
        let all = vec![
            entry("Zoom in", "View"),
            entry("Save", "File"),
            entry("Save as", "File"),
            entry("Retro theme", "Theme"),
        ];
        let hits: Vec<&str> = search(&all, "save")
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(hits, ["Save", "Save as"]);
        let hits: Vec<&str> = search(&all, "theme")
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(hits, ["Retro theme"]);
        let hits: Vec<&str> = search(&all, "view zoom")
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(hits, ["Zoom in"]);
        assert_eq!(search(&all, "").len(), 4);
    }
}
