//! Dev tool: lay out the local pacman graph and render it to a PNG.
//! usage: layout_png OUT.png [repulsion gravity mass_exp] [W H]
use depweave_core::layout::{ForceLayout, ForceParams};
use depweave_core::{load, LoadOptions, Reason};
use depweave_render::{palette, Edge, Highlight, Node, Renderer, Scene, Style};
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = a.get(1).cloned().unwrap_or("layout.png".into());
    let f = |i: usize, d: f32| a.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let params = ForceParams {
        repulsion: f(2, 1.0),
        gravity: f(3, 0.4),
        mass_exp: f(4, 0.5),
    };
    let (w, h) = (f(5, 1400.0) as u32, f(6, 1000.0) as u32);
    let g = match load(&LoadOptions {
        no_cache: true,
        ..Default::default()
    }) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("layout_png: {e}");
            std::process::exit(2);
        }
    };
    let edges: Vec<(usize, usize)> = g.edges(false).into_iter().map(|(x, y, _)| (x, y)).collect();
    let t = Instant::now();
    let mut l = ForceLayout::with_params(g.len(), &edges, &[], params);
    while !l.converged() {
        l.step();
    }
    eprintln!("layout: {} iterations in {:?}", l.iterations, t.elapsed());
    let max = g.packages.iter().map(|p| p.size).max().unwrap_or(1);
    let nodes: Vec<Node> = g
        .packages
        .iter()
        .enumerate()
        .map(|(i, p)| Node {
            id: i,
            pos: l.pos[i],
            radius: 3.0 + 9.0 * palette::size_t(p.size, max),
            color: if g.is_orphan(i) {
                palette::ORPHAN
            } else if p.reason == Reason::Explicit {
                palette::EXPLICIT
            } else {
                palette::DEPENDENCY
            },
            label: p.name.as_str().into(),
        })
        .collect();
    let es = g
        .edges(true)
        .into_iter()
        .map(|(a, b, k)| Edge {
            from: a as u32,
            to: b as u32,
            optional: k == depweave_core::EdgeKind::Optional,
        })
        .collect();
    let scene = Scene::new(nodes, es);
    let view = scene.fit_view(w as f32, h as f32);
    let Some(mut pm) = depweave_render::tiny_skia::Pixmap::new(w.max(1), h.max(1)) else {
        eprintln!("layout_png: bad size {w}x{h}");
        std::process::exit(2);
    };
    let t = Instant::now();
    Renderer::new().render(
        &mut pm.as_mut(),
        &scene,
        &view,
        &Highlight::default(),
        &Style::default(),
    );
    eprintln!("render: {:?} (fit zoom {:.3})", t.elapsed(), view.zoom);
    if let Err(e) = pm.save_png(&out) {
        eprintln!("layout_png: {out}: {e}");
        std::process::exit(2);
    }
    // frame-time benchmark: pan + zoom sweep, full system
    let mut r = Renderer::new();
    let mut v = view;
    let mut times = Vec::new();
    for i in 0..60 {
        v.pan(7.0, 3.0);
        if i % 2 == 0 {
            v.zoom_at([w as f32 / 2.0, h as f32 / 2.0], 1.01);
        }
        let t = Instant::now();
        r.render(
            &mut pm.as_mut(),
            &scene,
            &v,
            &Highlight::default(),
            &Style::default(),
        );
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.total_cmp(b));
    eprintln!(
        "frame ms: median {:.1}, p95 {:.1}, max {:.1}",
        times[30], times[56], times[59]
    );
}
