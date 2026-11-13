//! Application state and wiring between the Slint UI, background workers and
//! the renderer. All state lives on the UI thread; workers talk to it with
//! `slint::invoke_from_event_loop`.

use crate::export;
use crate::panels::{self, to_model, R};
use crate::{LegendItem, MainWindow, StatsInfo};
use depweave_core::cache::{self, LayoutCache};
use depweave_core::graph::human_size;
use depweave_core::layout::{layered_layout, ForceLayout, Pos};
use depweave_core::{load, DepGraph, Direction, EdgeKind, LoadOptions, Reason};
use depweave_render::palette;
use depweave_render::tiny_skia::PixmapMut;
use depweave_render::{Edge, Highlight, Node, Renderer, Scene, Style, View};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel, Weak,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

pub static START: OnceLock<Instant> = OnceLock::new();

pub fn elapsed_ms() -> u128 {
    START.get().map_or(0, |s| s.elapsed().as_millis())
}

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kb| kb / 1024.0)
}

// ---------------------------------------------------------------------------
// messages from worker threads
// ---------------------------------------------------------------------------

enum Msg {
    Loaded(u64, Result<(Arc<DepGraph>, Option<LayoutCache>), String>),
    Flash(String),
    Layout(u64, Arc<Vec<Pos>>, bool),
    Impacts(u64, Arc<Vec<(usize, u64)>>),
    FsChanged,
    FirstFrame,
}

thread_local! {
    static APP: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) };
}

fn post(msg: Msg) {
    let _ = slint::invoke_from_event_loop(move || deliver(msg));
}

/// Runs on the UI thread. Never panics on a busy RefCell: if the state is
/// borrowed (re-entrant event loop), the message is re-queued.
fn deliver(msg: Msg) {
    let app = APP.with(|a| a.try_borrow().ok().and_then(|a| a.clone()));
    let Some(app) = app else { return };
    if app.try_borrow_mut().is_err() {
        let _ = slint::invoke_from_event_loop(move || deliver(msg));
        return;
    }
    State::on_msg(&app, msg);
}

// ---------------------------------------------------------------------------
// render thread
// ---------------------------------------------------------------------------

struct RenderReq {
    scene: Arc<Scene>,
    view: View,
    hl: Highlight,
    style: Style,
}

fn spawn_render_thread(ui: Weak<MainWindow>) -> mpsc::Sender<RenderReq> {
    let (tx, rx) = mpsc::channel::<RenderReq>();
    std::thread::spawn(move || {
        let mut renderer = Renderer::new();
        let mut first = true;
        while let Ok(mut req) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                req = newer; // coalesce: only the latest view matters
            }
            let (w, h) = (req.view.width as u32, req.view.height as u32);
            if w < 2 || h < 2 {
                continue;
            }
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
            if let Some(mut pm) = PixmapMut::from_bytes(buf.make_mut_bytes(), w, h) {
                renderer.render(&mut pm, &req.scene, &req.view, &req.hl, &req.style);
            }
            let ui = ui.clone();
            let was_first = std::mem::replace(&mut first, false);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_graph_image(Image::from_rgba8_premultiplied(buf));
                }
                if was_first {
                    post(Msg::FirstFrame);
                }
            });
        }
    });
    tx
}

// ---------------------------------------------------------------------------
// state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorBy {
    Reason,
    Repo,
    Size,
}

struct Drag {
    last: [f32; 2],
    start: [f32; 2],
    moved: bool,
}

pub struct State {
    ui: Weak<MainWindow>,
    render_tx: mpsc::Sender<RenderReq>,
    opts: LoadOptions,
    graph: Option<Arc<DepGraph>>,
    labels: Vec<Arc<str>>,
    all_edges: Vec<(usize, usize, EdgeKind)>,
    max_size: u64,
    positions: Arc<Vec<Pos>>,
    layout_done: bool,
    gen: Arc<AtomicU64>,
    // view / scene
    scene: Option<Arc<Scene>>,
    scene_pkgs: Vec<usize>,
    pkg_scene: HashMap<usize, usize>,
    view: View,
    scale: f32,
    user_moved: bool,
    drag: Option<Drag>,
    // interaction
    selected: Option<usize>,
    hovered: Option<usize>, // scene index
    matches: Vec<usize>,    // package indices
    focus: Option<usize>,
    why_limit: usize,
    // options mirrored from the UI
    color_by: ColorBy,
    size_scale: bool,
    show_optional: bool,
    hierarchical: bool,
    focus_mode: bool,
    hops: usize,
    dir: Direction,
    dark: bool,
    // sidebar lists
    lists: [Vec<R>; 5],
    impact_ready: bool,
    _watcher: Option<notify::RecommendedWatcher>,
    selftest: bool,
    snapshot_pending: bool,
}

