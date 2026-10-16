//! The tools, driven by the pointer over the stage and carried out by
//! pixl-core: freehand strokes, shapes, the bucket, the eyedropper, the
//! rectangle selection and moving pixels.

use lumenui::ecs::system::SystemParam;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{ModifiersState, PointerButton, PointerData, PointerKind};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::history::Transaction;
use pixl_core::tools::fill::{self, Connectivity, FillOptions};
use pixl_core::tools::paint::{self, PencilStroke};
use pixl_core::tools::shapes;
use pixl_core::transform::Pixels;
use pixl_core::{
    Brush, Canvas, Color, CompositeOptions, Error, Ink, Point, Raster, Rect, Rgba, Scope, SelectOp,
    Selection, ToolContext, composite, edit_cel,
};

use crate::editor::{Editor, Tool};
use crate::input::{KeyInput, Navigation, Pointer, PointerInput, key_char};
use crate::view::{Preview, View};

/// A gesture in progress, from the press that started it to the release.
#[derive(Resource, Default)]
pub struct Gesture(Option<Session>);

impl Gesture {
    pub fn active(&self) -> bool {
        self.0.is_some()
    }
}

/// What a press started.
enum Session {
    /// A freehand stroke of the pencil or the eraser.
    Stroke {
        button: PointerButton,
        ink: Ink,
        ctx: ToolContext,
        tx: Option<Transaction>,
        line: Line,
        last: Point,
    },
    /// A line, rectangle or ellipse from where the press landed.
    Shape {
        button: PointerButton,
        ink: Ink,
        tool: Tool,
        from: Point,
        to: Point,
    },
    /// Picking colors while the button is down.
    Pick { button: PointerButton },
    /// Dragging out a selection rectangle.
    Select {
        button: PointerButton,
        op: SelectOp,
        before: Option<Selection>,
        from: Point,
        to: Point,
    },
    /// Dragging pixels: the lifted area and the cel as it was.
    Move {
        button: PointerButton,
        from: Point,
        offset: Point,
        rect: Rect,
        original: Raster,
        selected: bool,
        tx: Option<Transaction>,
    },
}

impl Session {
    fn button(&self) -> PointerButton {
        match self {
            Session::Stroke { button, .. }
            | Session::Shape { button, .. }
            | Session::Pick { button }
            | Session::Select { button, .. }
            | Session::Move { button, .. } => *button,
        }
    }
}

/// How a freehand stroke lays its pixels down.
enum Line {
    /// One brush for the whole stroke, through pixl-core's stroke, which
    /// drops L-shaped corners when pixel-perfect is on.
    Steady(PencilStroke),
    /// A brush sized by pen pressure at each point.
    Pressure { size: u32 },
}

/// The brush of a size: square up to two pixels, round from three.
pub fn brush(size: u32) -> Brush {
    if size >= 3 {
        Brush::circle(size)
    } else {
        Brush::square(size)
    }
}

/// The brush size a pen press of `pressure` paints with.
pub fn pressure_size(size: u32, pressure: f32) -> u32 {
    ((size as f32 * pressure.clamp(0.0, 1.0)).round() as u32).clamp(1, size.max(1))
}

/// The color a button paints: the foreground for the primary button, the
/// background for the secondary one.
fn paint_color(editor: &Editor, button: PointerButton) -> Rgba {
    if button == PointerButton::Secondary {
        editor.bg
    } else {
        editor.fg
    }
}

fn ink_for(editor: &Editor, tool: Tool, button: PointerButton, eraser_end: bool) -> Ink {
    if tool == Tool::Eraser || eraser_end {
        Ink::Erase
    } else {
        Ink::Paint(Color::Rgba(paint_color(editor, button)))
    }
}

fn context(editor: &Editor) -> ToolContext {
    ToolContext {
        selection: editor.selection.clone(),
        symmetry: Default::default(),
    }
}

/// Explains a refused edit in the status bar.
fn refuse(editor: &mut Editor, e: Error) {
    editor.message = Some(match e {
        Error::Locked(_) => "The layer is locked".to_string(),
        Error::NotRaster(_) => "Pick a raster layer to paint on".to_string(),
        other => other.to_string(),
    });
}

