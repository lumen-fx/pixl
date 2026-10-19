//! Timings for posing and rendering a rig: a 256x256 character of 20 bones,
//! each with its own layer. Run with `cargo bench -p pixl-document --bench rig`;
//! it prints wall-clock times.

use std::time::{Duration, Instant};

use pixl_document::{
    Binding, BonePose, ColorMode, Document, Easing, LayerId, NewLayer, Parts, Placement, Pose,
    PoseKey, RenderOptions, Rgba, Vec2,
};

fn time<R>(label: &str, f: impl FnOnce() -> R) -> (R, Duration) {
    let start = Instant::now();
    let out = f();
    let d = start.elapsed();
    println!("{label:<52} {:>10.3} ms", d.as_secs_f64() * 1e3);
    (out, d)
}

/// Head, tail and parent (an index into the list) of each bone: a spine,
/// a head, two arms, two legs and a tail.
fn skeleton() -> Vec<(Vec2, Vec2, Option<usize>)> {
    let v = Vec2::new;
    let mut bones = vec![
        (v(128.0, 150.0), v(128.0, 130.0), None),
        (v(128.0, 130.0), v(128.0, 110.0), Some(0)),
        (v(128.0, 110.0), v(128.0, 92.0), Some(1)),
        (v(128.0, 92.0), v(128.0, 80.0), Some(2)),
        (v(128.0, 80.0), v(128.0, 52.0), Some(3)),
    ];
    for side in [-1.0, 1.0] {
        let mut parent = 2;
        let mut at = v(128.0, 100.0);
        for _ in 0..3 {
            let next = at + v(side * 22.0, 10.0);
            bones.push((at, next, Some(parent)));
            parent = bones.len() - 1;
            at = next;
        }
        let mut parent = 0;
        let mut at = v(128.0 + side * 10.0, 150.0);
        for _ in 0..3 {
            let next = at + v(side * 4.0, 26.0);
            bones.push((at, next, Some(parent)));
            parent = bones.len() - 1;
            at = next;
        }
    }
    let mut parent = 0;
    let mut at = v(128.0, 150.0);
    for _ in 0..3 {
        let next = at + v(-14.0, 14.0);
        bones.push((at, next, Some(parent)));
        parent = bones.len() - 1;
        at = next;
    }
    bones
}

/// A thick outlined limb along a bone.
fn paint_limb(doc: &mut Document, layer: LayerId, head: Vec2, tail: Vec2) {
    let t = doc.target(layer, 0).unwrap();
    let d = tail - head;
    let len = d.length();
    let n = Vec2::new(-d.y / len, d.x / len);
    let fill = Rgba::opaque(200, 120, 80).to_u32();
    let line = Rgba::opaque(30, 20, 20).to_u32();
    for i in 0..=(len as i32 * 2) {
        let c = head + d * (i as f64 / (len * 2.0));
        for k in -4i32..=4 {
            let p = c + n * k as f64;
            let v = if k.abs() == 4 { line } else { fill };
            t.raster.set_raw(p.x as i32, p.y as i32, v);
        }
    }
}

fn main() {
    let mut doc = Document::new(256, 256, ColorMode::Rgba).unwrap();
    let first = doc.layers()[0].id;
    doc.remove_layer(first).ok();
    let skel = doc
        .add_layer(NewLayer::Skeleton, "Skeleton", Placement::default())
        .unwrap();
    let bones = skeleton();
    let mut ids = Vec::new();
    for (i, (head, tail, parent)) in bones.iter().enumerate() {
        let layer = doc
            .add_layer(NewLayer::Raster, format!("part {i}"), Placement::default())
            .unwrap();
        paint_limb(&mut doc, layer, *head, *tail);
        let rig = doc.rig_mut(skel).unwrap();
        let id = rig
            .add_bone(format!("bone {i}"), parent.map(|p| ids[p]), *head, *tail)
            .unwrap();
        rig.bind(Binding {
            layer,
            region: None,
            bone: id,
            pivot: *head,
        })
        .unwrap();
        ids.push(id);
    }
    println!(
        "{} bones, {} parts on a 256x256 canvas",
        ids.len(),
        ids.len()
    );

    let pose_for = |step: usize| {
        let mut p = Pose::default();
        for (i, id) in ids.iter().enumerate() {
            let a = ((step * 7 + i * 13) % 61) as f64 - 30.0;
            p.set(
                *id,
                BonePose {
                    rotation: a + 0.37,
                    offset: Vec2::ZERO,
                },
            );
        }
        p
    };

    let (parts, _) = time("cut parts out of the source frame", || {
        Parts::new(&doc, skel).unwrap()
    });
    let rig = doc.rig(skel).unwrap().clone();
    time("first posed render (builds RotSprite upscales)", || {
        parts.render(&rig, &pose_for(0))
    });
    let n = 200;
    let mut worst = Duration::ZERO;
    let (_, total) = time(&format!("{n} posed renders, all bones turned"), || {
        for step in 1..=n {
            let t = Instant::now();
            std::hint::black_box(parts.render(&rig, &pose_for(step)));
            worst = worst.max(t.elapsed());
        }
    });
    println!(
        "{:<52} {:>10.3} ms",
        "  per render (live pose preview)",
        total.as_secs_f64() * 1e3 / n as f64
    );
    println!(
        "{:<52} {:>10.3} ms",
        "  slowest render",
        worst.as_secs_f64() * 1e3
    );
    let mut quarter = Pose::default();
    for id in &ids {
        quarter.set(
            *id,
            BonePose {
                rotation: 90.0,
                offset: Vec2::new(1.0, 0.0),
            },
        );
    }
    time("posed render, quarter turns (no upscale)", || {
        parts.render(&rig, &quarter)
    });

    let opts = RenderOptions::default();
    time("render pose to a new frame (cut, pose, write)", || {
        doc.render_pose_to_new_frame(skel, &pose_for(3), 1, opts)
            .unwrap()
    });
    for f in 2..18 {
        doc.insert_frame(f, 100).unwrap();
    }
    for (f, step) in [(1, 0), (9, 20), (17, 40)] {
        doc.rig_mut(skel).unwrap().frame_mut(f).unwrap().key = Some(PoseKey {
            pose: pose_for(step),
            easing: Easing::EaseInOut,
        });
    }
    let ((done, _), _) = time("render keyed poses over 17 frames", || {
        doc.render_keyed(skel, 1, 17, opts).unwrap()
    });
    println!("{:<52} {:>10}", "  frames rendered", done.len());
}