impl State {
    fn ui(&self) -> Option<MainWindow> {
        self.ui.upgrade()
    }

    fn g(&self) -> Option<Arc<DepGraph>> {
        self.graph.clone()
    }

    // ---- loading ----------------------------------------------------------

    fn start_load(&mut self) {
        let my_gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(ui) = self.ui() {
            ui.set_loading(true);
            ui.set_loading_text(SharedString::from("Reading pacman database…"));
        }
        let opts = self.opts.clone();
        let want_cache = self.graph.is_none();
        std::thread::spawn(move || {
            // all disk I/O (db + layout cache) happens here, not on the UI thread
            let r = load(&opts)
                .map(|g| (Arc::new(g), want_cache.then(cache::load_layout).flatten()))
                .map_err(|e| e.to_string());
            post(Msg::Loaded(my_gen, r));
        });
    }

    fn on_msg(app: &Rc<RefCell<State>>, msg: Msg) {
        let mut st = app.borrow_mut();
        match msg {
            Msg::Flash(t) => st.flash(t),
            Msg::Loaded(gen, res) => {
                if gen != st.gen.load(Ordering::SeqCst) {
                    return;
                }
                match res {
                    Ok((g, layout_cache)) => st.on_loaded(g, layout_cache),
                    Err(e) => {
                        if let Some(ui) = st.ui() {
                            ui.set_loading(true);
                            ui.set_loading_text(SharedString::from(format!(
                                "Cannot read pacman database: {e}"
                            )));
                        }
                    }
                }
            }
            Msg::Layout(gen, pos, done) => {
                if gen != st.gen.load(Ordering::SeqCst) {
                    return;
                }
                st.positions = pos;
                st.layout_done = done;
                if st.focus_active() && st.hierarchical {
                    // the layered layout does not use force positions:
                    // don't recompute it on every progressive update
                    st.set_status();
                } else {
                    let refit = !st.user_moved;
                    st.rebuild_scene(refit);
                    st.set_status();
                }
            }
            Msg::Impacts(gen, imp) => {
                if gen != st.gen.load(Ordering::SeqCst) {
                    return;
                }
                st.on_impacts(&imp);
            }
            Msg::FsChanged => {
                log::info!("pacman database changed; reloading");
                st.start_load();
            }
            Msg::FirstFrame => {
                log::info!("first frame {} ms, rss {:.1} MB", elapsed_ms(), rss_mb());
                if st.selftest {
                    eprintln!(
                        "selftest: first-frame {} ms rss {:.1} MB",
                        elapsed_ms(),
                        rss_mb()
                    );
                }
            }
        }
        drop(st);
        // self-test: drive the UI through a scenario, snapshot, then exit
        let ready = {
            let st = app.borrow();
            st.selftest && st.layout_done && st.graph.is_some() && !st.snapshot_pending
        };
        if ready {
            app.borrow_mut().snapshot_pending = true;
            let ui = app.borrow().ui();
            if let Some(ui) = ui {
                eprintln!(
                    "selftest: layout done {} ms rss {:.1} MB",
                    elapsed_ms(),
                    rss_mb()
                );
                selftest_step(ui, 0);
            }
        }
    }

    fn on_loaded(&mut self, g: Arc<DepGraph>, layout_cache: Option<LayoutCache>) {
        log::info!("graph loaded: {} packages at {} ms", g.len(), elapsed_ms());
        // remember selection by name across reloads
        let prev_sel = self
            .selected
            .and_then(|i| self.graph.as_ref().map(|old| old.packages[i].name.clone()));
        let prev_focus = self
            .focus
            .and_then(|i| self.graph.as_ref().map(|old| old.packages[i].name.clone()));
        // seed layout from previous positions (by name) or the on-disk cache
        let mut seed_map: HashMap<String, Pos> = HashMap::new();
        let mut converged = false;
        if let Some(old) = &self.graph {
            for (i, p) in old.packages.iter().enumerate() {
                if let Some(pos) = self.positions.get(i) {
                    seed_map.insert(p.name.clone(), *pos);
                }
            }
        } else if let Some(c) = layout_cache {
            converged = c.converged && c.key == g.structure_hash();
            seed_map = c.positions;
        }
        let seed: Vec<Option<Pos>> = g
            .packages
            .iter()
            .map(|p| seed_map.get(&p.name).copied())
            .collect();
        let complete = seed.iter().all(|s| s.is_some());
        let first_load = self.graph.is_none();

        self.labels = g
            .packages
            .iter()
            .map(|p| Arc::from(p.name.as_str()))
            .collect();
        self.all_edges = g.edges(true);
        self.max_size = g.packages.iter().map(|p| p.size).max().unwrap_or(0);
        self.selected = prev_sel.and_then(|n| g.find(&n));
        self.focus = prev_focus.and_then(|n| g.find(&n));
        self.matches.clear();
        self.hovered = None;
        self.impact_ready = false;
        self.graph = Some(g.clone());
        let gen = self.gen.load(Ordering::SeqCst);

        // initial positions: cached/seeded where known, otherwise spiral
        let fl = ForceLayout::new(g.len(), &[], &seed);
        self.positions = Arc::new(fl.pos);
        self.layout_done = complete && (converged || !first_load);
        self.build_lists(&g);
        self.set_stats(&g);
        self.rebuild_scene(first_load || !self.user_moved);
        if let Some(ui) = self.ui() {
            ui.set_loading(false);
        }
        self.update_panels();
        self.set_status();

        // layout worker (skipped when the cache already holds a converged layout)
        if !(complete && converged) {
            spawn_layout(g.clone(), seed, gen);
        } else {
            self.layout_done = true;
        }
        // removal impact worker
        let g2 = g.clone();
        std::thread::spawn(move || post(Msg::Impacts(gen, Arc::new(g2.all_impacts()))));
    }