/// The pixels of a shape from `a` to `b`, for the preview and the edit.
fn shape_pixels(tool: Tool, a: Point, b: Point, filled: bool, size: u32) -> Vec<Point> {
    let base = match tool {
        Tool::Line => shapes::line(a, b),
        Tool::Rectangle if filled => Rect::from_corners(a, b).points().collect(),
        Tool::Rectangle => shapes::rect_outline(a, b),
        Tool::Ellipse if filled => shapes::ellipse_filled(a, b),
        Tool::Ellipse => shapes::ellipse_outline(a, b),
        _ => Vec::new(),
    };
    if size <= 1 || filled {
        return base;
    }
    let offsets = brush(size).offsets();
    let mut out: Vec<Point> = base
        .iter()
        .flat_map(|p| {
            offsets
                .iter()
                .map(move |o| Point::new(p.x + o.x, p.y + o.y))
        })
        .collect();
    out.sort_by_key(|p| (p.y, p.x));
    out.dedup();
    out
}

/// Where a shape ends: with shift held, a line snaps to clean pixel steps
/// and a rectangle or ellipse to a square or circle.
fn constrain(tool: Tool, from: Point, to: Point, shift: bool) -> Point {
    if !shift {
        return to;
    }
    match tool {
        Tool::Line => shapes::snap_line(from, to),
        Tool::Rectangle | Tool::Ellipse => {
            let (dx, dy) = (to.x - from.x, to.y - from.y);
            let side = dx.abs().max(dy.abs());
            Point::new(from.x + side * dx.signum(), from.y + side * dy.signum())
        }
        _ => to,
    }
}

/// The color of the composited document at `p`, as the view shows it.
pub fn color_at(editor: &Editor, p: Point) -> Option<Rgba> {
    if !editor.doc.bounds().contains(p.x, p.y) {
        return None;
    }
    let opts = CompositeOptions {
        include_reference: false,
        background: true,
        onion: None,
    };
    composite(&editor.doc, editor.frame, Rect::pixel(p.x, p.y), &opts)
        .first()
        .copied()
}

/// Picks the tool for a key, the way the tool bar's tooltips say.
pub fn tool_for_key(key: &str, shift: bool) -> Option<Tool> {
    Some(match key {
        "b" => Tool::Pencil,
        "e" => Tool::Eraser,
        "l" => Tool::Line,
        "u" if shift => Tool::Ellipse,
        "u" => Tool::Rectangle,
        "g" => Tool::Bucket,
        "i" => Tool::Eyedropper,
        "m" => Tool::Select,
        "v" => Tool::Move,
        _ => return None,
    })
}

/// Picks tools from their keys and their tool bar buttons.
pub fn pick_tool(
    mut editor: ResMut<Editor>,
    mut keys: KeyInput,
    mut clicks: MessageReader<ClickEvent>,
    ids: Query<&LumenId>,
    gesture: Res<Gesture>,
) {
    let mut picked = None;
    for click in clicks.read() {
        if let Ok(id) = ids.get(click.entity)
            && let Some(tool) = Tool::from_button_id(&id.0)
        {
            picked = Some(tool);
        }
    }
    let typing = keys.typing();
    for k in keys.pressed.read() {
        if typing || k.repeat || k.modifiers.ctrl || k.modifiers.alt || k.modifiers.super_ {
            continue;
        }
        if let Some(c) = key_char(&k.key)
            && let Some(tool) = tool_for_key(&c, k.modifiers.shift)
        {
            picked = Some(tool);
        }
        if key_char(&k.key).as_deref() == Some("x") {
            let editor = &mut *editor;
            std::mem::swap(&mut editor.fg, &mut editor.bg);
        }
    }
    if let Some(tool) = picked
        && tool != editor.tool
        && !gesture.active()
    {
        editor.tool = tool;
    }
}

/// Where the pointer is: the view, the pan state, the modifier keys and
/// the element under the pointer.
#[derive(SystemParam)]
pub struct Place<'w, 's> {
    view: Res<'w, View>,
    nav: Res<'w, Navigation>,
    modifiers: Res<'w, ModifiersState>,
    hovered: Query<'w, 's, Option<&'static LumenId>, With<Hovered>>,
}

