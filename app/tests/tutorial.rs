//! The rig tutorial over a rendered editor: Help > Rig tutorial opens the
//! lesson and its card, and each step ticks off when the driven input does
//! what it asks, not before.

pub mod common;

use std::sync::{Arc, Mutex};

use common::demo::{ARM_X, HAND_Y, LESSON_FRAMES, SHOULDER_Y};
use common::drive::{At, Outcome, Step, drive, editor_over};
use lumenui::lumen_core::input::{NamedKey, PointerButton};
use lumenui::prelude::*;
use pixl::Workspace;
use pixl::tutorial::STEPS;
use pixl_core::{ColorMode, Document, LayerKind};

const L: PointerButton = PointerButton::Primary;

fn drag(a: (i32, i32), b: (i32, i32)) -> Vec<Step> {
    let mut steps = vec![Step::Move(At::Doc(a.0, a.1)), Step::Wait];
    steps.push(Step::Down(At::Doc(a.0, a.1), L));
    for i in 1..=4 {
        let x = a.0 + (b.0 - a.0) * i / 4;
        let y = a.1 + (b.1 - a.1) * i / 4;
        steps.push(Step::Move(At::Doc(x, y)));
    }
    steps.push(Step::Up(At::Doc(b.0, b.1), L));
    steps.push(Step::Wait);
    steps
}

/// A script, with the place in it where each tutorial step should tick off.
#[derive(Default)]
struct Script {
    steps: Vec<Step>,
    /// The tutorial step, and how many scripted steps have run once the
    /// input that does it has.
    expect: Vec<(usize, usize)>,
}

impl Script {
    /// Input that does not finish a step.
    fn then(&mut self, steps: impl IntoIterator<Item = Step>) -> &mut Self {
        self.steps.extend(steps);
        self
    }

    /// Input whose last step finishes tutorial step `k`.
    fn ticks(&mut self, k: usize, steps: Vec<Step>) -> &mut Self {
        let last = steps.iter().rposition(|s| !matches!(s, Step::Wait));
        let done_at = self.steps.len() + last.expect("an input") + 1;
        self.steps.extend(steps);
        self.expect.push((k, done_at));
        self.steps.push(Step::Wait);
        self
    }

    /// Tutorial step `k` ticks off with the same input as the step before.
    fn also(&mut self, k: usize) -> &mut Self {
        let (_, done_at) = *self.expect.last().expect("an earlier step");
        self.expect.push((k, done_at));
        self
    }
}

/// The scripted steps that had run on the first tick the tutorial was past
/// step `k`.
fn passed(out: &Outcome, k: usize) -> Option<usize> {
    out.tutorial
        .iter()
        .find(|(_, step, _)| *step > k)
        .map(|(ran, _, _)| *ran)
}

fn skeleton_rig(doc: &Document) -> &pixl_core::rig::Rig {
    let skel = doc
        .walk()
        .into_iter()
        .find(|(_, l)| matches!(l.kind, LayerKind::Skeleton { .. }))
        .map(|(_, l)| l.id)
        .expect("a skeleton layer");
    doc.rig(skel).unwrap()
}