    fn build_lists(&mut self, g: &DepGraph) {
        let n = g.len();
        let mut orphans: Vec<usize> = g.orphans();
        orphans.sort_by_key(|&i| std::cmp::Reverse(g.packages[i].size));
        let mut explicit: Vec<usize> = (0..n)
            .filter(|&i| g.packages[i].reason == Reason::Explicit)
            .collect();
        explicit.sort_by(|&a, &b| g.packages[a].name.cmp(&g.packages[b].name));
        let mut foreign: Vec<usize> = (0..n).filter(|&i| g.packages[i].is_foreign()).collect();
        foreign.sort_by(|&a, &b| g.packages[a].name.cmp(&g.packages[b].name));
        let mut largest: Vec<usize> = (0..n).collect();
        largest.sort_by_key(|&i| std::cmp::Reverse(g.packages[i].size));
        largest.truncate(200);
        let rows = |v: &[usize]| v.iter().map(|&i| panels::pkg_row(g, i)).collect::<Vec<_>>();
        self.lists = [
            rows(&orphans),
            rows(&explicit),
            rows(&foreign),
            rows(&largest),
            vec![R::note("Computing removal impact…")],
        ];
        self.show_list();
    }

    fn on_impacts(&mut self, imp: &[(usize, u64)]) {
        let Some(g) = self.g() else { return };
        let mut idx: Vec<usize> = (0..imp.len()).filter(|&i| imp[i].0 > 0).collect();
        idx.sort_by_key(|&i| std::cmp::Reverse(imp[i].1));
        idx.truncate(200);
        self.lists[4] = idx
            .iter()
            .map(|&i| {
                R::link(
                    &g.packages[i].name,
                    format!("+{} · {}", imp[i].0, human_size(imp[i].1)),
                )
            })
            .collect();
        self.impact_ready = true;
        self.show_list();
    }

    fn show_list(&self) {
        if let Some(ui) = self.ui() {
            self.show_list_tab(ui.get_list_tab());
        }
    }

    fn show_list_tab(&self, tab: i32) {
        if let Some(ui) = self.ui() {
            let tab = (tab.max(0) as usize).min(4);
            ui.set_list_rows(to_model(&self.lists[tab]));
        }
    }

    fn set_stats(&self, g: &DepGraph) {
        let s = g.stats();
        if let Some(ui) = self.ui() {
            ui.set_stats(StatsInfo {
                total: s.total.to_string().into(),
                explicit: s.explicit.to_string().into(),
                deps: s.dependencies.to_string().into(),
                size: human_size(s.total_size).into(),
                orphans: format!("{} ({})", s.orphans, human_size(s.orphan_size)).into(),
                edges: format!("{} hard · {} optional", s.edges_hard, s.edges_optional).into(),
            });
        }
    }

    fn set_status(&self) {
        let (Some(ui), Some(g)) = (self.ui(), self.g()) else {
            return;
        };
        let shown = self.scene.as_ref().map_or(0, |s| s.nodes.len());
        let mut t = format!("{shown} of {} packages shown", g.len());
        if !self.layout_done {
            t.push_str(" · layout refining…");
        }
        if !g.warnings.is_empty() {
            t.push_str(&format!(
                " · {} warning(s) while reading the database",
                g.warnings.len()
            ));
        }
        ui.set_status_text(t.into());
    }

    fn flash(&self, msg: String) {
        if let Some(ui) = self.ui() {
            ui.set_status_text(msg.into());
        }
    }

    // ---- scene --------------------------------------------------------------

    fn focus_active(&self) -> bool {
        self.focus_mode && self.focus.is_some()
    }