/// Runs the active tool over this tick's pointer events.
pub fn use_tools(
    mut editor: ResMut<Editor>,
    mut gesture: ResMut<Gesture>,
    mut preview: ResMut<Preview>,
    place: Place,
    mut pointer: PointerInput,
) {
    let (view, nav) = (&place.view, &place.nav);
    let shift = place.modifiers.0.shift;
    let alt = place.modifiers.0.alt;
    // A popup or a panel over the stage takes the press. With nothing
    // hovered (input that did not come through the window), the stage's
    // own rectangle decides.
    let over_stage = place.hovered.iter().all(|id| {
        id.is_some_and(|id| matches!(id.0.as_str(), "stage" | "checker" | "art" | "overlay"))
    });
    for event in pointer.events() {
        match event {
            Pointer::Down { at, button, data } => {
                if gesture.0.is_some()
                    || nav.space_held
                    || nav.panning.is_some()
                    || !view.contains(at)
                    || !over_stage
                    || !matches!(
                        button,
                        PointerButton::Primary | PointerButton::Secondary | PointerButton::Eraser
                    )
                {
                    continue;
                }
                let p = view.doc_pixel(at);
                let eraser_end = button == PointerButton::Eraser
                    || data.buttons & PointerButton::Eraser.bit() != 0;
                let button = if eraser_end {
                    PointerButton::Primary
                } else {
                    button
                };
                let press = Press {
                    p,
                    button,
                    data,
                    eraser_end,
                    shift,
                    alt,
                };
                gesture.0 = begin(&mut editor, &mut preview, press);
            }
            Pointer::Move { at, data } => {
                let Some(session) = gesture.0.as_mut() else {
                    continue;
                };
                let p = view.doc_pixel(at);
                extend(
                    &mut editor,
                    &mut preview,
                    session,
                    p,
                    data.pressure,
                    data.kind,
                    shift,
                );
            }
            Pointer::Up { at, button, .. } => {
                let ends = gesture
                    .0
                    .as_ref()
                    .is_some_and(|s| s.button() == button || button == PointerButton::Eraser);
                if !ends {
                    continue;
                }
                let session = gesture.0.take().expect("checked above");
                let p = view.doc_pixel(at);
                finish(&mut editor, &mut preview, session, p, shift);
            }
        }
    }
}

/// A press that may start a gesture.
struct Press {
    /// The document pixel under the press.
    p: Point,
    button: PointerButton,
    data: PointerData,
    /// The pen's eraser end, which erases whatever the tool.
    eraser_end: bool,
    shift: bool,
    alt: bool,
}

