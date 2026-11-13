//! Scene description and spatial index for hit-testing.

use crate::view::View;
use rstar::{primitives::GeomWithData, RTree};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Node {
    /// Caller-defined identifier (e.g. package index).
    pub id: usize,
    pub pos: [f32; 2],
    /// Radius in world units; on screen it is clamped (see `Style`).
    pub radius: f32,
    pub color: [u8; 3],
    pub label: Arc<str>,
}

#[derive(Debug, Clone, Copy)]
pub struct Edge {
    /// Indices into `Scene::nodes`.
    pub from: u32,
    pub to: u32,
    pub optional: bool,
}

type Pt = GeomWithData<[f32; 2], u32>;

pub struct Scene {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    tree: RTree<Pt>,
}

pub const MIN_NODE_PX: f32 = 2.0;
pub const MAX_NODE_PX: f32 = 28.0;

pub fn screen_radius(world_radius: f32, zoom: f32) -> f32 {
    (world_radius * zoom).clamp(MIN_NODE_PX, MAX_NODE_PX)
}

impl Scene {
    pub fn new(nodes: Vec<Node>, edges: Vec<Edge>) -> Self {
        let tree = RTree::bulk_load(
            nodes
                .iter()
                .enumerate()
                .map(|(i, n)| GeomWithData::new(n.pos, i as u32))
                .collect(),
        );
        Scene { nodes, edges, tree }
    }

    /// Same nodes/edges at new positions (used for progressive layout updates).
    pub fn with_positions(&self, pos: &[[f32; 2]]) -> Scene {
        let mut nodes = self.nodes.clone();
        for (n, p) in nodes.iter_mut().zip(pos) {
            n.pos = *p;
        }
        Scene::new(nodes, self.edges.clone())
    }

    /// Index of the node under screen point `s`, preferring the nearest.
    pub fn hit(&self, view: &View, s: [f32; 2]) -> Option<usize> {
        let w = view.to_world(s);
        let reach_world = (MAX_NODE_PX + 4.0) / view.zoom;
        let r2 = reach_world * reach_world;
        let mut best: Option<(f32, usize)> = None;
        for p in self.tree.locate_within_distance(w, r2) {
            let i = p.data as usize;
            let n = &self.nodes[i];
            let sp = view.to_screen(n.pos);
            let d = ((sp[0] - s[0]).powi(2) + (sp[1] - s[1]).powi(2)).sqrt();
            let r = screen_radius(n.radius, view.zoom).max(4.0) + 2.0;
            if d <= r && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, i));
            }
        }
        best.map(|b| b.1)
    }

    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut it = self.nodes.iter();
        let f = it.next()?;
        let mut b = (f.pos[0], f.pos[1], f.pos[0], f.pos[1]);
        for n in it {
            b.0 = b.0.min(n.pos[0]);
            b.1 = b.1.min(n.pos[1]);
            b.2 = b.2.max(n.pos[0]);
            b.3 = b.3.max(n.pos[1]);
        }
        Some(b)
    }

    /// A view that shows everything with some padding.
    pub fn fit_view(&self, width: f32, height: f32) -> View {
        let Some((x0, y0, x1, y1)) = self.bounds() else {
            return View {
                width,
                height,
                ..View::default()
            };
        };
        let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let zoom = ((width - 60.0).max(50.0) / w)
            .min((height - 60.0).max(50.0) / h)
            .clamp(crate::view::MIN_ZOOM, 4.0);
        View {
            cx: (x0 + x1) / 2.0,
            cy: (y0 + y1) / 2.0,
            zoom,
            width,
            height,
        }
    }

    /// View centred on node `i` at a zoom that makes labels visible.
    pub fn center_on(&self, i: usize, view: &View, min_zoom: f32) -> View {
        let n = &self.nodes[i];
        View {
            cx: n.pos[0],
            cy: n.pos[1],
            zoom: view.zoom.max(min_zoom),
            ..*view
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Scene {
        let mk = |id, x, y| Node {
            id,
            pos: [x, y],
            radius: 5.0,
            color: [1, 2, 3],
            label: "n".into(),
        };
        Scene::new(
            vec![mk(0, 0.0, 0.0), mk(1, 100.0, 0.0), mk(2, 102.0, 1.0)],
            vec![],
        )
    }

    #[test]
    fn hit_testing() {
        let s = scene();
        let v = View {
            cx: 0.0,
            cy: 0.0,
            zoom: 1.0,
            width: 400.0,
            height: 400.0,
        };
        let c = v.to_screen([0.0, 0.0]);
        assert_eq!(s.hit(&v, c), Some(0));
        assert_eq!(s.hit(&v, [c[0] + 50.0, c[1] + 50.0]), None);
        // nearest of two close nodes
        let p = v.to_screen([102.0, 1.0]);
        assert_eq!(s.hit(&v, p), Some(2));
        let p = v.to_screen([100.0, 0.0]);
        assert_eq!(s.hit(&v, p), Some(1));
    }

    #[test]
    fn fit_contains_all() {
        let s = scene();
        let v = s.fit_view(800.0, 600.0);
        for n in &s.nodes {
            let p = v.to_screen(n.pos);
            assert!(p[0] >= 0.0 && p[0] <= 800.0 && p[1] >= 0.0 && p[1] <= 600.0);
        }
        assert!(Scene::new(vec![], vec![]).fit_view(10.0, 10.0).zoom > 0.0);
    }
}