    fn rebuild_scene(&mut self, refit: bool) {
        let Some(g) = self.g() else { return };
        let visible: Vec<usize> = match (self.focus_active(), self.focus) {
            (true, Some(root)) => g.neighborhood(root, self.hops, self.dir, self.show_optional),
            _ => (0..g.len()).collect(),
        };
        let mut pkg_scene = HashMap::with_capacity(visible.len());
        for (si, &p) in visible.iter().enumerate() {
            pkg_scene.insert(p, si);
        }
        let edges: Vec<(usize, usize, bool)> = self
            .all_edges
            .iter()
            .filter(|(_, _, k)| self.show_optional || *k == EdgeKind::Hard)
            .filter_map(|&(a, b, k)| {
                Some((
                    *pkg_scene.get(&a)?,
                    *pkg_scene.get(&b)?,
                    k == EdgeKind::Optional,
                ))
            })
            .collect();
        let positions: Vec<Pos> = if self.focus_active() && self.hierarchical {
            let e: Vec<(usize, usize)> = edges.iter().map(|&(a, b, _)| (a, b)).collect();
            layered_layout(visible.len(), &e, 80.0, 90.0)
        } else {
            visible
                .iter()
                .map(|&p| self.positions.get(p).copied().unwrap_or([0.0, 0.0]))
                .collect()
        };
        let custom: Vec<&String> = g
            .repos
            .iter()
            .filter(|r| !matches!(r.as_str(), "core" | "extra" | "multilib"))
            .collect();
        let nodes: Vec<Node> = visible
            .iter()
            .zip(positions)
            .map(|(&p, pos)| {
                let pk = &g.packages[p];
                let color = match self.color_by {
                    ColorBy::Reason => {
                        if g.is_orphan(p) {
                            palette::ORPHAN
                        } else if pk.reason == Reason::Explicit {
                            palette::EXPLICIT
                        } else {
                            palette::DEPENDENCY
                        }
                    }
                    ColorBy::Repo => {
                        let ci = pk
                            .repo
                            .as_ref()
                            .and_then(|r| custom.iter().position(|c| *c == r))
                            .unwrap_or(0);
                        palette::repo_color(pk.repo.as_deref(), ci)
                    }
                    ColorBy::Size => palette::gradient(palette::size_t(pk.size, self.max_size)),
                };
                let radius = if self.size_scale {
                    3.0 + 9.0 * palette::size_t(pk.size, self.max_size)
                } else {
                    5.0
                };
                Node {
                    id: p,
                    pos,
                    radius,
                    color,
                    label: self.labels[p].clone(),
                }
            })
            .collect();
        let edges: Vec<Edge> = edges
            .into_iter()
            .map(|(a, b, o)| Edge {
                from: a as u32,
                to: b as u32,
                optional: o,
            })
            .collect();
        let scene = Scene::new(nodes, edges);
        if refit {
            self.view = scene.fit_view(self.view.width, self.view.height);
            if self.focus_active() {
                self.user_moved = false;
            }
        }
        self.scene = Some(Arc::new(scene));
        self.scene_pkgs = visible;
        self.pkg_scene = pkg_scene;
        self.hovered = None;
        self.update_legend(&g);
        self.set_status();
        self.request_render();
    }

    fn update_legend(&self, g: &DepGraph) {
        let Some(ui) = self.ui() else { return };
        let c = |rgb: [u8; 3]| slint::Color::from_rgb_u8(rgb[0], rgb[1], rgb[2]);
        let item = |rgb, t: &str| LegendItem {
            c: c(rgb),
            t: t.into(),
        };
        let items = match self.color_by {
            ColorBy::Reason => vec![
                item(palette::EXPLICIT, "explicitly installed"),
                item(palette::DEPENDENCY, "dependency"),
                item(palette::ORPHAN, "orphan"),
            ],
            ColorBy::Repo => {
                let custom: Vec<&String> = g
                    .repos
                    .iter()
                    .filter(|r| !matches!(r.as_str(), "core" | "extra" | "multilib"))
                    .collect();
                let mut v: Vec<LegendItem> = g
                    .repos
                    .iter()
                    .map(|r| {
                        let ci = custom.iter().position(|x| *x == r).unwrap_or(0);
                        item(palette::repo_color(Some(r), ci), r)
                    })
                    .collect();
                v.push(item(palette::FOREIGN, "foreign (AUR/manual)"));
                v
            }
            ColorBy::Size => vec![
                item(palette::gradient(0.0), "small"),
                item(palette::gradient(0.5), "medium"),
                item(palette::gradient(1.0), "large"),
            ],
        };
        ui.set_legend(ModelRc::new(VecModel::from(items)));
    }

    fn highlight(&self) -> Highlight {
        Highlight {
            selected: self.selected.and_then(|p| self.pkg_scene.get(&p).copied()),
            hovered: self.hovered,
            matches: self
                .matches
                .iter()
                .filter_map(|p| self.pkg_scene.get(p).copied())
                .collect(),
        }
    }