/// Starts a gesture.
fn begin(editor: &mut Editor, preview: &mut Preview, press: Press) -> Option<Session> {
    let Press {
        p,
        button,
        data,
        eraser_end,
        shift,
        alt,
    } = press;
    let tool = if eraser_end {
        Tool::Eraser
    } else {
        editor.tool
    };
    let (layer, frame) = (editor.layer, editor.frame);
    match tool {
        Tool::Pencil | Tool::Eraser => {
            let ink = ink_for(editor, tool, button, eraser_end);
            let ctx = context(editor);
            let image = match editor.doc.target_image(layer, frame) {
                Ok(image) => image,
                Err(e) => {
                    refuse(editor, e);
                    return None;
                }
            };
            let mut tx =
                editor
                    .history
                    .begin(&editor.doc, tool.label(), Scope::Images(vec![image]));
            tx.watch(&editor.doc, image);
            let size = editor.options.size;
            let pen = data.kind == PointerKind::Pen && editor.options.pressure_size && size > 1;
            let line = if pen {
                Line::Pressure { size }
            } else {
                // The stroke reads the canvas's symmetry to know how many
                // copies it draws.
                match editor.doc.target(layer, frame) {
                    Ok(target) => {
                        let canvas = Canvas::new(target, &ctx);
                        Line::Steady(PencilStroke::new(
                            &canvas,
                            brush(size),
                            ink.clone(),
                            editor.options.pixel_perfect,
                        ))
                    }
                    Err(e) => {
                        refuse(editor, e);
                        return None;
                    }
                }
            };
            let mut session = Session::Stroke {
                button,
                ink,
                ctx,
                tx: Some(tx),
                line,
                last: p,
            };
            stroke_to(editor, &mut session, p, data.pressure);
            Some(session)
        }
        Tool::Line | Tool::Rectangle | Tool::Ellipse => {
            let ink = ink_for(editor, tool, button, false);
            let session = Session::Shape {
                button,
                ink,
                tool,
                from: p,
                to: p,
            };
            show_shape(editor, preview, &session, shift);
            Some(session)
        }
        Tool::Bucket => {
            let ink = ink_for(editor, tool, button, false);
            let opts = FillOptions {
                connectivity: Connectivity::Four,
                tolerance: editor.options.tolerance,
                contiguous: editor.options.contiguous,
            };
            let ctx = context(editor);
            let done = edit_cel(
                &mut editor.doc,
                &mut editor.history,
                "Fill",
                layer,
                frame,
                &ctx,
                |canvas| fill::bucket_fill(canvas, p, &ink, &opts, None),
            );
            match done {
                Ok(((), dirty)) if !dirty.is_empty() => editor.touch(),
                Ok(_) => {}
                Err(e) => refuse(editor, e),
            }
            None
        }
        Tool::Eyedropper => {
            pick(editor, p, button);
            Some(Session::Pick { button })
        }
        Tool::Select => {
            let op = match (shift, alt) {
                (true, _) => SelectOp::Add,
                (false, true) => SelectOp::Subtract,
                _ => SelectOp::Replace,
            };
            Some(Session::Select {
                button,
                op,
                before: editor.selection.clone(),
                from: p,
                to: p,
            })
        }
        Tool::Move => {
            let image = match editor.doc.target_image(layer, frame) {
                Ok(image) => image,
                Err(e) => {
                    refuse(editor, e);
                    return None;
                }
            };
            if let Err(e) = editor.doc.effective_locks(layer).and_then(|l| {
                if l.position {
                    Err(Error::Locked(layer))
                } else {
                    Ok(())
                }
            }) {
                refuse(editor, e);
                return None;
            }
            let original = editor.doc.image(image)?.clone();
            let (rect, selected) = match &editor.selection {
                Some(s) if !s.is_empty() => (s.bounds(), true),
                _ => (original.content_bounds(), false),
            };
            if rect.is_empty() {
                return None;
            }
            let mut tx = editor
                .history
                .begin(&editor.doc, "Move", Scope::Images(vec![image]));
            tx.watch(&editor.doc, image);
            Some(Session::Move {
                button,
                from: p,
                offset: Point::new(0, 0),
                rect,
                original,
                selected,
                tx: Some(tx),
            })
        }
    }
}

/// Extends a freehand stroke to `p`.
fn stroke_to(editor: &mut Editor, session: &mut Session, p: Point, pressure: f32) {
    let Session::Stroke {
        ink,
        ctx,
        line,
        last,
        ..
    } = session
    else {
        return;
    };
    let (layer, frame) = (editor.layer, editor.frame);
    let Ok(target) = editor.doc.target(layer, frame) else {
        return;
    };
    let mut canvas = Canvas::new(target, ctx);
    match line {
        Line::Steady(stroke) => stroke.add(&mut canvas, p),
        Line::Pressure { size } => {
            let b = brush(pressure_size(*size, pressure));
            paint::line(&mut canvas, *last, p, &b, ink);
        }
    }
    *last = p;
    if !canvas.dirty().is_empty() {
        editor.touch();
    }
}

