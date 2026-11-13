//! Graph layouts: force-directed (Fruchterman–Reingold with Barnes–Hut
//! repulsion) and a layered (Sugiyama-style) layout for focused subgraphs.

pub type Pos = [f32; 2];

// ---- Barnes–Hut quadtree ----------------------------------------------------

struct QNode {
    cx: f32,
    cy: f32,
    mass: f32,
    x: f32,
    y: f32,
    half: f32,
    children: [u32; 4],
    leaf: u32, // point index when a leaf with one point, else u32::MAX
}

const NONE: u32 = u32::MAX;

struct QuadTree {
    nodes: Vec<QNode>,
}

impl QuadTree {
    fn new(pos: &[Pos], w: &[f32]) -> Self {
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pos {
            minx = minx.min(p[0]);
            maxx = maxx.max(p[0]);
            miny = miny.min(p[1]);
            maxy = maxy.max(p[1]);
        }
        if pos.is_empty() {
            (minx, miny, maxx, maxy) = (0.0, 0.0, 1.0, 1.0);
        }
        let half = ((maxx - minx).max(maxy - miny) / 2.0).max(1.0) + 1.0;
        let mut t = QuadTree {
            nodes: Vec::with_capacity(pos.len() * 2),
        };
        t.nodes.push(QNode {
            cx: 0.0,
            cy: 0.0,
            mass: 0.0,
            x: (minx + maxx) / 2.0,
            y: (miny + maxy) / 2.0,
            half,
            children: [NONE; 4],
            leaf: NONE,
        });
        for (i, p) in pos.iter().enumerate() {
            t.insert(0, i as u32, *p, w[i], pos, w, 0);
        }
        // finalize centres of mass
        for n in &mut t.nodes {
            if n.mass > 0.0 {
                n.cx /= n.mass;
                n.cy /= n.mass;
            }
        }
        t
    }

    fn child_for(&mut self, node: usize, p: Pos) -> usize {
        let (x, y, half) = (
            self.nodes[node].x,
            self.nodes[node].y,
            self.nodes[node].half,
        );
        let q = (p[0] >= x) as usize | (((p[1] >= y) as usize) << 1);
        if self.nodes[node].children[q] == NONE {
            let h = half / 2.0;
            let nx = if q & 1 == 1 { x + h } else { x - h };
            let ny = if q & 2 == 2 { y + h } else { y - h };
            let id = self.nodes.len() as u32;
            self.nodes.push(QNode {
                cx: 0.0,
                cy: 0.0,
                mass: 0.0,
                x: nx,
                y: ny,
                half: h,
                children: [NONE; 4],
                leaf: NONE,
            });
            self.nodes[node].children[q] = id;
        }
        self.nodes[node].children[q] as usize
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        &mut self,
        mut node: usize,
        idx: u32,
        p: Pos,
        mass: f32,
        pos: &[Pos],
        w: &[f32],
        mut depth: u32,
    ) {
        loop {
            // accumulate mass sums (centres divided at the end)
            {
                let n = &mut self.nodes[node];
                n.cx += p[0] * mass;
                n.cy += p[1] * mass;
                n.mass += mass;
            }
            let is_internal = self.nodes[node].children.iter().any(|&c| c != NONE);
            if !is_internal && self.nodes[node].leaf == NONE {
                self.nodes[node].leaf = idx;
                return;
            }
            if depth > 40 {
                return; // coincident points: aggregate only
            }
            if !is_internal {
                // split: push the existing leaf point down
                let old = self.nodes[node].leaf;
                self.nodes[node].leaf = NONE;
                let op = pos[old as usize];
                let c = self.child_for(node, op);
                // existing point goes into child (its mass is already counted in `node`)
                self.insert(c, old, op, w[old as usize], pos, w, depth + 1);
            }
            node = self.child_for(node, p);
            depth += 1;
        }
    }

    /// Repulsive force on point `i`: magnitude `k * mass / d` per cell.
    fn repulse(&self, i: usize, p: Pos, k2: f32, theta: f32) -> [f32; 2] {
        let mut f = [0.0f32, 0.0];
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni];
            if n.mass == 0.0 {
                continue;
            }
            let dx = p[0] - n.cx;
            let dy = p[1] - n.cy;
            let d2 = dx * dx + dy * dy;
            let is_leaf = n.children.iter().all(|&c| c == NONE);
            if is_leaf && n.leaf == i as u32 {
                continue;
            }
            let size = n.half * 2.0;
            if is_leaf || size * size < theta * theta * d2 {
                let d2 = d2.max(0.01);
                let s = k2 * n.mass / d2;
                f[0] += dx * s;
                f[1] += dy * s;
            } else {
                for &c in &n.children {
                    if c != NONE {
                        stack.push(c as usize);
                    }
                }
            }
        }
        f
    }
}

// ---- force layout -----------------------------------------------------------