    fn style(&self) -> Style {
        Style {
            dark: self.dark,
            show_optional: self.show_optional,
            label_zoom: 1.2,
            scale: self.scale,
        }
    }

    fn request_render(&self) {
        if let Some(scene) = &self.scene {
            let _ = self.render_tx.send(RenderReq {
                scene: scene.clone(),
                view: self.view,
                hl: self.highlight(),
                style: self.style(),
            });
        }
    }

    // ---- selection / panels -----------------------------------------------------

    fn update_panels(&self) {
        let (Some(ui), Some(g)) = (self.ui(), self.g()) else {
            return;
        };
        match self.selected {
            Some(i) => {
                let p = &g.packages[i];
                ui.set_detail_title(format!("{} {}", p.name, p.version).into());
                ui.set_detail_rows(to_model(&panels::details_rows(&g, i)));
                let w = panels::why_rows(&g, i, self.why_limit);
                ui.set_why_title(w.title.into());
                ui.set_why_rows(to_model(&w.rows));
                ui.set_orphan_cmd(w.command.into());
                ui.set_why_more_visible(w.truncated);
            }
            None => {
                ui.set_detail_title("No package selected".into());
                ui.set_detail_rows(to_model(&[R::note(
                    "Click a node, or search / pick a package",
                )]));
                ui.set_why_title("Why is it installed?".into());
                ui.set_why_rows(to_model(&[R::note(
                    "Select a package to see why it is installed",
                )]));
                ui.set_orphan_cmd("".into());
                ui.set_why_more_visible(false);
            }
        }
    }

    fn select(&mut self, pkg: Option<usize>, center: bool) {
        self.selected = pkg;
        self.why_limit = 20;
        self.matches.clear();
        if let Some(p) = pkg {
            if self.focus_mode {
                self.focus = Some(p);
                self.rebuild_scene(true);
            } else if let (true, Some(&si), Some(scene)) =
                (center, self.pkg_scene.get(&p), self.scene.clone())
            {
                self.view = scene.center_on(si, &self.view, 1.5);
                self.user_moved = true;
            }
        }
        self.update_panels();
        self.request_render();
    }

    // ---- pointer ------------------------------------------------------------------

    fn phys(&self, x: f32, y: f32) -> [f32; 2] {
        [x * self.scale, y * self.scale]
    }

    fn set_hover(&mut self, h: Option<usize>) {
        if h == self.hovered {
            return;
        }
        self.hovered = h;
        if let (Some(ui), Some(g)) = (self.ui(), self.g()) {
            let t = h
                .and_then(|si| self.scene_pkgs.get(si))
                .map(|&p| {
                    format!(
                        "{} · {} · {}",
                        g.packages[p].name,
                        g.packages[p].version,
                        human_size(g.packages[p].size)
                    )
                })
                .unwrap_or_default();
            ui.set_hover_text(t.into());
        }
        self.request_render();
    }

    fn pointer_move(&mut self, x: f32, y: f32, pressed: bool) {
        let p = self.phys(x, y);
        if let Some(d) = &mut self.drag {
            if pressed {
                if !d.moved && ((p[0] - d.start[0]).abs() > 4.0 || (p[1] - d.start[1]).abs() > 4.0)
                {
                    d.moved = true;
                }
                if d.moved {
                    let (dx, dy) = (p[0] - d.last[0], p[1] - d.last[1]);
                    d.last = p;
                    self.view.pan(dx, dy);
                    self.user_moved = true;
                    self.request_render();
                } else {
                    d.last = p;
                }
                return;
            }
        }
        let hit = self.scene.as_ref().and_then(|s| s.hit(&self.view, p));
        self.set_hover(hit);
    }

    fn pointer_up(&mut self, x: f32, y: f32) {
        let Some(d) = self.drag.take() else { return };
        if d.moved {
            return;
        }
        let hit = self
            .scene
            .as_ref()
            .and_then(|s| s.hit(&self.view, self.phys(x, y)));
        let pkg = hit.and_then(|si| self.scene_pkgs.get(si).copied());
        self.select(pkg, false);
    }

    fn scroll(&mut self, x: f32, y: f32, dy: f32) {
        let f = 1.0015f32.powf(dy.clamp(-400.0, 400.0));
        let p = self.phys(x, y);
        self.view.zoom_at(p, f);
        self.user_moved = true;
        self.request_render();
    }

    fn double_click(&mut self, x: f32, y: f32) {
        let hit = self
            .scene
            .as_ref()
            .and_then(|s| s.hit(&self.view, self.phys(x, y)));
        if let Some(si) = hit {
            let pkg = self.scene_pkgs[si];
            self.selected = Some(pkg);
            if let Some(scene) = self.scene.clone() {
                self.view = scene.center_on(si, &self.view, 2.5);
                self.user_moved = true;
            }
            self.update_panels();
            self.request_render();
        }
    }