fn extend(
    editor: &mut Editor,
    preview: &mut Preview,
    session: &mut Session,
    p: Point,
    pressure: f32,
    kind: PointerKind,
    shift: bool,
) {
    match session {
        Session::Stroke { .. } => {
            let pressure = if kind == PointerKind::Pen {
                pressure
            } else {
                1.0
            };
            stroke_to(editor, session, p, pressure);
        }
        Session::Shape { to, .. } => {
            if *to != p {
                *to = p;
                show_shape(editor, preview, session, shift);
            }
        }
        Session::Pick { button } => pick(editor, p, *button),
        Session::Select {
            op,
            before,
            from,
            to,
            ..
        } => {
            if *to != p {
                *to = p;
                editor.selection = selection_of(editor, before.as_ref(), *op, *from, p);
            }
        }
        Session::Move {
            from,
            offset,
            rect,
            original,
            selected,
            ..
        } => {
            let d = Point::new(p.x - from.x, p.y - from.y);
            if d != *offset {
                *offset = d;
                place(editor, original, *rect, d);
                if *selected {
                    let (w, h) = (editor.doc.width(), editor.doc.height());
                    let moved = Rect::new(rect.x + d.x, rect.y + d.y, rect.w, rect.h);
                    editor.selection = Some(Selection::rect(w, h, moved));
                }
            }
        }
    }
}

fn finish(editor: &mut Editor, preview: &mut Preview, session: Session, p: Point, shift: bool) {
    let (layer, frame) = (editor.layer, editor.frame);
    match session {
        Session::Stroke {
            mut tx, line, ctx, ..
        } => {
            if let Line::Steady(stroke) = line
                && let Ok(target) = editor.doc.target(layer, frame)
            {
                let mut canvas = Canvas::new(target, &ctx);
                stroke.finish(&mut canvas);
                if !canvas.dirty().is_empty() {
                    editor.touch();
                }
            }
            if let Some(tx) = tx.take() {
                editor.history.commit(&editor.doc, tx);
                editor.touch();
            }
        }
        Session::Shape {
            ink,
            tool,
            from,
            to,
            ..
        } => {
            preview.clear();
            let to = constrain(tool, from, if to == p { to } else { p }, shift);
            let filled = editor.options.filled;
            let b = brush(editor.options.size);
            let ctx = context(editor);
            let done = edit_cel(
                &mut editor.doc,
                &mut editor.history,
                tool.label(),
                layer,
                frame,
                &ctx,
                |canvas| match tool {
                    Tool::Line => paint::line(canvas, from, to, &b, &ink),
                    Tool::Rectangle => paint::rectangle(canvas, from, to, filled, &b, &ink),
                    Tool::Ellipse => paint::ellipse(canvas, from, to, filled, &b, &ink),
                    _ => {}
                },
            );
            match done {
                Ok(((), dirty)) if !dirty.is_empty() => editor.touch(),
                Ok(_) => {}
                Err(e) => refuse(editor, e),
            }
        }
        Session::Pick { .. } => {}
        Session::Select {
            op,
            before,
            from,
            to,
            ..
        } => {
            // A click that did not drag clears the selection.
            editor.selection = if from == to && op == SelectOp::Replace {
                None
            } else {
                selection_of(editor, before.as_ref(), op, from, to)
            };
        }
        Session::Move { mut tx, .. } => {
            if let Some(tx) = tx.take() {
                editor.history.commit(&editor.doc, tx);
                editor.touch();
            }
        }
    }
}

/// The selection a drag from `a` to `b` makes, combined with what was
/// selected before.
fn selection_of(
    editor: &Editor,
    before: Option<&Selection>,
    op: SelectOp,
    a: Point,
    b: Point,
) -> Option<Selection> {
    let (w, h) = (editor.doc.width(), editor.doc.height());
    let rect = Rect::from_corners(a, b);
    let dragged = Selection::rect(w, h, rect);
    let mut out = match (op, before) {
        (SelectOp::Replace, _) | (_, None) => dragged,
        (_, Some(prev)) => {
            let mut s = prev.clone();
            s.combine(&dragged, op);
            s
        }
    };
    if out.is_empty() {
        return None;
    }
    if op == SelectOp::Replace && before.is_none() {
        out = Selection::rect(w, h, rect);
    }
    Some(out)
}

