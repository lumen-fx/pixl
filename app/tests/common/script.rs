//! Scripts that walk a tutorial: driven input, with the place in it where
//! each lesson step should tick off.

use lumenui::lumen_core::input::PointerButton;

use super::drive::{At, Outcome, Step};

const L: PointerButton = PointerButton::Primary;

/// A drag with the primary button from pixel `a` to pixel `b`.
pub fn drag(a: (i32, i32), b: (i32, i32)) -> Vec<Step> {
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

/// A drag through `points` in turn, as a lasso draws.
pub fn trace(points: &[(i32, i32)]) -> Vec<Step> {
    let (first, rest) = points.split_first().expect("a point");
    let mut steps = vec![Step::Move(At::Doc(first.0, first.1)), Step::Wait];
    steps.push(Step::Down(At::Doc(first.0, first.1), L));
    for p in rest {
        steps.push(Step::Move(At::Doc(p.0, p.1)));
    }
    let last = points.last().expect("a point");
    steps.push(Step::Up(At::Doc(last.0, last.1), L));
    steps.push(Step::Wait);
    steps
}

/// A press on the canvas at a pixel, which is what selects a bone.
pub fn press(a: (i32, i32)) -> Vec<Step> {
    vec![
        Step::Move(At::Doc(a.0, a.1)),
        Step::Wait,
        Step::Down(At::Doc(a.0, a.1), L),
    ]
}

/// A click on the canvas at a pixel.
pub fn click(a: (i32, i32)) -> Vec<Step> {
    let mut steps = press(a);
    steps.extend([Step::Up(At::Doc(a.0, a.1), L), Step::Wait]);
    steps
}

/// A script, with the place in it where each tutorial step should tick off.
#[derive(Default)]
pub struct Script {
    pub steps: Vec<Step>,
    /// The tutorial step, and how many scripted steps have run once the
    /// input that does it has.
    pub expect: Vec<(usize, usize)>,
}

impl Script {
    /// Input that does not finish a step.
    pub fn then(&mut self, steps: impl IntoIterator<Item = Step>) -> &mut Self {
        self.steps.extend(steps);
        self
    }

    /// Input whose last step finishes tutorial step `k`, then two ticks
    /// for the next step's outline to show.
    pub fn ticks(&mut self, k: usize, steps: Vec<Step>) -> &mut Self {
        let last = steps.iter().rposition(|s| !matches!(s, Step::Wait));
        let done_at = self.steps.len() + last.expect("an input") + 1;
        self.steps.extend(steps);
        self.expect.push((k, done_at));
        self.steps.extend([Step::Wait, Step::Wait]);
        self
    }

    /// Checks that each step ticked off on the tick its input ran or just
    /// after, not before, and that the lesson, whose steps `titles` names,
    /// ended with every step done and its card showing.
    pub fn check(&self, out: &Outcome, titles: &[&str]) {
        for (k, done_at) in &self.expect {
            let ran = passed(out, *k).unwrap_or_else(|| {
                panic!(
                    "step {k} ({}) never ticked off: {:?}",
                    titles[*k],
                    out.tutorial.last()
                )
            });
            assert!(
                ran >= *done_at && ran <= done_at + 2,
                "step {k} ({}) ticked off after {ran} scripted steps, its input ran at {done_at}",
                titles[*k]
            );
        }
        let last = out.tutorial.last().expect("the tutorial ran");
        assert_eq!(last.step, titles.len(), "every step is done");
        assert!(last.open, "the card shows the end");
        // Every control a step pointed at is one the window has, and it
        // took the outline.
        for t in &out.tutorial {
            let Some(target) = &t.target else { continue };
            assert!(
                out.tutorial
                    .iter()
                    .any(|u| u.outlined && u.target.as_ref() == Some(target)),
                "{target} never took the outline: {:?}",
                out.tutorial
                    .iter()
                    .filter(|u| u.target.as_ref() == Some(target))
                    .collect::<Vec<_>>()
            );
        }
    }
}

/// The scripted steps that had run on the first tick the tutorial was past
/// step `k`.
pub fn passed(out: &Outcome, k: usize) -> Option<usize> {
    out.tutorial.iter().find(|t| t.step > k).map(|t| t.ran)
}