/// Tunable force parameters.
#[derive(Debug, Clone, Copy)]
pub struct ForceParams {
    /// Repulsion strength.
    pub repulsion: f32,
    /// Constant-magnitude pull towards the origin, scaled by node mass.
    pub gravity: f32,
    /// Node mass = (degree + 1)^mass_exp; <1 tames hubs.
    pub mass_exp: f32,
}

impl Default for ForceParams {
    fn default() -> Self {
        ForceParams {
            repulsion: 6.0,
            gravity: 3.0,
            mass_exp: 0.3,
        }
    }
}

/// Force-directed layout (LinLog attraction, degree-weighted Barnes–Hut
/// repulsion, ForceAtlas2-style gravity) with simulated-annealing cooling.
pub struct ForceLayout {
    pub pos: Vec<Pos>,
    edges: Vec<(u32, u32)>,
    mass: Vec<f32>,
    params: ForceParams,
    temp: f32,
    min_temp: f32,
    pub iterations: u32,
}

impl ForceLayout {
    /// `seed` gives optional warm-start positions (e.g. from the cache).
    pub fn new(n: usize, edges: &[(usize, usize)], seed: &[Option<Pos>]) -> Self {
        Self::with_params(n, edges, seed, ForceParams::default())
    }

    pub fn with_params(
        n: usize,
        edges: &[(usize, usize)],
        seed: &[Option<Pos>],
        params: ForceParams,
    ) -> Self {
        let radius = 60.0 * (n as f32).sqrt();
        let mut pos = Vec::with_capacity(n);
        for i in 0..n {
            let p = match seed.get(i).copied().flatten() {
                Some(p) => p,
                None => {
                    // deterministic golden-angle spiral
                    let t = (i as f32 + 0.5) / n.max(1) as f32;
                    let r = radius * t.sqrt();
                    let a = i as f32 * 2.399_963;
                    [r * a.cos(), r * a.sin()]
                }
            };
            pos.push(p);
        }
        let mut deg = vec![0u32; n];
        for &(a, b) in edges {
            if a < n && b < n {
                deg[a] += 1;
                deg[b] += 1;
            }
        }
        let mass = deg
            .iter()
            .map(|&d| (d as f32 + 1.0).powf(params.mass_exp))
            .collect();
        let warm = !seed.is_empty() && seed.iter().all(|s| s.is_some());
        ForceLayout {
            pos,
            edges: edges
                .iter()
                .filter(|&&(a, b)| a < n && b < n)
                .map(|&(a, b)| (a as u32, b as u32))
                .collect(),
            mass,
            params,
            temp: if warm { radius * 0.02 } else { radius * 0.2 },
            min_temp: 0.3,
            iterations: 0,
        }
    }

    pub fn converged(&self) -> bool {
        self.temp <= self.min_temp * 1.001
    }

    /// Run one iteration with Barnes–Hut repulsion.
    pub fn step(&mut self) {
        let n = self.pos.len();
        if n == 0 {
            return;
        }
        let tree = QuadTree::new(&self.pos, &self.mass);
        let kr = self.params.repulsion;
        let mut disp: Vec<[f32; 2]> = (0..n)
            .map(|i| tree.repulse(i, self.pos[i], kr * self.mass[i], 0.9))
            .collect();
        for &(a, b) in &self.edges {
            let (a, b) = (a as usize, b as usize);
            let dx = self.pos[a][0] - self.pos[b][0];
            let dy = self.pos[a][1] - self.pos[b][1];
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            // LinLog: magnitude ln(1+d); (dx,dy)/d is the unit vector
            let f = (1.0 + d).ln() / d;
            disp[a][0] -= dx * f;
            disp[a][1] -= dy * f;
            disp[b][0] += dx * f;
            disp[b][1] += dy * f;
        }
        for (i, d_i) in disp.iter_mut().enumerate() {
            let (x, y) = (self.pos[i][0], self.pos[i][1]);
            let r = (x * x + y * y).sqrt().max(0.01);
            let g = self.params.gravity * self.mass[i];
            let g = g * (1.0 + r / 300.0);
            d_i[0] -= x / r * g;
            d_i[1] -= y / r * g;
            let d = (d_i[0] * d_i[0] + d_i[1] * d_i[1]).sqrt();
            if d > 0.0 {
                let m = d.min(self.temp);
                self.pos[i][0] += d_i[0] / d * m;
                self.pos[i][1] += d_i[1] / d * m;
            }
        }
        self.temp = (self.temp * 0.985).max(self.min_temp);
        self.iterations += 1;
    }
}

// ---- layered layout ---------------------------------------------------------