/// Puts the lifted pixels of `rect` back at `offset` from where they were,
/// starting again from the cel as it was before the move.
fn place(editor: &mut Editor, original: &Raster, rect: Rect, offset: Point) {
    let (layer, frame) = (editor.layer, editor.frame);
    let Ok(target) = editor.doc.target(layer, frame) else {
        return;
    };
    let clear = match original {
        Raster::Indexed(_) => u32::from(target.transparent_index),
        _ => 0,
    };
    let raster = target.raster;
    *raster = original.clone();
    let lifted = Pixels::from_raster(original, rect);
    raster.fill_rect_raw(&rect, clear);
    lifted.write_to(
        raster,
        Point::new(rect.x + offset.x, rect.y + offset.y),
        Some(clear),
    );
    editor.touch();
}

fn pick(editor: &mut Editor, p: Point, button: PointerButton) {
    if let Some(c) = color_at(editor, p)
        && c.a > 0
    {
        if button == PointerButton::Secondary {
            editor.bg = c;
        } else {
            editor.fg = c;
        }
    }
}

fn show_shape(editor: &Editor, preview: &mut Preview, session: &Session, shift: bool) {
    if let Session::Shape {
        tool,
        from,
        to,
        button,
        ..
    } = session
    {
        let to = constrain(*tool, *from, *to, shift);
        let pixels = shape_pixels(*tool, *from, to, editor.options.filled, editor.options.size);
        preview.set(pixels, paint_color(editor, *button));
    }
}

/// Tells the status bar where the pointer is and what is under it.
pub fn track_pointer(
    editor: Res<Editor>,
    view: Res<View>,
    nav: Res<Navigation>,
    mut signals: Signals,
) {
    let Some(at) = nav.hover else {
        return;
    };
    let (pos, color) = if view.contains(at) {
        let p = view.doc_pixel(at);
        if editor.doc.bounds().contains(p.x, p.y) {
            let c = color_at(&editor, p).map_or_else(String::new, hex);
            (format!("{}, {}", p.x, p.y), c)
        } else {
            (String::new(), String::new())
        }
    } else {
        (String::new(), String::new())
    };
    set_if_changed(&mut signals, "status_pos", pos);
    set_if_changed(&mut signals, "status_color", color);
    let size = match &editor.selection {
        Some(s) => {
            let b = s.bounds();
            format!(
                "{} x {}  selection {} x {}",
                editor.doc.width(),
                editor.doc.height(),
                b.w,
                b.h
            )
        }
        None => format!("{} x {}", editor.doc.width(), editor.doc.height()),
    };
    set_if_changed(&mut signals, "status_size", size);
}

/// `#rrggbb`, with the alpha when it is not opaque.
pub fn hex(c: Rgba) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}

pub fn set_if_changed(signals: &mut Signals, name: &str, value: String) {
    if signals.get::<String>(name).as_deref() != Some(value.as_str()) {
        signals.set(name, value);
    }
}

/// The stage's cursor: a crosshair to paint, a hand while space pans.
pub fn stage_cursor(
    nav: Res<Navigation>,
    mut shown: Local<Option<bool>>,
    wake: Option<Res<WakeDeadline>>,
) {
    let panning = nav.space_held || nav.panning.is_some();
    if *shown == Some(panning) {
        return;
    }
    crate::repaint_soon(&wake);
    if let Some(stage) = dom::get_by_id("stage") {
        if panning {
            stage.add_class("panning");
        } else {
            stage.remove_class("panning");
        }
        *shown = Some(panning);
    }
}

/// Shows the active tool: its tool bar button lit, its name and options in
/// the tool options bar.
pub fn show_tool(
    editor: Res<Editor>,
    mut shown: Local<Option<Tool>>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    if *shown == Some(editor.tool) {
        return;
    }
    crate::repaint_soon(&wake);
    let tool = editor.tool;
    for t in Tool::ALL {
        if let Some(button) = dom::get_by_id(t.button_id()) {
            if t == tool {
                button.add_class("active");
            } else {
                button.remove_class("active");
            }
        }
    }
    use Tool::*;
    signals.set("tool_label", tool.label());
    signals.set(
        "show_size",
        matches!(tool, Pencil | Eraser | Line | Rectangle | Ellipse),
    );
    signals.set("show_pixel_perfect", tool == Pencil);
    signals.set("show_pressure", matches!(tool, Pencil | Eraser));
    signals.set("show_filled", matches!(tool, Rectangle | Ellipse));
    signals.set("show_bucket", tool == Bucket);
    signals.set(
        "tool_hint",
        match tool {
            Eyedropper => "Click to pick the foreground, right-click the background",
            Select => "Drag to select; Shift adds, Alt subtracts; click to clear",
            Move => "Drag the selection, or the whole layer",
            Line => "Shift snaps to clean pixel steps",
            Rectangle | Ellipse => "Shift keeps it square",
            _ => "",
        },
    );
    *shown = Some(tool);
}

