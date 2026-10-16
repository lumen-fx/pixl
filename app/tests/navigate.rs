//! Zoom and pan driven by input messages, the way the window delivers them:
//! the wheel zooms around the pointer, and a drag pans with the middle
//! button or with the space bar held, and holding W, A, S or D moves the
//! view.

pub mod common;

use std::sync::{Arc, Mutex};

use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{
    Key, Modifiers, ModifiersState, MouseWheel, NamedKey, PointerButton, PointerData, PointerMoved,
    PointerPressed, PointerReleased,
};
use lumenui::prelude::*;
use pixl::view::View;

/// One scripted input, written on a tick of its own.
#[derive(Clone, Copy)]
enum Step {
    Wheel(Vec2, f32),
    KeyDown(NamedKey),
    KeyUp(NamedKey),
    Down(Vec2, PointerButton),
    Move(Vec2, u32),
    Up(Vec2, PointerButton),
    /// A character key goes down, with these modifiers.
    CharDown(&'static str, Modifiers),
    CharUp(&'static str),
    /// Holds or lets go of Shift for the steps that follow.
    Shift(bool),
    /// Lets this much wall-clock time pass before the tick goes on.
    Sleep(u64),
    Wait,
}

/// Runs the editor through `steps`, one per tick from the fifth on, and
/// returns the view after each.
fn drive(steps: Vec<Step>) -> Vec<View> {
    let _turn = common::turn();
    let views: Arc<Mutex<Vec<View>>> = Arc::default();
    let out = Arc::clone(&views);
    let n = steps.len() as u64;
    pixl::editor(Source::Disk {
        dir: common::app_dir("cpu"),
    })
    .add_systems(
        TickStage::Systems,
        (move |mut tick: Local<usize>,
               mut shift: Local<bool>,
               mut modifiers: ResMut<ModifiersState>,
               mut wheel: MessageWriter<MouseWheel>,
               mut keys: MessageWriter<KeyPressed>,
               mut keys_up: MessageWriter<KeyReleased>,
               mut down: MessageWriter<PointerPressed>,
               mut moved: MessageWriter<PointerMoved>,
               mut up: MessageWriter<PointerReleased>| {
            *tick += 1;
            modifiers.0.shift = *shift;
            let Some(step) = tick.checked_sub(5).and_then(|i| steps.get(i)) else {
                return;
            };
            match *step {
                Step::CharDown(c, m) => {
                    keys.write(KeyPressed {
                        key: Key::from(c),
                        modifiers: m,
                        repeat: false,
                    });
                }
                Step::CharUp(c) => {
                    keys_up.write(KeyReleased {
                        key: Key::from(c),
                        modifiers: Modifiers::default(),
                    });
                }
                Step::Shift(held) => {
                    *shift = held;
                    modifiers.0.shift = held;
                }
                Step::Sleep(ms) => std::thread::sleep(std::time::Duration::from_millis(ms)),
                Step::Wait => {}
                Step::Wheel(at, dy) => {
                    wheel.write(MouseWheel {
                        delta: Vec2::new(0.0, dy),
                        position: at,
                        local: None,
                    });
                }
                Step::KeyDown(k) => {
                    keys.write(KeyPressed {
                        key: Key::Named(k),
                        modifiers: Modifiers::default(),
                        repeat: false,
                    });
                }
                Step::KeyUp(k) => {
                    keys_up.write(KeyReleased {
                        key: Key::Named(k),
                        modifiers: Modifiers::default(),
                    });
                }
                Step::Down(at, button) => {
                    down.write(PointerPressed {
                        position: at,
                        button,
                        local: None,
                        pointer: PointerData::mouse(button.bit()),
                    });
                }
                Step::Move(at, buttons) => {
                    moved.write(PointerMoved {
                        position: at,
                        local: None,
                        pointer: PointerData::mouse(buttons),
                    });
                }
                Step::Up(at, button) => {
                    up.write(PointerReleased {
                        position: at,
                        button,
                        local: None,
                        pointer: PointerData::mouse(0),
                    });
                }
            }
        })
        .before(pixl::input::navigate),
    )
    .add_systems(
        TickStage::Systems,
        (move |view: Res<View>, mut tick: Local<usize>| {
            *tick += 1;
            if *tick >= 5 {
                out.lock().unwrap().push(view.clone());
            }
        })
        .after(pixl::input::navigate),
    )
    .run_headless_rendered(HeadlessOptions {
        dpr: 1.0,
        ticks: Some(6 + n),
    })
    .expect("the editor runs");
    views.lock().unwrap().clone()
}

/// A point in the middle of the stage, in window coordinates.
const MID: Vec2 = Vec2::new(600.0, 450.0);

#[test]
fn the_wheel_zooms_around_the_pointer() {
    let views = drive(vec![Step::Wheel(MID, -40.0), Step::Wheel(MID, -40.0)]);
    let (before, after) = (&views[0], &views[2]);
    assert!(after.zoom > before.zoom);
    let at = before.stage_point(MID);
    let (a, b) = (before.to_doc(at), after.to_doc(at));
    assert!((a.0 - b.0).abs() <= 1.0 / after.zoom + 1e-9);
    assert!((a.1 - b.1).abs() <= 1.0 / after.zoom + 1e-9);
}

#[test]
fn a_middle_button_drag_pans() {
    let to = MID + Vec2::new(120.0, -40.0);
    let views = drive(vec![
        Step::Down(MID, PointerButton::Middle),
        Step::Move(to, 4),
        Step::Up(to, PointerButton::Middle),
    ]);
    let (before, after) = (&views[0], views.last().unwrap());
    assert_eq!(after.origin.0 - before.origin.0, 120.0);
    assert_eq!(after.origin.1 - before.origin.1, -40.0);
}

#[test]
fn a_drag_with_space_held_pans() {
    let to = MID + Vec2::new(-90.0, 30.0);
    let views = drive(vec![
        Step::KeyDown(NamedKey::Space),
        Step::Down(MID, PointerButton::Primary),
        Step::Move(to, 1),
        Step::Up(to, PointerButton::Primary),
        Step::KeyUp(NamedKey::Space),
    ]);
    let (before, after) = (&views[0], views.last().unwrap());
    assert_eq!(after.origin.0 - before.origin.0, -90.0);
    assert_eq!(after.origin.1 - before.origin.1, 30.0);
}

#[test]
fn a_drag_without_space_does_not_pan() {
    let to = MID + Vec2::new(-90.0, 30.0);
    let views = drive(vec![
        Step::Down(MID, PointerButton::Primary),
        Step::Move(to, 1),
        Step::Up(to, PointerButton::Primary),
    ]);
    assert_eq!(views[0].origin, views.last().unwrap().origin);
}

/// Holds `key` for three ticks 60 ms apart, then lets go, and returns how far
/// the document moved on screen.
fn hold(key: &'static str, modifiers: Modifiers, shift: bool) -> (f64, f64) {
    let views = drive(vec![
        Step::Shift(shift),
        Step::CharDown(key, modifiers),
        Step::Sleep(60),
        Step::Sleep(60),
        Step::Sleep(60),
        Step::CharUp(key),
        Step::Wait,
    ]);
    let (before, after) = (&views[0], views.last().unwrap());
    (
        after.origin.0 - before.origin.0,
        after.origin.1 - before.origin.1,
    )
}

#[test]
fn holding_w_a_s_d_moves_the_view_that_way() {
    // W looks further up, so the document moves down on screen.
    let (x, y) = hold("w", Modifiers::default(), false);
    assert_eq!(x, 0.0);
    assert!(y > 0.0, "W moved the document by {y}");
    let (x, y) = hold("d", Modifiers::default(), false);
    assert!(x < 0.0, "D moved the document by {x}");
    assert_eq!(y, 0.0);
}

#[test]
fn the_pan_follows_the_time_held_and_shift_is_faster() {
    // Three 60 ms steps at the base speed; the first tick only starts the
    // clock. Slow ticks only add time, never more than the cap per tick.
    let cap = pixl::input::KEY_PAN_SPEED * pixl::input::KEY_PAN_MAX_STEP.as_secs_f64();
    let (slow, _) = hold("a", Modifiers::default(), false);
    assert!(slow >= pixl::input::KEY_PAN_SPEED * 0.17, "{slow}");
    assert!(slow <= cap * 4.0 + 1.0, "{slow}");
    let (fast, _) = hold("a", Modifiers::default(), true);
    assert!(fast > slow * 2.0, "{fast} against {slow}");
}

#[test]
fn a_pan_key_with_ctrl_held_keeps_to_its_shortcut() {
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    assert_eq!(hold("d", ctrl, false), (0.0, 0.0));
}
