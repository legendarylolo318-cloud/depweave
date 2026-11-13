//! Rasterisation of a [`Scene`] into a tiny-skia pixmap.

use crate::lines;
use crate::scene::{screen_radius, Scene};
use crate::text::TextEngine;
use crate::view::View;
use std::collections::HashMap;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, PixmapMut, Stroke, Transform};

#[derive(Debug, Clone, Default)]
pub struct Highlight {
    pub selected: Option<usize>,
    pub hovered: Option<usize>,
    /// Search matches (scene indices); always labelled and ringed.
    pub matches: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Style {
    pub dark: bool,
    pub show_optional: bool,
    /// Labels appear when `zoom >= label_zoom` (hovered/selected/matches always).
    pub label_zoom: f32,
    /// Device pixel ratio; scales strokes and fonts.
    pub scale: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            dark: true,
            show_optional: true,
            label_zoom: 1.2,
            scale: 1.0,
        }
    }
}

impl Style {
    pub fn background(&self) -> [u8; 3] {
        if self.dark {
            [22, 24, 29]
        } else {
            [248, 248, 250]
        }
    }
    pub fn edge_color(&self) -> [u8; 3] {
        if self.dark {
            [150, 160, 180]
        } else {
            [90, 98, 115]
        }
    }
    pub fn label_color(&self) -> [u8; 3] {
        if self.dark {
            [232, 234, 240]
        } else {
            [24, 26, 32]
        }
    }
    pub fn accent(&self) -> [u8; 3] {
        if self.dark {
            [255, 255, 255]
        } else {
            [10, 10, 10]
        }
    }
}

pub const LABEL_PX: f32 = 12.0;
const MAX_LABELS: usize = 500;
const ARROW_EDGE_LIMIT: usize = 400;

/// Decide which labels to draw: `(node index, x, y)` with `(x, y)` the
/// label's left-middle anchor in screen pixels. Greedy, collision-avoiding.
pub fn plan_labels(
    scene: &Scene,
    view: &View,
    hl: &Highlight,
    style: &Style,
) -> Vec<(usize, f32, f32)> {
    let px = LABEL_PX * style.scale;
    let mut order: Vec<usize> = Vec::new();
    let mut forced = Vec::new();
    for i in hl
        .selected
        .iter()
        .chain(hl.hovered.iter())
        .chain(hl.matches.iter())
    {
        if *i < scene.nodes.len() && !forced.contains(i) {
            forced.push(*i);
        }
    }
    order.extend(forced.iter().copied());
    if view.zoom >= style.label_zoom {
        let (x0, y0, x1, y1) = view.world_rect(0.0);
        let mut vis: Vec<usize> = scene
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| {
                n.pos[0] >= x0
                    && n.pos[0] <= x1
                    && n.pos[1] >= y0
                    && n.pos[1] <= y1
                    && !forced.contains(i)
            })
            .map(|(i, _)| i)
            .collect();
        vis.sort_by(|&a, &b| {
            scene.nodes[b]
                .radius
                .total_cmp(&scene.nodes[a].radius)
                .then(a.cmp(&b))
        });
        order.extend(vis);
    }
    let cell = 80.0f32;
    let mut grid: HashMap<(i32, i32), Vec<[f32; 4]>> = HashMap::new();
    let mut out = Vec::new();
    for i in order {
        if out.len() >= MAX_LABELS && !forced.contains(&i) {
            break;
        }
        let n = &scene.nodes[i];
        let s = view.to_screen(n.pos);
        let r = screen_radius(n.radius, view.zoom);
        let w = TextEngine::estimate_width(&n.label, px);
        let rect = [
            s[0] + r + 3.0,
            s[1] - px * 0.7,
            s[0] + r + 3.0 + w,
            s[1] + px * 0.5,
        ];
        let is_forced = forced.contains(&i);
        let (cx0, cx1) = (
            (rect[0] / cell).floor() as i32,
            (rect[2] / cell).floor() as i32,
        );
        let (cy0, cy1) = (
            (rect[1] / cell).floor() as i32,
            (rect[3] / cell).floor() as i32,
        );
        if !is_forced {
            if rect[2] < 0.0 || rect[0] > view.width || rect[3] < 0.0 || rect[1] > view.height {
                continue;
            }
            let mut clash = false;
            'c: for gx in cx0..=cx1 {
                for gy in cy0..=cy1 {
                    if let Some(v) = grid.get(&(gx, gy)) {
                        if v.iter().any(|o| {
                            rect[0] < o[2] && rect[2] > o[0] && rect[1] < o[3] && rect[3] > o[1]
                        }) {
                            clash = true;
                            break 'c;
                        }
                    }
                }
            }
            if clash {
                continue;
            }
        }
        for gx in cx0..=cx1 {
            for gy in cy0..=cy1 {
                grid.entry((gx, gy)).or_default().push(rect);
            }
        }
        out.push((i, rect[0], s[1]));
    }
    out
}