    fn resized(&mut self, w: f32, h: f32) {
        let scale = self.ui().map_or(1.0, |u| u.window().scale_factor());
        self.scale = scale;
        let (pw, ph) = ((w * scale).max(1.0), (h * scale).max(1.0));
        if (pw - self.view.width).abs() < 0.5 && (ph - self.view.height).abs() < 0.5 {
            return;
        }
        if self.user_moved || self.scene.is_none() {
            self.view.width = pw;
            self.view.height = ph;
        } else if let Some(s) = &self.scene {
            self.view = s.fit_view(pw, ph);
        }
        self.request_render();
    }

    // ---- controls ---------------------------------------------------------------------

    fn read_controls(&mut self) -> bool {
        let Some(ui) = self.ui() else { return false };
        let focus_before = (self.focus_active(), self.hops, self.dir, self.hierarchical);
        self.color_by = match ui.get_color_index() {
            1 => ColorBy::Repo,
            2 => ColorBy::Size,
            _ => ColorBy::Reason,
        };
        self.size_scale = ui.get_size_scale();
        self.show_optional = ui.get_show_optional();
        self.hierarchical = ui.get_layout_index() == 1;
        self.hops = ui.get_hops().clamp(1, 5) as usize;
        self.dir = match ui.get_dir_index() {
            0 => Direction::Dependencies,
            1 => Direction::Dependents,
            _ => Direction::Both,
        };
        self.dark = ui.get_dark();
        let was_focus_mode = self.focus_mode;
        self.focus_mode = ui.get_focus_mode();
        if self.focus_mode && !was_focus_mode && self.focus.is_none() {
            self.focus = self.selected;
        }
        if self.hierarchical && !self.focus_active() {
            ui.set_layout_note("Hierarchical layout applies to focused subgraphs".into());
        } else {
            ui.set_layout_note("".into());
        }
        focus_before != (self.focus_active(), self.hops, self.dir, self.hierarchical)
    }

    fn controls_changed(&mut self) {
        let refit = self.read_controls();
        self.rebuild_scene(refit);
    }