/// Keeps the tool options and their controls in step: a control the user
/// moved writes its option, and an option changed elsewhere moves its
/// control.
pub fn tool_options(
    mut editor: ResMut<Editor>,
    mut synced: Local<Option<crate::editor::ToolOptions>>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let mut o = editor.options;
    if let Some(last) = *synced {
        let num = |s: &Signals, name: &str| s.get::<f64>(name);
        if let Some(v) = num(&signals, "opt_size") {
            let v = v.round().clamp(1.0, 32.0) as u32;
            if v != last.size {
                o.size = v;
            }
        }
        if let Some(v) = num(&signals, "opt_tolerance") {
            let v = v.round().clamp(0.0, 255.0) as u8;
            if v != last.tolerance {
                o.tolerance = v;
            }
        }
        let flag = |s: &Signals, name: &str, was: bool| s.get::<bool>(name).unwrap_or(was);
        if flag(&signals, "opt_pixel_perfect", last.pixel_perfect) != last.pixel_perfect {
            o.pixel_perfect = !last.pixel_perfect;
        }
        if flag(&signals, "opt_pressure", last.pressure_size) != last.pressure_size {
            o.pressure_size = !last.pressure_size;
        }
        if flag(&signals, "opt_filled", last.filled) != last.filled {
            o.filled = !last.filled;
        }
        if flag(&signals, "opt_contiguous", last.contiguous) != last.contiguous {
            o.contiguous = !last.contiguous;
        }
    }
    if o != editor.options {
        editor.options = o;
    }
    if *synced != Some(o) {
        signals.set("opt_size", f64::from(o.size));
        signals.set("opt_size_text", format!("{} px", o.size));
        signals.set("opt_tolerance", f64::from(o.tolerance));
        signals.set("opt_tolerance_text", o.tolerance.to_string());
        signals.set("opt_pixel_perfect", o.pixel_perfect);
        signals.set("opt_pressure", o.pressure_size);
        signals.set("opt_filled", o.filled);
        signals.set("opt_contiguous", o.contiguous);
        *synced = Some(o);
        crate::repaint_soon(&wake);
    }
}

/// The window position of a document pixel's middle, for tests that drive
/// the tools with the pointer.
pub fn screen_of(view: &View, p: Point) -> Vec2 {
    Vec2::new(
        view.stage.0.x + (view.origin.0 + (f64::from(p.x) + 0.5) * view.zoom) as f32,
        view.stage.0.y + (view.origin.1 + (f64::from(p.y) + 0.5) * view.zoom) as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_pick_tools() {
        assert_eq!(tool_for_key("b", false), Some(Tool::Pencil));
        assert_eq!(tool_for_key("u", false), Some(Tool::Rectangle));
        assert_eq!(tool_for_key("u", true), Some(Tool::Ellipse));
        assert_eq!(tool_for_key("q", false), None);
    }

    #[test]
    fn pressure_scales_the_brush_but_never_below_one_pixel() {
        assert_eq!(pressure_size(8, 1.0), 8);
        assert_eq!(pressure_size(8, 0.5), 4);
        assert_eq!(pressure_size(8, 0.0), 1);
    }

    #[test]
    fn shift_makes_a_square() {
        let to = constrain(Tool::Rectangle, Point::new(0, 0), Point::new(5, -2), true);
        assert_eq!(to, Point::new(5, -5));
    }

    #[test]
    fn hex_colors() {
        assert_eq!(hex(Rgba::opaque(1, 2, 255)), "#0102ff");
        assert_eq!(hex(Rgba::new(1, 2, 3, 4)), "#01020304");
    }
}
