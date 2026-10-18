//! Performance tracking: what each frame cost, shown in an overlay over the
//! canvas (F3, or the performance settings) and written per frame to a CSV
//! file with `--perf-log <file>`.
//!
//! Lumen tells an app when each tick starts but not when its frame reached
//! the screen (lumen-fx/lumen#527), so the times here are read at tick
//! boundaries: a frame's time is from its tick's start to the next frame's,
//! which includes the render and the present between them, and a stroke's
//! latency is from the tick that took the stroke to the start of the tick
//! after its frame. While the overlay shows, a stroke asks for that next
//! tick at once so the measure is not stretched by idle time.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lumenui::ecs::system::{
    CombinatorSystem, Combine, In, IntoSystem, RunSystemError, System, SystemIn,
};
use lumenui::lumen_core::components::Transform;
use lumenui::lumen_core::render_world::FrameDirty;
use lumenui::lumen_core::tick::{Tick, WakeDeadline};
use lumenui::prelude::*;
use lumenui_canvas::{Canvas, Context2d};
use pixl_core::{Document, ImageId, LayerKind};

use crate::editor::Editor;
use crate::keymap::KeyCommands;
use crate::settings::Settings;
use crate::toolbox::Toolbox;
use crate::tools::Gesture;
use crate::view::{Layers, parse_hex};

/// How far back the overlay's numbers and graph look.
pub const WINDOW: Duration = Duration::from_secs(3);

/// How often the overlay's text and graph are redrawn.
pub const REFRESH: Duration = Duration::from_millis(250);

/// A gap between frames longer than this is idle time, not a frame time.
pub const IDLE_GAP: Duration = Duration::from_millis(250);

/// How often the document's memory is counted again.
const COUNT_MEMORY_EVERY: Duration = Duration::from_millis(500);

/// One rendered frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameRecord {
    /// When its tick started.
    pub start: Instant,
    /// From its tick's start to the next frame's; `None` until that frame,
    /// and for a frame after idle time.
    pub time: Option<Duration>,
    /// The tick's work: from the first of the editor's systems to the end
    /// of the tick's main-world stages, Lumen's style, layout and the rest
    /// included.
    pub work: Duration,
    /// The editor's own systems' part of it.
    pub systems: Duration,
    /// Bringing the layer buffers up to date and drawing the stage.
    pub composite: Duration,
    /// Tiles written into layer buffers.
    pub tiles: u64,
}

/// What the overlay shows of the document's memory.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Memory {
    /// Layer buffers the renderer draws.
    pub buffers: usize,
    /// Their pixels, in bytes.
    pub buffer_bytes: u64,
    /// The document's tiles on the CPU, in bytes; shared tiles count once
    /// per image that holds them.
    pub cpu_bytes: u64,
    /// The undo history, in bytes.
    pub history_bytes: u64,
}

/// Frame times and the other numbers the overlay shows.
#[derive(Resource)]
pub struct Perf {
    pub frames: VecDeque<FrameRecord>,
    /// The tick in progress: when it started and when the editor's first
    /// system ran.
    tick_start: Option<Instant>,
    systems_start: Option<Instant>,
    composite_start: Option<Instant>,
    composite: Duration,
    tiles_seen: u64,
    /// A stroke taken this tick, waiting for its frame to be shown.
    stroke_pending: Option<Instant>,
    /// The tick that took the last stroke whose frame was shown, and how
    /// long that took.
    pub last_stroke: Option<Duration>,
    pub memory: Memory,
    memory_counted: Option<Instant>,
    /// The overlay as last drawn.
    shown: Option<Instant>,
    /// Frames since the start, for the log.
    pub count: u64,
    started: Instant,
    log: Option<PerfLog>,
}