    fn search(&mut self, text: &str) {
        let Some(g) = self.g() else { return };
        let ui = match self.ui() {
            Some(u) => u,
            None => return,
        };
        let q = text.trim();
        if q.is_empty() {
            self.matches.clear();
            ui.set_search_results(to_model(&[]));
            self.request_render();
            return;
        }
        let m = SkimMatcherV2::default();
        let mut scored: Vec<(i64, usize)> = g
            .packages
            .iter()
            .enumerate()
            .filter_map(|(i, p)| m.fuzzy_match(&p.name, q).map(|s| (s, i)))
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(g.packages[a.1].name.len().cmp(&g.packages[b.1].name.len()))
        });
        scored.truncate(12);
        self.matches = scored.iter().map(|s| s.1).collect();
        let rows: Vec<R> = scored
            .iter()
            .take(8)
            .map(|s| panels::pkg_row(&g, s.1))
            .collect();
        ui.set_search_results(to_model(&rows));
        self.request_render();
    }

    fn search_accept(&mut self) {
        if let Some(&top) = self.matches.first() {
            if let Some(ui) = self.ui() {
                ui.set_search_results(to_model(&[]));
            }
            self.select(Some(top), true);
        }
    }

    /// Exports run on a worker thread: rendering, font loading and file
    /// writes must not stall the UI.
    fn do_export(&self, kind: &str) {
        let (Some(scene), Some(g)) = (self.scene.clone(), self.g()) else {
            return;
        };
        let kind = kind.to_string();
        let (view, hl, style) = (self.view, self.highlight(), self.style());
        let pkgs = self.scene_pkgs.clone();
        let show_optional = self.show_optional;
        self.flash(format!("Exporting {}…", kind.to_uppercase()));
        std::thread::spawn(move || {
            let msg =
                match export_to_file(&kind, &scene, &g, &pkgs, view, &hl, &style, show_optional) {
                    Ok(p) => format!("Exported {}", p.display()),
                    Err(e) => format!("Export failed: {e}"),
                };
            post(Msg::Flash(msg));
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn export_to_file(
    kind: &str,
    scene: &Scene,
    g: &DepGraph,
    scene_pkgs: &[usize],
    view: View,
    hl: &Highlight,
    style: &Style,
    show_optional: bool,
) -> Result<std::path::PathBuf, String> {
    {
        let path = export::target_path(kind).map_err(|e| e.to_string())?;
        let res: Result<(), String> = match kind {
            "png" => {
                let (w, h) = (view.width as u32, view.height as u32);
                match depweave_render::tiny_skia::Pixmap::new(w.max(1), h.max(1)) {
                    Some(mut pm) => {
                        Renderer::new().render(&mut pm.as_mut(), scene, &view, hl, style);
                        pm.save_png(&path).map_err(|e| e.to_string())
                    }
                    None => Err("empty view".into()),
                }
            }
            "svg" => std::fs::write(&path, depweave_render::svg::to_svg(scene, &view, hl, style))
                .map_err(|e| e.to_string()),
            "dot" => std::fs::write(
                &path,
                depweave_core::export::to_dot(g, Some(scene_pkgs), show_optional),
            )
            .map_err(|e| e.to_string()),
            _ => std::fs::write(
                &path,
                depweave_core::export::to_json(g, Some(scene_pkgs), show_optional),
            )
            .map_err(|e| e.to_string()),
        };
        res.map(|()| path)
    }
}

// ---------------------------------------------------------------------------
// layout worker
// ---------------------------------------------------------------------------

fn spawn_layout(g: Arc<DepGraph>, seed: Vec<Option<Pos>>, gen: u64) {
    let gen_cell = current_gen();
    std::thread::spawn(move || {
        let edges: Vec<(usize, usize)> =
            g.edges(false).into_iter().map(|(a, b, _)| (a, b)).collect();
        let mut l = ForceLayout::new(g.len(), &edges, &seed);
        let mut last = Instant::now();
        while !l.converged() {
            if gen_cell.load(Ordering::SeqCst) != gen {
                return; // superseded by a reload
            }
            l.step();
            if last.elapsed() > Duration::from_millis(90) {
                post(Msg::Layout(gen, Arc::new(l.pos.clone()), false));
                last = Instant::now();
            }
        }
        if gen_cell.load(Ordering::SeqCst) != gen {
            return;
        }
        let positions = g
            .packages
            .iter()
            .zip(&l.pos)
            .map(|(p, &pos)| (p.name.clone(), pos))
            .collect();
        cache::save_layout(&LayoutCache {
            key: g.structure_hash(),
            converged: true,
            positions,
        });
        post(Msg::Layout(gen, Arc::new(l.pos), true));
    });
}

static GEN: OnceLock<Arc<AtomicU64>> = OnceLock::new();

fn current_gen() -> Arc<AtomicU64> {
    GEN.get_or_init(|| Arc::new(AtomicU64::new(0))).clone()
}

// ---------------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------------

pub fn run(db_path: Option<std::path::PathBuf>) -> Result<(), slint::PlatformError> {
    START.get_or_init(Instant::now);
    let ui = MainWindow::new()?;
    let render_tx = spawn_render_thread(ui.as_weak());
    let opts = LoadOptions {
        db_path,
        ..LoadOptions::default()
    };
    let (db_dir, _) = depweave_core::loader::resolve_db_path(&opts);

    let state = Rc::new(RefCell::new(State {
        ui: ui.as_weak(),
        render_tx,
        opts,
        graph: None,
        labels: Vec::new(),
        all_edges: Vec::new(),
        max_size: 0,
        positions: Arc::new(Vec::new()),
        layout_done: false,
        gen: current_gen(),
        scene: None,
        scene_pkgs: Vec::new(),
        pkg_scene: HashMap::new(),
        view: View {
            width: 900.0,
            height: 700.0,
            ..View::default()
        },
        scale: ui.window().scale_factor(),
        user_moved: false,
        drag: None,
        selected: None,
        hovered: None,
        matches: Vec::new(),
        focus: None,
        why_limit: 20,
        color_by: ColorBy::Reason,
        size_scale: true,
        show_optional: true,
        hierarchical: false,
        focus_mode: false,
        hops: 2,
        dir: Direction::Dependencies,
        dark: true,
        lists: Default::default(),
        impact_ready: false,
        _watcher: None,
        selftest: std::env::var_os("DEPWEAVE_SELFTEST").is_some(),
        snapshot_pending: false,
    }));
    APP.with(|a| *a.borrow_mut() = Some(state.clone()));

    macro_rules! on {
        ($setter:ident, |$st:ident $(, $arg:ident)*| $body:expr) => {{
            let s = state.clone();
            ui.$setter(move |$($arg),*| {
                // a callback fired while the state is already borrowed
                // (re-entrancy) is dropped instead of panicking
                let Ok(mut $st) = s.try_borrow_mut() else {
                    log::warn!("ignored re-entrant UI callback {}", stringify!($setter));
                    return Default::default();
                };
                #[allow(clippy::unused_unit)]
                { let _ = &mut $st; $body }
            });
        }};
    }
    on!(on_reload, |st| st.start_load());
    on!(on_search_edited, |st, t| st.search(t.as_str()));
    on!(on_search_accepted, |st| st.search_accept());
    on!(on_select_package, |st, name| {
        if let Some(i) = st.g().and_then(|g| g.find(name.as_str())) {
            if let Some(ui) = st.ui() {
                ui.set_search_results(to_model(&[]));
            }
            st.select(Some(i), true);
        }
    });
    on!(on_list_changed, |st, i| st.show_list_tab(i));
    on!(on_controls_changed, |st| st.controls_changed());
    on!(on_focus_cleared, |st| {
        st.focus = None;
        if let Some(ui) = st.ui() {
            ui.set_focus_mode(false);
        }
        st.controls_changed();
        st.rebuild_scene(true);
    });
    on!(on_fit, |st| {
        if let Some(s) = st.scene.clone() {
            st.view = s.fit_view(st.view.width, st.view.height);
            st.user_moved = false;
            st.request_render();
        }
    });
    on!(on_why_more, |st| {
        st.why_limit += 20;
        st.update_panels();
    });
    on!(on_export, |st, kind| st.do_export(kind.as_str()));
    on!(on_graph_resized, |st, w, h| st.resized(w, h));
    on!(on_graph_down, |st, x, y| {
        let p = st.phys(x, y);
        st.drag = Some(Drag {
            last: p,
            start: p,
            moved: false,
        });
    });
    on!(on_graph_up, |st, x, y| st.pointer_up(x, y));
    on!(on_graph_move, |st, x, y, pressed| st
        .pointer_move(x, y, pressed));
    on!(on_graph_scroll, |st, x, y, dy| st.scroll(x, y, dy));
    on!(on_graph_double, |st, x, y| st.double_click(x, y));

    {
        let mut st = state.borrow_mut();
        st.update_panels();
        st.read_controls();
        st._watcher = crate::watch::start(&db_dir.join("local"), || post(Msg::FsChanged));
        st.start_load();
    }
    ui.run()
}

// ---------------------------------------------------------------------------
// self-test scenario (DEPWEAVE_SELFTEST=1): exercises the UI callbacks end to
// end and writes snapshots to $DEPWEAVE_SNAPSHOT-<n>.png
// ---------------------------------------------------------------------------

fn selftest_snapshot(ui: &MainWindow, n: usize, what: &str) {
    if let Ok(base) = std::env::var("DEPWEAVE_SNAPSHOT") {
        if let Ok(buf) = ui.window().take_snapshot() {
            let (w, h) = (buf.width(), buf.height());
            if let Some(pm) =
                depweave_render::tiny_skia::PixmapRef::from_bytes(buf.as_bytes(), w, h)
            {
                let path = format!("{base}-{n}-{what}.png");
                let _ = pm.save_png(&path);
            }
        }
    }
}

fn selftest_env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn selftest_step(ui: MainWindow, n: usize) {
    // DEPWEAVE_SELFTEST_LOOPS=N repeats steps 0..=6 (search, focus, layouts,
    // exports, reload) N times and reports RSS after each cycle: leak check.
    const CYCLE: usize = 7;
    let loops = selftest_env("DEPWEAVE_SELFTEST_LOOPS", 1).max(1) as usize;
    let step_ms = selftest_env("DEPWEAVE_SELFTEST_STEP_MS", 900);
    let steps = CYCLE * loops + 1;
    let k = if n + 1 >= steps {
        usize::MAX
    } else {
        n % CYCLE
    };
    let what = match k {
        0 => {
            ui.invoke_search_edited("firefx".into());
            "search"
        }
        1 => {
            ui.invoke_select_package("firefox".into());
            ui.set_right_tab(1);
            "why"
        }
        2 => {
            ui.set_focus_mode(true);
            ui.set_hops(2);
            ui.invoke_controls_changed();
            "focus"
        }
        3 => {
            ui.set_layout_index(1);
            ui.invoke_controls_changed();
            "hier"
        }
        4 => {
            ui.invoke_graph_scroll(300.0, 300.0, 200.0);
            ui.invoke_graph_move(300.0, 300.0, false);
            ui.set_color_index(1);
            ui.set_dark(false);
            "light-repo"
        }
        5 => {
            ui.set_right_tab(0);
            ui.invoke_select_package("dbus".into());
            ui.set_list_tab(4);
            ui.invoke_list_changed(4);
            "details-impact"
        }
        6 => {
            for k in ["png", "svg", "dot", "json"] {
                ui.invoke_export(k.into());
            }
            ui.invoke_focus_cleared();
            ui.invoke_reload();
            "exported-reloading"
        }
        _ => "done",
    };
    let ui2 = ui.clone_strong();
    slint::Timer::single_shot(Duration::from_millis(step_ms), move || {
        if loops == 1 {
            selftest_snapshot(&ui2, n, what);
        }
        eprintln!("selftest: step {n} ({what}) ok at {} ms", elapsed_ms());
        if k == CYCLE - 1 {
            eprintln!("selftest: cycle {} rss {:.1} MB", n / CYCLE + 1, rss_mb());
        }
        if n + 1 >= steps {
            let _ = slint::quit_event_loop();
        } else {
            selftest_step(ui2, n + 1);
        }
    });
}