/// Layered layout of a (sub)graph. Edges point from dependent to dependency,
/// so layer 0 holds the top-most dependents. Cycles are broken by ignoring
/// DFS back edges. Long edges are not split with dummy nodes.
pub fn layered_layout(n: usize, edges: &[(usize, usize)], x_gap: f32, y_gap: f32) -> Vec<Pos> {
    if n == 0 {
        return Vec::new();
    }
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in edges {
        if a < n && b < n && a != b {
            out[a].push(b);
        }
    }
    // cycle breaking: iterative DFS, drop back edges
    let mut state = vec![0u8; n]; // 0 new, 1 on stack, 2 done
    let mut dag: Vec<Vec<usize>> = vec![Vec::new(); n];
    for s in 0..n {
        if state[s] != 0 {
            continue;
        }
        let mut stack = vec![(s, 0usize)];
        state[s] = 1;
        while let Some(&mut (u, ref mut i)) = stack.last_mut() {
            if *i < out[u].len() {
                let v = out[u][*i];
                *i += 1;
                match state[v] {
                    0 => {
                        dag[u].push(v);
                        state[v] = 1;
                        stack.push((v, 0));
                    }
                    2 => dag[u].push(v),
                    _ => {} // back edge: skip
                }
            } else {
                state[u] = 2;
                stack.pop();
            }
        }
    }
    // longest-path layering via Kahn
    let mut indeg = vec![0usize; n];
    for outs in &dag {
        for &v in outs {
            indeg[v] += 1;
        }
    }
    let mut layer = vec![0usize; n];
    let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut head = 0;
    while head < queue.len() {
        let u = queue[head];
        head += 1;
        for &v in &dag[u] {
            layer[v] = layer[v].max(layer[u] + 1);
            indeg[v] -= 1;
            if indeg[v] == 0 {
                queue.push(v);
            }
        }
    }
    let nl = layer.iter().max().copied().unwrap_or(0) + 1;
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); nl];
    for i in 0..n {
        rows[layer[i]].push(i);
    }
    // barycenter ordering sweeps
    let mut inn: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (u, outs) in dag.iter().enumerate() {
        for &v in outs {
            inn[v].push(u);
        }
    }
    let mut order = vec![0f32; n];
    for r in &rows {
        for (j, &v) in r.iter().enumerate() {
            order[v] = j as f32;
        }
    }
    for sweep in 0..6 {
        let down = sweep % 2 == 0;
        let idxs: Vec<usize> = if down {
            (1..nl).collect()
        } else {
            (0..nl.saturating_sub(1)).rev().collect()
        };
        for l in idxs {
            let nb = if down { &inn } else { &dag };
            let mut keyed: Vec<(f32, usize)> = rows[l]
                .iter()
                .map(|&v| {
                    let ns = &nb[v];
                    let b = if ns.is_empty() {
                        order[v]
                    } else {
                        ns.iter().map(|&u| order[u]).sum::<f32>() / ns.len() as f32
                    };
                    (b, v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            rows[l] = keyed.iter().map(|k| k.1).collect();
            for (j, &v) in rows[l].iter().enumerate() {
                order[v] = j as f32;
            }
        }
    }
    let mut pos = vec![[0.0f32; 2]; n];
    for (l, r) in rows.iter().enumerate() {
        let w = (r.len().saturating_sub(1)) as f32 * x_gap;
        for (j, &v) in r.iter().enumerate() {
            pos[v] = [j as f32 * x_gap - w / 2.0, l as f32 * y_gap];
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_layout_finite_and_separates() {
        let n = 200;
        let edges: Vec<(usize, usize)> = (1..n).map(|i| (i, i / 2)).collect();
        let mut l = ForceLayout::new(n, &edges, &[]);
        for _ in 0..150 {
            l.step();
        }
        assert!(l.pos.iter().all(|p| p[0].is_finite() && p[1].is_finite()));
        // no two nodes may coincide
        for i in 0..n {
            for j in i + 1..n {
                let d = (l.pos[i][0] - l.pos[j][0]).hypot(l.pos[i][1] - l.pos[j][1]);
                assert!(d > 0.01, "nodes {i},{j} coincide");
            }
        }
    }

    #[test]
    fn coincident_seed_points_do_not_hang() {
        let seed = vec![Some([1.0, 1.0]); 50];
        let mut l = ForceLayout::new(50, &[], &seed);
        l.step();
        assert!(l.pos.iter().all(|p| p[0].is_finite()));
    }

    #[test]
    fn layered_respects_direction_and_cycles() {
        // 0 -> 1 -> 2, plus back edge 2 -> 0
        let p = layered_layout(3, &[(0, 1), (1, 2), (2, 0)], 10.0, 20.0);
        assert!(p[0][1] < p[1][1] && p[1][1] < p[2][1]);
        let p = layered_layout(4, &[(0, 1), (0, 2), (1, 3), (2, 3)], 10.0, 20.0);
        assert_eq!(p[3][1], 40.0);
        assert!(layered_layout(0, &[], 1.0, 1.0).is_empty());
    }
}