#[test]
fn each_step_ticks_off_when_the_editor_shows_it_done() {
    let (x, shoulder, hand) = (ARM_X, SHOULDER_Y, HAND_Y);
    let mut s = Script::default();
    s.then([Step::Menu("help-rig-tutorial"), Step::Wait, Step::Wait])
        // New skeleton starts Bones mode, so both steps tick together.
        .ticks(0, vec![Step::Click("rig-new")])
        .also(1)
        .ticks(2, drag((x, shoulder), (x, hand)))
        // Closing the card keeps the lesson; Help shows it again.
        .then([Step::Click("tutor-close"), Step::Wait])
        .then([Step::Menu("help-rig-tutorial"), Step::Wait])
        .ticks(3, vec![Step::Click("rig-bind-layer")])
        .ticks(4, vec![Step::Click("rig-mode-pose")])
        .ticks(5, vec![Step::Click("tl-next")])
        // Turn the arm forward from its hand.
        .ticks(6, drag((x, hand), (x + 6, shoulder + 4)))
        .then([Step::Click("tl-last"), Step::Wait])
        // The last frame holds the first key's pose: the bone points
        // forward and down from the shoulder. Swing it back by its middle.
        .ticks(7, drag((x + 3, shoulder + 2), (x - 5, shoulder + 4)))
        .ticks(8, vec![Step::Click("rig-render-keyed")])
        .ticks(9, vec![Step::Click("tl-play")]);
    let lesson = Document::new(16, 16, ColorMode::Rgba).unwrap();
    let out = drive(editor_over(lesson), s.steps);

    assert_eq!(out.title, "rig lesson *");
    assert_eq!(out.doc.frame_count(), LESSON_FRAMES);
    for (k, done_at) in &s.expect {
        let ran = passed(&out, *k).unwrap_or_else(|| panic!("step {k} never ticked off"));
        assert!(
            ran >= *done_at && ran <= done_at + 2,
            "step {k} ({}) ticked off after {ran} scripted steps, its input ran at {done_at}",
            STEPS[*k].title
        );
    }
    let (_, step, open) = *out.tutorial.last().unwrap();
    assert_eq!(step, STEPS.len(), "every step is done");
    assert!(open, "the card shows the end");
    // The card hid while closed and showed again from the Help menu.
    let closed = out.tutorial.iter().position(|t| !t.2 && t.1 == 3);
    let reopened = out.tutorial.iter().rposition(|t| t.2 && t.1 == 3);
    assert!(closed.is_some() && reopened > closed, "{:?}", out.tutorial);

    let rig = skeleton_rig(&out.doc);
    assert_eq!(rig.bones().len(), 1);
    let keyed: Vec<usize> = (0..LESSON_FRAMES)
        .filter(|&f| rig.frames()[f].key.is_some())
        .collect();
    assert_eq!(keyed, vec![1, LESSON_FRAMES - 1]);
    assert!((1..LESSON_FRAMES).all(|f| rig.frames()[f].generated));
    common::save(&out.frame, "rig-tutorial");
}

#[test]
fn steps_wait_for_their_own_input() {
    let (x, shoulder, hand) = (ARM_X, SHOULDER_Y, HAND_Y);
    let mut steps = vec![
        Step::Menu("help-rig-tutorial"),
        Step::Wait,
        Step::Wait,
        Step::Click("rig-new"),
        Step::Wait,
        // Leaving Bones mode and coming back unticks nothing.
        Step::KeyDown(NamedKey::Escape),
        Step::KeyUp(NamedKey::Escape),
        Step::Wait,
        Step::Click("rig-mode-bones"),
        Step::Wait,
    ];
    steps.extend(drag((x, shoulder), (x, hand)));
    // Posing before binding: Pose mode is the step after next.
    steps.extend([Step::Click("rig-mode-pose"), Step::Wait, Step::Wait]);
    let out = drive(
        editor_over(Document::new(16, 16, ColorMode::Rgba).unwrap()),
        steps,
    );
    assert_eq!(out.tutorial.last().unwrap().1, 3, "binding the arm is next");
}

#[test]
fn opening_another_document_ends_the_lesson() {
    let out = drive(
        editor_over(Document::new(16, 16, ColorMode::Rgba).unwrap()),
        vec![
            Step::Menu("help-rig-tutorial"),
            Step::Wait,
            Step::Wait,
            Step::Menu("help-rig-demo"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert_eq!(out.title, "rig demo");
    assert!(out.tutorial.iter().any(|t| t.2), "the card showed");
    assert!(!out.tutorial.last().unwrap().2, "and went with the lesson");
}

#[test]
fn the_lesson_works_in_the_rig_layout() {
    let layout: Arc<Mutex<String>> = Arc::default();
    let seen = Arc::clone(&layout);
    let app = editor_over(Document::new(16, 16, ColorMode::Rgba).unwrap())
        .add_systems(TickStage::Systems, move |ws: Res<Workspace>| {
            *seen.lock().unwrap() = ws.live.name.clone()
        });
    drive(
        app,
        vec![Step::Menu("help-rig-tutorial"), Step::Wait, Step::Wait],
    );
    assert_eq!(*layout.lock().unwrap(), "Rig");
}