impl Default for Perf {
    fn default() -> Self {
        Self {
            frames: VecDeque::new(),
            tick_start: None,
            systems_start: None,
            composite_start: None,
            composite: Duration::ZERO,
            tiles_seen: 0,
            stroke_pending: None,
            last_stroke: None,
            memory: Memory::default(),
            memory_counted: None,
            shown: None,
            count: 0,
            started: Instant::now(),
            log: None,
        }
    }
}

impl Perf {
    /// Tracking that also writes every frame to `log`.
    pub fn logging(log: PerfLog) -> Self {
        Self {
            log: Some(log),
            ..Self::default()
        }
    }

    /// Records that the tick at `start` drew a frame, ending the time of the
    /// frame before it.
    pub fn frame(&mut self, record: FrameRecord) {
        if let Some(last) = self.frames.back_mut() {
            let gap = record.start.saturating_duration_since(last.start);
            last.time = (gap <= IDLE_GAP).then_some(gap);
        }
        self.frames.push_back(record);
        self.count += 1;
        while self
            .frames
            .front()
            .is_some_and(|f| record.start.saturating_duration_since(f.start) > WINDOW)
        {
            self.frames.pop_front();
        }
    }

    /// The frame times of the window, oldest first.
    pub fn times(&self) -> Vec<Duration> {
        self.frames.iter().filter_map(|f| f.time).collect()
    }

    /// Frames drawn in the last second before `now`.
    pub fn fps(&self, now: Instant) -> usize {
        self.frames
            .iter()
            .filter(|f| now.saturating_duration_since(f.start) <= Duration::from_secs(1))
            .count()
    }

    /// Average, 95th percentile and longest frame time of the window.
    pub fn summary(&self) -> Option<(Duration, Duration, Duration)> {
        summarize(&self.times())
    }
}

/// Average, 95th percentile and longest of `times`.
pub fn summarize(times: &[Duration]) -> Option<(Duration, Duration, Duration)> {
    if times.is_empty() {
        return None;
    }
    let mut sorted = times.to_vec();
    sorted.sort();
    let avg = sorted.iter().sum::<Duration>() / sorted.len() as u32;
    let p95 = sorted[((sorted.len() - 1) as f64 * 0.95).round() as usize];
    Some((avg, p95, *sorted.last().expect("not empty")))
}