fn paint(rgb: [u8; 3], a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(rgb[0], rgb[1], rgb[2], a);
    p.anti_alias = true;
    p
}

pub struct Renderer {
    pub text: TextEngine,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Renderer {
            text: TextEngine::new(),
        }
    }

    pub fn render(
        &mut self,
        pm: &mut PixmapMut<'_>,
        scene: &Scene,
        view: &View,
        hl: &Highlight,
        style: &Style,
    ) {
        let bg = style.background();
        pm.fill(Color::from_rgba8(bg[0], bg[1], bg[2], 255));
        let (w, h) = (pm.width(), pm.height());
        let sc = style.scale;
        let (x0, y0, x1, y1) = view.world_rect(40.0);
        let in_rect = |p: [f32; 2]| p[0] >= x0 && p[0] <= x1 && p[1] >= y0 && p[1] <= y1;
        let pos = |i: u32| scene.nodes[i as usize].pos;

        // ---- edges, batched ----
        let mut hard: Vec<[f32; 4]> = Vec::new();
        let mut opt: Vec<[f32; 4]> = Vec::new();
        let mut hot: Vec<[f32; 4]> = Vec::new();
        let mut arrows = PathBuilder::new();
        let focus: Vec<usize> = hl
            .selected
            .iter()
            .chain(hl.hovered.iter())
            .copied()
            .collect();
        let visible_edges = scene
            .edges
            .iter()
            .filter(|e| {
                let (a, b) = (pos(e.from), pos(e.to));
                (in_rect(a) || in_rect(b) || {
                    let (mnx, mxx) = (a[0].min(b[0]), a[0].max(b[0]));
                    let (mny, mxy) = (a[1].min(b[1]), a[1].max(b[1]));
                    mnx <= x1 && mxx >= x0 && mny <= y1 && mxy >= y0
                }) && (style.show_optional || !e.optional)
            })
            .count();
        let draw_arrows = visible_edges <= ARROW_EDGE_LIMIT;
        for e in &scene.edges {
            if e.optional && !style.show_optional {
                continue;
            }
            let (a, b) = (pos(e.from), pos(e.to));
            let (mnx, mxx) = (a[0].min(b[0]), a[0].max(b[0]));
            let (mny, mxy) = (a[1].min(b[1]), a[1].max(b[1]));
            if mnx > x1 || mxx < x0 || mny > y1 || mxy < y0 {
                continue;
            }
            let (sa, sb) = (view.to_screen(a), view.to_screen(b));
            let is_hot = focus.contains(&(e.from as usize)) || focus.contains(&(e.to as usize));
            let bucket = if is_hot {
                &mut hot
            } else if e.optional {
                &mut opt
            } else {
                &mut hard
            };
            bucket.push([sa[0], sa[1], sb[0], sb[1]]);
            if is_hot || draw_arrows {
                let (dx, dy) = (sb[0] - sa[0], sb[1] - sa[1]);
                let len = (dx * dx + dy * dy).sqrt();
                if len > 1.0 {
                    let r = screen_radius(scene.nodes[e.to as usize].radius, view.zoom) + 1.0;
                    let (ux, uy) = (dx / len, dy / len);
                    let (tx, ty) = (sb[0] - ux * r, sb[1] - uy * r);
                    let sz = 6.0 * sc;
                    if len > r + sz {
                        arrows.move_to(tx, ty);
                        arrows
                            .line_to(tx - ux * sz - uy * sz * 0.45, ty - uy * sz + ux * sz * 0.45);
                        arrows
                            .line_to(tx - ux * sz + uy * sz * 0.45, ty - uy * sz - ux * sz * 0.45);
                        arrows.close();
                    }
                }
            }
        }
        let ec = style.edge_color();
        let alpha = if visible_edges > 4000 {
            38
        } else if visible_edges > 1000 {
            60
        } else {
            110
        };
        {
            let data = pm.data_mut();
            let thick = sc >= 1.5;
            let a = alpha as f32 / 255.0;
            for seg in &hard {
                lines::line(data, w, h, *seg, ec, a, None, thick);
            }
            for seg in &opt {
                lines::line(
                    data,
                    w,
                    h,
                    *seg,
                    ec,
                    (a * 1.6).min(1.0),
                    Some((5.0 * sc, 4.0 * sc)),
                    thick,
                );
            }
            for seg in &hot {
                lines::line(data, w, h, *seg, style.accent(), 0.85, None, true);
            }
        }
        if let Some(p) = arrows.finish() {
            pm.fill_path(
                &p,
                &paint(ec, 190),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }

        // ---- nodes, batched by colour ----
        let mut batches: HashMap<[u8; 3], PathBuilder> = HashMap::new();
        for n in &scene.nodes {
            if !in_rect(n.pos) {
                continue;
            }
            let s = view.to_screen(n.pos);
            let r = screen_radius(n.radius, view.zoom) * sc;
            batches
                .entry(n.color)
                .or_default()
                .push_circle(s[0], s[1], r);
        }
        let mut keys: Vec<_> = batches.keys().copied().collect();
        keys.sort_unstable();
        for k in keys {
            if let Some(pb) = batches.remove(&k) {
                if let Some(p) = pb.finish() {
                    pm.fill_path(
                        &p,
                        &paint(k, 235),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
        // rings for matches / hovered / selected
        let ring = |pm: &mut PixmapMut<'_>, i: usize, rgb: [u8; 3], width: f32, extra: f32| {
            let n = &scene.nodes[i];
            let s = view.to_screen(n.pos);
            let r = screen_radius(n.radius, view.zoom) * sc + extra * sc;
            let mut pb = PathBuilder::new();
            pb.push_circle(s[0], s[1], r);
            if let Some(p) = pb.finish() {
                let st = Stroke {
                    width: width * sc,
                    ..Stroke::default()
                };
                pm.stroke_path(&p, &paint(rgb, 255), &st, Transform::identity(), None);
            }
        };
        for &m in &hl.matches {
            if m < scene.nodes.len() {
                ring(pm, m, [255, 214, 10], 2.0, 3.0);
            }
        }
        if let Some(i) = hl.hovered.filter(|&i| i < scene.nodes.len()) {
            ring(pm, i, style.accent(), 2.0, 2.0);
        }
        if let Some(i) = hl.selected.filter(|&i| i < scene.nodes.len()) {
            ring(pm, i, style.accent(), 2.5, 3.5);
        }

        // ---- labels ----
        if self.text.available() {
            let px = LABEL_PX * sc;
            let lc = style.label_color();
            let bgc = style.background();
            for (i, x, y) in plan_labels(scene, view, hl, style) {
                let label = &scene.nodes[i].label;
                let tw = self.text.width(label, px);
                let mut pb = PathBuilder::new();
                if let Some(r) =
                    tiny_skia::Rect::from_xywh(x - 2.0, y - px * 0.8, tw + 4.0, px * 1.3)
                {
                    pb.push_rect(r);
                }
                if let Some(p) = pb.finish() {
                    pm.fill_path(
                        &p,
                        &paint(bgc, 170),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                self.text
                    .draw(pm.data_mut(), w, h, label, x, y + px * 0.35, px, lc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Edge, Node};

    fn scene(n: usize) -> Scene {
        let nodes = (0..n)
            .map(|i| Node {
                id: i,
                pos: [(i % 40) as f32 * 30.0, (i / 40) as f32 * 30.0],
                radius: 4.0,
                color: [200, 100, 50],
                label: format!("pkg-{i}").into(),
            })
            .collect();
        let edges = (1..n)
            .map(|i| Edge {
                from: i as u32,
                to: (i / 2) as u32,
                optional: i % 7 == 0,
            })
            .collect();
        Scene::new(nodes, edges)
    }

    #[test]
    fn renders_something_and_is_deterministic() {
        let s = scene(500);
        let v = s.fit_view(320.0, 240.0);
        let mut r = Renderer::new();
        let mut a = tiny_skia::Pixmap::new(320, 240).unwrap();
        r.render(
            &mut a.as_mut(),
            &s,
            &v,
            &Highlight::default(),
            &Style::default(),
        );
        let bg = Style::default().background();
        let non_bg = a
            .data()
            .chunks(4)
            .filter(|p| p[0] != bg[0] || p[1] != bg[1] || p[2] != bg[2])
            .count();
        assert!(non_bg > 500, "expected drawn pixels, got {non_bg}");
        let mut b = tiny_skia::Pixmap::new(320, 240).unwrap();
        r.render(
            &mut b.as_mut(),
            &s,
            &v,
            &Highlight::default(),
            &Style::default(),
        );
        assert_eq!(a.data(), b.data());
    }

    #[test]
    fn labels_respect_zoom_and_force_hover() {
        let s = scene(100);
        let mut v = s.fit_view(800.0, 600.0);
        v.zoom = 0.3;
        let st = Style::default();
        assert!(plan_labels(&s, &v, &Highlight::default(), &st).is_empty());
        let hl = Highlight {
            hovered: Some(3),
            ..Default::default()
        };
        assert_eq!(plan_labels(&s, &v, &hl, &st).len(), 1);
        v.zoom = 2.0;
        assert!(plan_labels(&s, &v, &Highlight::default(), &st).len() > 3);
    }

    #[test]
    fn edge_cases_do_not_panic() {
        let mut r = Renderer::new();
        let mut pm = tiny_skia::Pixmap::new(10, 10).unwrap();
        let empty = Scene::new(vec![], vec![]);
        r.render(
            &mut pm.as_mut(),
            &empty,
            &View::default(),
            &Highlight::default(),
            &Style::default(),
        );
        let s = scene(3);
        let hl = Highlight {
            selected: Some(99),
            hovered: Some(99),
            matches: vec![42],
        };
        r.render(
            &mut pm.as_mut(),
            &s,
            &View::default(),
            &hl,
            &Style::default(),
        );
    }
}
