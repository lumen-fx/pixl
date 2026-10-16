//! Timings for the editor's hot paths on a 4096x4096 document. Run with
//! `cargo bench -p pixl-core`; it prints wall-clock times.

use std::time::{Duration, Instant};

use pixl_core::tools::paint::PencilStroke;
use pixl_core::{
    BlendMode, Brush, ColorMode, CompositeOptions, Document, History, Ink, NewLayer, Placement,
    Point, Rect, Rgba, ToolContext, composite, composite::composite_with_threads, edit_cel,
};

const SIZE: u32 = 4096;

fn time<R>(label: &str, f: impl FnOnce() -> R) -> (R, Duration) {
    let start = Instant::now();
    let out = f();
    let d = start.elapsed();
    println!("{label:<48} {:>10.2} ms", d.as_secs_f64() * 1e3);
    (out, d)
}

/// A deterministic zig-zag of `n + 1` points across the canvas.
fn stroke_points(n: usize) -> Vec<Point> {
    let mut state = 0x2545_f491_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut p = Point::new(SIZE as i32 / 2, SIZE as i32 / 2);
    let mut out = vec![p];
    for _ in 0..n {
        let dx = (next() % 81) as i32 - 40;
        let dy = (next() % 81) as i32 - 40;
        p = Point::new(
            (p.x + dx).clamp(0, SIZE as i32 - 1),
            (p.y + dy).clamp(0, SIZE as i32 - 1),
        );
        out.push(p);
    }
    out
}

fn stroke(
    doc: &mut Document,
    history: &mut History,
    brush: Brush,
    pixel_perfect: bool,
) -> Duration {
    let layer = doc.layers()[0].id;
    let points = stroke_points(1000);
    let ctx = ToolContext::default();
    let ink = Ink::Paint(Rgba::opaque(200, 40, 40).into());
    let mut worst = Duration::ZERO;
    edit_cel(doc, history, "pencil", layer, 0, &ctx, |c| {
        let mut s = PencilStroke::new(c, brush, ink, pixel_perfect);
        for &p in &points {
            let t = Instant::now();
            s.add(c, p);
            worst = worst.max(t.elapsed());
        }
        s.finish(c);
    })
    .unwrap();
    worst
}

fn main() {
    let (mut doc, _) = time("new 4096x4096 document", || {
        Document::new(SIZE, SIZE, ColorMode::Rgba).unwrap()
    });
    let mut history = History::default();

    let (worst, _) = time("1000-segment pencil stroke, 1px pixel-perfect", || {
        stroke(&mut doc, &mut history, Brush::PIXEL, true)
    });
    println!(
        "{:<48} {:>10.3} ms",
        "  slowest segment",
        worst.as_secs_f64() * 1e3
    );
    let (worst, _) = time("1000-segment pencil stroke, 8px circle", || {
        stroke(&mut doc, &mut history, Brush::circle(8), false)
    });
    println!(
        "{:<48} {:>10.3} ms",
        "  slowest segment",
        worst.as_secs_f64() * 1e3
    );
    time("undo the 8px stroke", || history.undo(&mut doc));
    println!(
        "{:<48} {:>10.1} MiB",
        "history held",
        history.memory_bytes() as f64 / 1048576.0
    );

    time("fill 32 full-canvas layers", || {
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
        ];
        let base = doc.layers()[0].id;
        let bounds = doc.bounds();
        doc.target(base, 0)
            .unwrap()
            .raster
            .fill_rect_raw(&bounds, Rgba::opaque(90, 120, 150).to_u32());
        for i in 1..32u8 {
            let l = doc
                .add_layer(NewLayer::Raster, format!("l{i}"), Placement::default())
                .unwrap();
            doc.update_layer(l, |p| p.blend = modes[i as usize % 4])
                .unwrap();
            let c = Rgba::new(
                i.wrapping_mul(37),
                i.wrapping_mul(91),
                i.wrapping_mul(13),
                200,
            );
            doc.target(l, 0)
                .unwrap()
                .raster
                .fill_rect_raw(&bounds, c.to_u32());
        }
    });
    let opts = CompositeOptions::EXPORT;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let label = format!("composite 32 layers, full canvas, {threads} threads");
    time(&label, || composite(&doc, 0, doc.bounds(), &opts));
    time("composite 32 layers, full canvas, 1 thread", || {
        composite_with_threads(&doc, 0, doc.bounds(), &opts, 1)
    });
    time("composite 32 layers, one 64x64 dirty tile", || {
        composite_with_threads(&doc, 0, Rect::new(2048, 2048, 64, 64), &opts, 1)
    });
    time("composite 32 layers, 256x256 dirty rect", || {
        composite_with_threads(&doc, 0, Rect::new(1000, 1000, 256, 256), &opts, 1)
    });
}