/// What each of the editor's systems cost: the tick in progress, the last
/// tick, and every tick since the start. Each system the editor adds runs
/// through [`timed`], which adds its time here.
#[derive(Resource, Default)]
pub struct SystemTimes {
    current: Vec<(&'static str, Duration)>,
    /// The systems that ran in the last tick, in the order they ran.
    pub last: Vec<(&'static str, Duration)>,
    /// Per system: total time, ticks it ran and its longest run.
    pub totals: std::collections::BTreeMap<&'static str, (Duration, u64, Duration)>,
}

impl SystemTimes {
    fn add(&mut self, name: &'static str, time: Duration) {
        self.current.push((name, time));
        let t = self.totals.entry(name).or_default();
        t.0 += time;
        t.1 += 1;
        t.2 = t.2.max(time);
    }

    /// The editor's systems' time in the last tick.
    pub fn last_total(&self) -> Duration {
        self.last.iter().map(|(_, t)| *t).sum()
    }

    fn roll(&mut self) {
        self.last = std::mem::take(&mut self.current);
    }
}

/// Runs a system and then adds its time to [`SystemTimes`].
pub struct Timing;

impl<A, B> Combine<A, B> for Timing
where
    A: System<In = (), Out = ()>,
    B: System<In = In<Duration>, Out = ()>,
{
    type In = ();
    type Out = ();

    fn combine<T>(
        _input: (),
        data: &mut T,
        a: impl FnOnce(SystemIn<'_, A>, &mut T) -> std::result::Result<(), RunSystemError>,
        b: impl FnOnce(SystemIn<'_, B>, &mut T) -> std::result::Result<(), RunSystemError>,
    ) -> std::result::Result<(), RunSystemError> {
        let start = Instant::now();
        let ran = a((), data);
        b(start.elapsed(), data)?;
        ran
    }
}

/// `system`, timed under `name` into [`SystemTimes`]. It keeps the
/// system's own sets, so ordering against the plain system still holds.
pub fn timed<M>(
    name: &'static str,
    system: impl IntoSystem<(), (), M>,
) -> impl System<In = (), Out = ()> {
    let a = IntoSystem::into_system(system);
    let b = IntoSystem::into_system(
        move |In(time): In<Duration>, times: Option<ResMut<SystemTimes>>| {
            if let Some(mut times) = times {
                times.add(name, time);
            }
        },
    );
    let label = a.name();
    CombinatorSystem::<Timing, _, _>::new(a, b, label)
}

/// Each system given, [`timed`] under its path.
#[macro_export]
macro_rules! timed {
    ($($system:path),* $(,)?) => {
        ($($crate::perf::timed(stringify!($system), $system),)*)
    };
}

/// The per-frame CSV file `--perf-log` writes.
pub struct PerfLog {
    out: BufWriter<File>,
    flushed: Instant,
}

/// The CSV header of a performance log.
pub const LOG_HEADER: &str = "frame,start_ms,frame_ms,work_ms,systems_ms,composite_ms,tiles,buffers,buffer_mib,queued,stroke_ms";

impl PerfLog {
    pub fn create(path: &Path) -> std::io::Result<PerfLog> {
        let mut out = BufWriter::new(File::create(path)?);
        writeln!(out, "{LOG_HEADER}")?;
        Ok(PerfLog {
            out,
            flushed: Instant::now(),
        })
    }

    fn row(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        if self.flushed.elapsed() >= Duration::from_secs(1) {
            let _ = self.out.flush();
            self.flushed = Instant::now();
        }
    }
}

impl Drop for PerfLog {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

/// The `--perf-log <file>` of a command line, and the arguments left.
pub fn take_perf_log(args: Vec<std::ffi::OsString>) -> (Option<PathBuf>, Vec<std::ffi::OsString>) {
    let mut log = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--perf-log" {
            log = it.next().map(PathBuf::from);
        } else if let Some(path) = a.to_str().and_then(|s| s.strip_prefix("--perf-log=")) {
            log = Some(PathBuf::from(path));
        } else {
            rest.push(a);
        }
    }
    (log, rest)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Marks where the tick's own systems start. Runs first in the editor's
/// chain.
pub fn tick_begin(mut perf: ResMut<Perf>, tick: Option<Res<Tick>>) {
    let now = Instant::now();
    perf.tick_start = Some(tick.map_or(now, |t| t.now));
    perf.systems_start = Some(now);
    perf.composite = Duration::ZERO;
}

/// Marks the start of the layer and stage drawing.
pub fn composite_begin(mut perf: ResMut<Perf>) {
    perf.composite_start = Some(Instant::now());
}

/// Marks the end of the layer and stage drawing.
pub fn composite_end(mut perf: ResMut<Perf>) {
    if let Some(start) = perf.composite_start.take() {
        perf.composite += start.elapsed();
    }
}

/// Notes a stroke taken this tick: the document changed while a tool's
/// gesture was under way.
pub fn note_stroke(
    editor: Res<Editor>,
    gesture: Res<Gesture>,
    toolbox: Res<Toolbox>,
    mut perf: ResMut<Perf>,
    mut seen: Local<(u64, u64)>,
) {
    let now = (editor.generation, editor.revision);
    if *seen != now {
        let drawing = gesture.active() || toolbox.active();
        if seen.0 == now.0 && drawing && perf.stroke_pending.is_none() {
            perf.stroke_pending = perf.tick_start;
        }
        *seen = now;
    }
}

/// Closes the tick: records its frame if it drew one, and writes the log.
/// Runs at the end of the tick, after Lumen has decided whether it renders.
pub fn tick_end(
    mut perf: ResMut<Perf>,
    dirty: Option<Res<FrameDirty>>,
    layers: Res<Layers>,
    settings: Res<Settings>,
    wake: Option<Res<WakeDeadline>>,
    times: Option<ResMut<SystemTimes>>,
) {
    let own = times.map_or(Duration::ZERO, |mut t| {
        t.roll();
        t.last_total()
    });
    let (Some(start), Some(systems)) = (perf.tick_start, perf.systems_start) else {
        return;
    };
    // A stroke's frame was shown before this tick started.
    if let Some(stroke) = perf.stroke_pending
        && stroke < start
    {
        perf.last_stroke = Some(start.saturating_duration_since(stroke));
        perf.stroke_pending = None;
    }
    let drew = dirty.is_none_or(|d| d.dirty);
    let tiles = layers.tiles_written.saturating_sub(perf.tiles_seen);
    perf.tiles_seen = layers.tiles_written;
    if !drew {
        return;
    }
    let record = FrameRecord {
        start,
        time: None,
        work: systems.elapsed(),
        systems: own,
        composite: perf.composite,
        tiles,
    };
    perf.frame(record);
    if perf.stroke_pending == Some(start) && settings.performance.show_fps {
        // Tick again as soon as this frame is shown, to time the stroke.
        crate::repaint_soon(&wake);
    }
    if perf.log.is_some() {
        let (buffers, bytes) = layers.buffer_stats();
        let queued = layers.pending();
        let stroke = perf
            .last_stroke
            .filter(|_| perf.stroke_pending.is_none())
            .map_or(String::new(), |d| format!("{:.3}", ms(d)));
        // The frame before this one now knows its time.
        let previous = perf
            .frames
            .iter()
            .rev()
            .nth(1)
            .and_then(|f| f.time)
            .map_or(String::new(), |d| format!("{:.3}", ms(d)));
        let line = format!(
            "{},{:.3},{},{:.3},{:.3},{:.3},{},{},{:.1},{},{}",
            perf.count,
            ms(start.saturating_duration_since(perf.started)),
            previous,
            ms(record.work),
            ms(record.systems),
            ms(record.composite),
            tiles,
            buffers,
            bytes as f64 / (1024.0 * 1024.0),
            queued,
            stroke,
        );
        if let Some(log) = perf.log.as_mut() {
            log.row(&line);
        }
    }
}

/// The document's tiles on the CPU, in bytes, each image counted once.
pub fn document_bytes(doc: &Document) -> u64 {
    let mut seen: std::collections::BTreeSet<ImageId> = std::collections::BTreeSet::new();
    let mut total = 0u64;
    for (_, layer) in doc.walk() {
        let ids: Vec<ImageId> = match &layer.kind {
            LayerKind::Raster { cels, mask } => cels
                .iter()
                .flatten()
                .map(|c| c.image)
                .chain(mask.map(|m| m.image))
                .collect(),
            LayerKind::Reference { image, .. } => vec![*image],
            _ => Vec::new(),
        };
        for id in ids {
            if seen.insert(id)
                && let Some(img) = doc.image(id)
            {
                total += img.memory_bytes() as u64;
            }
        }
    }
    total
}

/// Bytes as MiB, with a decimal under ten.
fn mib(bytes: u64) -> String {
    let m = bytes as f64 / (1024.0 * 1024.0);
    if m < 10.0 {
        format!("{m:.1}")
    } else {
        format!("{m:.0}")
    }
}

/// What the overlay's lines say.
pub fn overlay_lines(perf: &Perf, layers: &Layers, renderer: &str, now: Instant) -> Vec<String> {
    let fps = perf.fps(now);
    let mut lines = Vec::new();
    lines.push(match perf.summary() {
        Some((avg, p95, max)) if fps > 0 => format!(
            "{fps} fps  frame {:.1} / p95 {:.1} / max {:.1} ms",
            ms(avg),
            ms(p95),
            ms(max)
        ),
        _ => "idle: no frames in the last second".to_string(),
    });
    let last = perf.frames.back();
    lines.push(format!(
        "work {:.1} ms  editor {:.1} ms  composite {:.1} ms  tiles {}",
        last.map_or(0.0, |f| ms(f.work)),
        last.map_or(0.0, |f| ms(f.systems)),
        last.map_or(0.0, |f| ms(f.composite)),
        last.map_or(0, |f| f.tiles),
    ));
    lines.push(match perf.last_stroke {
        Some(d) => format!("stroke to screen {:.1} ms", ms(d)),
        None => "stroke to screen: no stroke yet".to_string(),
    });
    let m = perf.memory;
    lines.push(format!(
        "buffers {} ({} MiB)  tiles {} MiB  undo {} MiB",
        m.buffers,
        mib(m.buffer_bytes),
        mib(m.cpu_bytes),
        mib(m.history_bytes),
    ));
    let gpu = if renderer.starts_with("CPU") {
        String::new()
    } else {
        // Each buffer is a texture of its own, and vello samples images
        // through one 8192x8192 atlas.
        format!("  GPU about {} MiB", mib(m.buffer_bytes + 8192 * 8192 * 4))
    };
    lines.push(format!(
        "workers {}  queued {}{gpu}",
        layers.workers(),
        layers.pending(),
    ));
    lines.push(format!("renderer {renderer}"));
    lines
}

/// Keeps the overlay showing: its text and the frame-time graph, a few
/// times a second, and its visibility from the settings (F3 flips it).
#[allow(clippy::too_many_arguments)]
pub fn show_perf(
    mut perf: ResMut<Perf>,
    mut settings: ResMut<Settings>,
    keys: Res<KeyCommands>,
    editor: Res<Editor>,
    layers: Res<Layers>,
    canvases: Query<(&Canvas, &Transform)>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
    mut renderer: Local<Option<String>>,
) {
    if keys.ran("toggle-perf", false) {
        let p = &mut settings.performance;
        p.show_fps = !p.show_fps;
    }
    let show = settings.performance.show_fps;
    if signals.get::<bool>("perf_open") != Some(show) {
        signals.set("perf_open", show);
        perf.shown = None;
        crate::repaint_soon(&wake);
    }
    if !show {
        return;
    }
    let now = Instant::now();
    if perf
        .memory_counted
        .is_none_or(|t| now.saturating_duration_since(t) >= COUNT_MEMORY_EVERY)
    {
        let (buffers, buffer_bytes) = layers.buffer_stats();
        perf.memory = Memory {
            buffers,
            buffer_bytes,
            cpu_bytes: document_bytes(&editor.doc),
            history_bytes: editor.history.memory_bytes() as u64,
        };
        perf.memory_counted = Some(now);
    }
    let due = perf
        .shown
        .is_none_or(|t| now.saturating_duration_since(t) >= REFRESH);
    if let Some(wake) = &wake {
        wake.request(perf.shown.map_or(now, |t| t + REFRESH));
    }
    if !due {
        return;
    }
    perf.shown = Some(now);
    let renderer = renderer.get_or_insert_with(crate::settings_ui::renderer_text);
    for (i, line) in overlay_lines(&perf, &layers, renderer, now)
        .into_iter()
        .enumerate()
    {
        crate::tools::set_if_changed(&mut signals, &format!("perf_line_{i}"), line);
    }
    if let Some((_, t)) = canvases.iter().find(|(c, _)| c.id == "perf-graph") {
        draw_graph(&perf, f64::from(t.size.x), f64::from(t.size.y));
    }
}

/// The frame times of the window as bars, the longest frames tallest, with
/// a rule at 60 frames a second, in the theme's colors.
fn draw_graph(perf: &Perf, w: f64, h: f64) {
    if w < 4.0 || h < 4.0 {
        return;
    }
    let probe = |id: &str| {
        dom::get_by_id(id)?
            .computed_style_of("bg")
            .as_deref()
            .and_then(parse_hex)
    };
    let (Some(bar), Some(slow), Some(rule)) = (
        probe("probe-perf-bar"),
        probe("probe-perf-slow"),
        probe("probe-perf-rule"),
    ) else {
        return;
    };
    let ctx = Context2d::new("perf-graph");
    ctx.resize(w as f32, h as f32);
    ctx.clear();
    let budget = 1000.0 / 60.0;
    let top = 3.0 * budget;
    let color = |c: pixl_core::Rgba| {
        lumenui_canvas::Rgba::new(
            f64::from(c.r) / 255.0,
            f64::from(c.g) / 255.0,
            f64::from(c.b) / 255.0,
            f64::from(c.a) / 255.0,
        )
    };
    let times = perf.times();
    let n = times.len().min(w as usize / 2);
    for (i, t) in times[times.len() - n..].iter().enumerate() {
        let v = ms(*t).min(top);
        let bh = (v / top * h).max(1.0);
        ctx.set_fill_color(color(if ms(*t) > budget { slow } else { bar }));
        ctx.fill_rect(w - (n - i) as f64 * 2.0, h - bh, 1.0, bh);
    }
    ctx.set_fill_color(color(rule));
    ctx.fill_rect(0.0, (h - budget / top * h).round(), w, 1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> FrameRecord {
        FrameRecord {
            start: t0 + Duration::from_millis(ms),
            time: None,
            work: Duration::ZERO,
            systems: Duration::ZERO,
            composite: Duration::ZERO,
            tiles: 0,
        }
    }

    #[test]
    fn a_frame_time_runs_to_the_next_frame_and_idle_time_is_left_out() {
        let t0 = Instant::now();
        let mut p = Perf::default();
        for ms in [0, 16, 33, 50, 1000, 1016] {
            p.frame(at(t0, ms));
        }
        let times: Vec<u128> = p.times().iter().map(Duration::as_millis).collect();
        assert_eq!(times, vec![16, 17, 17, 16]);
        assert_eq!(p.fps(t0 + Duration::from_millis(2000)), 2);
    }

    #[test]
    fn the_window_keeps_the_last_seconds() {
        let t0 = Instant::now();
        let mut p = Perf::default();
        for i in 0..400 {
            p.frame(at(t0, i * 16));
        }
        let span = p.frames.back().unwrap().start - p.frames.front().unwrap().start;
        assert!(span <= WINDOW);
    }

    #[test]
    fn the_summary_is_average_p95_and_max() {
        let times: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        let (avg, p95, max) = summarize(&times).unwrap();
        assert_eq!(avg, Duration::from_micros(50500));
        assert_eq!(p95, Duration::from_millis(95));
        assert_eq!(max, Duration::from_millis(100));
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn the_perf_log_flag_is_taken_from_the_arguments() {
        let args = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let (log, rest) = take_perf_log(args(&["--perf-log", "f.csv", "art.pixl"]));
        assert_eq!(log, Some(PathBuf::from("f.csv")));
        assert_eq!(rest, args(&["art.pixl"]));
        let (log, rest) = take_perf_log(args(&["--perf-log=g.csv"]));
        assert_eq!(log, Some(PathBuf::from("g.csv")));
        assert!(rest.is_empty());
    }

    #[test]
    fn document_memory_counts_each_image_once() {
        let mut doc = Document::new(128, 64, pixl_core::ColorMode::Rgba).unwrap();
        assert_eq!(document_bytes(&doc), 0);
        let l = doc.layers()[0].id;
        let t = doc.target(l, 0).unwrap();
        t.raster.fill_rect_raw(
            &pixl_core::Rect::new(0, 0, 1, 1),
            pixl_core::Rgba::BLACK.to_u32(),
        );
        let one = document_bytes(&doc);
        assert!(one >= 64 * 64 * 4, "{one}");
    }
}
