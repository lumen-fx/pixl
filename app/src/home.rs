//! The start screen: what pixl shows when it opens without a file, after
//! File > Close, and from the Home button. It lists the recent documents
//! and offers a new project or a file to open.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::native;

use crate::editor::Editor;
use crate::panels::escape;
use crate::recents::{Recents, Thumb, key};

/// Whether the start screen shows, and what it last showed.
#[derive(Resource, Debug, Default)]
pub struct Home {
    pub open: bool,
    /// The editor holds a document the screen can go back to.
    pub can_return: bool,
    /// A file to open on the first tick: the one named on the command line.
    pub pending: Option<PathBuf>,
    /// The document generation the screen last saw. A new document closes
    /// the screen.
    seen_generation: u64,
    shown: Option<String>,
}

impl Home {
    /// The start screen, showing.
    pub fn start() -> Self {
        Self {
            open: true,
            ..Self::default()
        }
    }

    /// No start screen: `path` opens in the editor straight away.
    pub fn opening(path: PathBuf) -> Self {
        Self {
            pending: Some(path),
            ..Self::default()
        }
    }

    /// Shows the screen over the editor's current document.
    pub fn show(&mut self, editor: &Editor, can_return: bool) {
        self.open = true;
        self.can_return = can_return;
        self.seen_generation = editor.generation;
    }
}

/// Lists a document on the start screen when it is opened or saved.
pub fn track_recents(
    editor: Res<Editor>,
    mut recents: ResMut<Recents>,
    mut seen: Local<Option<(u64, u64)>>,
) {
    let now = (editor.generation, editor.saved_revision);
    // The document the editor starts with (generation 0) was not opened
    // from anything; one opened before this first look (the file named on
    // the command line) was.
    let first = if editor.generation == 0 { now } else { (0, 0) };
    let before = seen.replace(now).unwrap_or(first);
    if before == now {
        return;
    }
    let Some(path) = editor.recent_path() else {
        return;
    };
    let thumb = if before.0 == now.0 {
        Thumb::Redraw
    } else {
        Thumb::IfStale
    };
    recents.note(&path.clone(), &editor.doc, thumb);
}

/// Keeps the start screen showing the recent documents, and closes it when
/// a document opens.
pub fn show_home(
    editor: Res<Editor>,
    mut home: ResMut<Home>,
    mut recents: ResMut<Recents>,
    settings: Res<crate::settings::Settings>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    if recents.limit != settings.files.recent_count {
        recents.limit = settings.files.recent_count;
        home.shown = None;
    }
    if editor.generation != home.seen_generation {
        home.seen_generation = editor.generation;
        home.open = false;
    }
    recents.collect_thumbnails();
    if recents.drawing()
        && let Some(wake) = &wake
    {
        // Look again soon, to show each thumbnail as it is done.
        wake.request(Instant::now() + Duration::from_millis(50));
    }
    if signals.get::<bool>("home_can_return") != Some(home.can_return) {
        signals.set("home_can_return", home.can_return);
    }
    if !home.open {
        home.shown = None;
        return;
    }
    let filter = signals.get::<String>("home_filter").unwrap_or_default();
    let markup = recents_markup(&recents, &filter, SystemTime::now());
    if home.shown.as_deref() == Some(markup.as_str()) {
        return;
    }
    if let Some(list) = dom::get_by_id("recent-list") {
        list.set_inner_markup(&markup);
        home.shown = Some(markup);
        crate::repaint_soon(&wake);
    }
}

/// The recent documents whose name or folder holds `filter`, as markup.
pub fn recents_markup(recents: &Recents, filter: &str, now: SystemTime) -> String {
    if recents.entries.is_empty() {
        return r#"<label class="recent-empty" text="No recent documents yet. Start a new project or open a file."/>"#.to_string();
    }
    let filter = filter.trim().to_lowercase();
    let mut out = String::new();
    for entry in recents.shown() {
        let name = entry
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let folder = folder_of(&entry.path);
        if !filter.is_empty()
            && !name.to_lowercase().contains(&filter)
            && !folder.to_lowercase().contains(&filter)
        {
            continue;
        }
        let k = key(&entry.path);
        let modified = std::fs::metadata(&entry.path).and_then(|m| m.modified());
        let (class, meta) = match modified {
            Ok(time) => (
                "recent",
                format!("{} x {} px, {}", entry.width, entry.height, ago(time, now)),
            ),
            Err(_) => (
                "recent missing",
                "Missing: the file is not there".to_string(),
            ),
        };
        let thumb = match recents.thumb_path(entry) {
            Some(t) => format!(
                r#"<tile class="thumb-frame"><image class="thumb" src="{}" fit="contain" alt=""/></tile>"#,
                escape(&t.to_string_lossy())
            ),
            None => r#"<tile class="thumb-frame empty"/>"#.to_string(),
        };
        let mut actions = String::new();
        if modified.is_ok() && native::find_recovery(&entry.path).is_some() {
            actions.push_str(&format!(
                r#"<label class="recent-flag" text="Unsaved edits"/><button id="recent-recover-{k}" class="small primary" text="Recover"/>"#
            ));
        }
        if modified.is_err() {
            actions.push_str(&format!(
                r#"<button id="recent-remove-{k}" class="small" text="Remove"/>"#
            ));
        }
        out.push_str(&format!(
            r#"<row id="recent-{k}" class="{class}">{thumb}<column class="recent-text"><label class="recent-name" text="{name}"/><label class="recent-folder" text="{folder}"/><label class="recent-meta" text="{meta}"/></column><spacer/>{actions}</row>"#,
            name = escape(&name),
            folder = escape(&folder),
        ));
    }
    if out.is_empty() {
        out.push_str(
            r#"<label class="recent-empty" text="No recent document matches the search."/>"#,
        );
    }
    out
}

/// The folder a document is in, with the home directory shown as `~`.
fn folder_of(path: &Path) -> String {
    let folder = path.parent().unwrap_or(Path::new(""));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = folder.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    folder.display().to_string()
}

/// How long ago `time` was, as the list shows it.
pub fn ago(time: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(time).map_or(0, |d| d.as_secs());
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match secs {
        0..60 => "just now".to_string(),
        60..3600 => plural(secs / 60, "minute"),
        3600..86_400 => plural(secs / 3600, "hour"),
        86_400..172_800 => "yesterday".to_string(),
        172_800..2_592_000 => plural(secs / 86_400, "day"),
        _ => date(time),
    }
}

/// The calendar date of `time`, in UTC, as `YYYY-MM-DD`.
fn date(time: SystemTime) -> String {
    let days = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) / 86_400;
    // Howard Hinnant's days-to-civil conversion.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_how_long_ago() {
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let at = |secs: u64| now - Duration::from_secs(secs);
        assert_eq!(ago(at(5), now), "just now");
        assert_eq!(ago(at(60), now), "1 minute ago");
        assert_eq!(ago(at(7200), now), "2 hours ago");
        assert_eq!(ago(at(90_000), now), "yesterday");
        assert_eq!(ago(at(3 * 86_400), now), "3 days ago");
        // 1_800_000_000 s after the epoch is 2027-01-15; 40 days before it:
        assert_eq!(ago(at(40 * 86_400), now), "2026-12-06");
    }
}
