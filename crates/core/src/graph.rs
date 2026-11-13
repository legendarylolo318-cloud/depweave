//! Dependency graph construction and analysis.

use crate::dep::{parse_dep, parse_optdep, Op};
use petgraph::graph::{DiGraph, NodeIndex};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum Reason {
    #[default]
    Explicit,
    Dependency,
}

#[derive(Debug, Clone, Default)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub description: String,
    pub url: String,
    pub arch: String,
    pub packager: String,
    pub build_date: i64,
    pub install_date: i64,
    /// Installed size in bytes.
    pub size: u64,
    pub reason: Reason,
    pub depends: Vec<String>,
    pub optdepends: Vec<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    /// Sync repo name; `None` = foreign (AUR / manual).
    pub repo: Option<String>,
}

impl Package {
    pub fn is_foreign(&self) -> bool {
        self.repo.is_none()
    }
    pub fn repo_label(&self) -> &str {
        self.repo.as_deref().unwrap_or("foreign")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum EdgeKind {
    /// `depends` (solid)
    Hard,
    /// installed `optdepends` (dashed)
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Follow what a package depends on.
    Dependencies,
    /// Follow what depends on a package.
    Dependents,
    Both,
}

/// Result of a recursive-removal simulation.
#[derive(Debug, Clone, Default)]
pub struct Impact {
    /// Packages that would additionally be removed (excluding the target).
    pub members: Vec<usize>,
    /// Bytes freed including the target itself.
    pub freed_size: u64,
    /// Installed packages left with an unsatisfied dependency (pacman would
    /// refuse the removal without -c / -dd).
    pub blocked_by: Vec<usize>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub total: usize,
    pub explicit: usize,
    pub dependencies: usize,
    pub foreign: usize,
    pub total_size: u64,
    pub orphans: usize,
    pub orphan_size: u64,
    pub edges_hard: usize,
    pub edges_optional: usize,
}

pub struct DepGraph {
    pub packages: Vec<Package>,
    index: HashMap<String, usize>,
    /// Node weight = package index (== NodeIndex index).
    pub graph: DiGraph<usize, EdgeKind>,
    hard_out: Vec<Vec<u32>>,
    hard_in: Vec<Vec<u32>>,
    opt_out: Vec<Vec<u32>>,
    opt_in: Vec<Vec<u32>>,
    /// Per package, per hard `depends` entry: every installed satisfier.
    dep_sats: Vec<Vec<Vec<u32>>>,
    /// Per package: the satisfier libalpm picks for each hard dep (a package
    /// whose name matches literally, else the first provider by name), deduped.
    resolved: Vec<Vec<u32>>,
    /// Package satisfies one of its own hard / optional deps. No self edge is
    /// stored, but libalpm counts it in required-by / optional-for, so such a
    /// package is never an orphan and never removed by `-Rs`.
    self_dep: Vec<bool>,
    self_optdep: Vec<bool>,
    /// Hard deps with no installed satisfier, per package.
    pub unresolved: Vec<Vec<String>>,
    /// Repo names in priority order (for colouring / legends).
    pub repos: Vec<String>,
    pub warnings: Vec<String>,
}

type Providers = HashMap<String, Vec<(u32, Option<String>)>>;

impl DepGraph {
    /// Build the graph from parsed local packages.
    pub fn build(packages: Vec<Package>) -> DepGraph {
        let mut warnings = Vec::new();
        let mut kept: Vec<Package> = Vec::with_capacity(packages.len());
        let mut index = HashMap::with_capacity(packages.len());
        for p in packages {
            if index.contains_key(&p.name) {
                warnings.push(format!("duplicate package {} skipped", p.name));
                continue;
            }
            index.insert(p.name.clone(), kept.len());
            kept.push(p);
        }
        let n = kept.len();

        let mut providers: Providers = HashMap::new();
        for (i, p) in kept.iter().enumerate() {
            providers
                .entry(p.name.clone())
                .or_default()
                .push((i as u32, Some(p.version.clone())));
            for prov in &p.provides {
                match parse_dep(prov) {
                    // libalpm only uses a provision's version when it is
                    // `name=ver`; `name>=ver` provides behave as unversioned.
                    Some(d) => providers.entry(d.name).or_default().push((
                        i as u32,
                        d.constraint.and_then(|(op, v)| (op == Op::Eq).then_some(v)),
                    )),
                    None => warnings.push(format!("{}: bad provides {prov:?}", p.name)),
                }
            }
        }

        let mut graph: DiGraph<usize, EdgeKind> = DiGraph::with_capacity(n, n * 6);
        for i in 0..n {
            graph.add_node(i);
        }
        let mut hard_out = vec![Vec::new(); n];
        let mut hard_in = vec![Vec::new(); n];
        let mut opt_out = vec![Vec::new(); n];
        let mut opt_in = vec![Vec::new(); n];
        let mut unresolved = vec![Vec::new(); n];
        let mut dep_sats: Vec<Vec<Vec<u32>>> = vec![Vec::new(); n];
        let mut resolved: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut self_dep = vec![false; n];
        let mut self_optdep = vec![false; n];

        let resolve = |spec: &crate::dep::DepSpec| -> Vec<u32> {
            providers
                .get(&spec.name)
                .map(|v| {
                    v.iter()
                        .filter(|(_, pv)| spec.satisfied_by(pv.as_deref()))
                        .map(|(i, _)| *i)
                        .collect()
                })
                .unwrap_or_default()
        };

        for (i, p) in kept.iter().enumerate() {
            for d in &p.depends {
                let Some(spec) = parse_dep(d) else {
                    warnings.push(format!("{}: bad depends {d:?}", p.name));
                    continue;
                };
                let targets = resolve(&spec);
                if targets.is_empty() {
                    unresolved[i].push(d.clone());
                }
                let chosen = targets
                    .iter()
                    .copied()
                    .find(|&t| kept[t as usize].name == spec.name)
                    .or_else(|| {
                        targets
                            .iter()
                            .copied()
                            .min_by(|&a, &b| kept[a as usize].name.cmp(&kept[b as usize].name))
                    });
                if let Some(c) = chosen {
                    if c as usize != i && !resolved[i].contains(&c) {
                        resolved[i].push(c);
                    }
                }
                dep_sats[i].push(targets.clone());
                if targets.contains(&(i as u32)) {
                    self_dep[i] = true;
                }
                for t in targets {
                    if t as usize != i && !hard_out[i].contains(&t) {
                        hard_out[i].push(t);
                        hard_in[t as usize].push(i as u32);
                        graph.add_edge(
                            NodeIndex::new(i),
                            NodeIndex::new(t as usize),
                            EdgeKind::Hard,
                        );
                    }
                }
            }
        }
        for (i, p) in kept.iter().enumerate() {
            for d in &p.optdepends {
                let Some((spec, _)) = parse_optdep(d) else {
                    warnings.push(format!("{}: bad optdepends {d:?}", p.name));
                    continue;
                };
                for t in resolve(&spec) {
                    let tu = t as usize;
                    if tu == i {
                        self_optdep[i] = true;
                    }
                    // a hard edge already says everything; skip the dashed duplicate
                    if tu != i && !hard_out[i].contains(&t) && !opt_out[i].contains(&t) {
                        opt_out[i].push(t);
                        opt_in[tu].push(i as u32);
                        graph.add_edge(NodeIndex::new(i), NodeIndex::new(tu), EdgeKind::Optional);
                    }
                }
            }
        }

        let mut repos: Vec<String> = Vec::new();
        for p in &kept {
            if let Some(r) = &p.repo {
                if !repos.contains(r) {
                    repos.push(r.clone());
                }
            }
        }
        for w in &warnings {
            log::warn!("{w}");
        }
        DepGraph {
            packages: kept,
            index,
            graph,
            hard_out,
            hard_in,
            opt_out,
            opt_in,
            dep_sats,
            resolved,
            self_dep,
            self_optdep,
            unresolved,
            repos,
            warnings,
        }
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
    pub fn find(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }

    /// Resolve a user-supplied name: exact package name, else a unique provider.
    pub fn find_or_provider(&self, name: &str) -> Option<usize> {
        if let Some(i) = self.find(name) {
            return Some(i);
        }
        let mut hit = None;
        for (i, p) in self.packages.iter().enumerate() {
            if p.provides
                .iter()
                .any(|s| parse_dep(s).is_some_and(|d| d.name == name))
            {
                if hit.is_some() {
                    return None;
                }
                hit = Some(i);
            }
        }
        hit
    }

    /// Packages `i` depends on (hard).
    pub fn depends_on(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.hard_out[i].iter().map(|&x| x as usize)
    }
    /// The package libalpm resolves each of `i`'s hard deps to (what
    /// `pactree -d1` and `pacman -Qi` show), in `depends` order.
    pub fn resolved_deps(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.resolved[i].iter().map(|&x| x as usize)
    }
    /// Packages that hard-depend on `i` (required-by).
    pub fn required_by(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.hard_in[i].iter().map(|&x| x as usize)
    }
    /// Does `i` (also) satisfy one of its own hard dependencies?
    pub fn requires_itself(&self, i: usize) -> bool {
        self.self_dep[i]
    }
    /// Does `i` satisfy one of its own optional dependencies?
    pub fn optionally_requires_itself(&self, i: usize) -> bool {
        self.self_optdep[i]
    }
    /// Installed packages `i` lists as optional.
    pub fn optional_deps(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.opt_out[i].iter().map(|&x| x as usize)
    }
    /// Packages that list `i` in optdepends.
    pub fn optional_for(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.opt_in[i].iter().map(|&x| x as usize)
    }

    /// The optdepends description `from` gives for `to`, if any.
    pub fn optdep_description(&self, from: usize, to: usize) -> Option<String> {
        let target = &self.packages[to];
        for line in &self.packages[from].optdepends {
            if let Some((spec, desc)) = parse_optdep(line) {
                if spec.name == target.name
                    || target
                        .provides
                        .iter()
                        .any(|p| parse_dep(p).is_some_and(|d| d.name == spec.name))
                {
                    return desc;
                }
            }
        }
        None
    }

    /// Orphans exactly as `pacman -Qdt`: installed as dependency, required by
    /// nothing and optionally required by nothing.
    pub fn is_orphan(&self, i: usize) -> bool {
        self.packages[i].reason == Reason::Dependency
            && self.hard_in[i].is_empty()
            && self.opt_in[i].is_empty()
            && !self.self_dep[i]
            && !self.self_optdep[i]
    }

    pub fn orphans(&self) -> Vec<usize> {
        (0..self.len()).filter(|&i| self.is_orphan(i)).collect()
    }

    /// Simulate `pacman -Rs <pkg>`: transitively drop dependency-reason
    /// packages that nothing outside the removal set requires.
    pub fn removal_impact(&self, target: usize) -> Impact {
        let mut removed: HashSet<usize> = HashSet::new();
        removed.insert(target);
        let mut members = Vec::new();
        let mut queue = VecDeque::from([target]);
        while let Some(cur) = queue.pop_front() {
            for d in self.depends_on(cur) {
                if removed.contains(&d)
                    || self.packages[d].reason != Reason::Dependency
                    || self.self_dep[d]
                {
                    continue;
                }
                if self.required_by(d).all(|r| removed.contains(&r)) {
                    removed.insert(d);
                    members.push(d);
                    queue.push_back(d);
                }
            }
        }
        let freed_size = removed.iter().map(|&i| self.packages[i].size).sum();
        // Like libalpm's checkdeps: a remaining package is broken only if one
        // of its deps loses *every* installed satisfier.
        let mut blocked_by: Vec<usize> = Vec::new();
        for &r in &removed {
            for o in self.required_by(r) {
                if removed.contains(&o) || blocked_by.contains(&o) {
                    continue;
                }
                let broken = self.dep_sats[o].iter().any(|sats| {
                    !sats.is_empty() && sats.iter().all(|&s| removed.contains(&(s as usize)))
                });
                if broken {
                    blocked_by.push(o);
                }
            }
        }
        blocked_by.sort_unstable();
        Impact {
            members,
            freed_size,
            blocked_by,
        }
    }

    /// (members, freed bytes incl. target) for every package. Used for the
    /// "largest removal impact" list.
    pub fn all_impacts(&self) -> Vec<(usize, u64)> {
        (0..self.len())
            .map(|i| {
                let imp = self.removal_impact(i);
                (imp.members.len(), imp.freed_size)
            })
            .collect()
    }

    /// BFS neighbourhood within `hops`, optionally following optional edges.
    /// The root is always first in the result.
    pub fn neighborhood(
        &self,
        root: usize,
        hops: usize,
        dir: Direction,
        include_optional: bool,
    ) -> Vec<usize> {
        let mut seen = HashSet::from([root]);
        let mut out = vec![root];
        let mut frontier = vec![root];
        for _ in 0..hops {
            let mut next = Vec::new();
            for &u in &frontier {
                let mut push = |v: usize| {
                    if seen.insert(v) {
                        out.push(v);
                        next.push(v);
                    }
                };
                if matches!(dir, Direction::Dependencies | Direction::Both) {
                    self.depends_on(u).for_each(&mut push);
                    if include_optional {
                        self.optional_deps(u).for_each(&mut push);
                    }
                }
                if matches!(dir, Direction::Dependents | Direction::Both) {
                    self.required_by(u).for_each(&mut push);
                    if include_optional {
                        self.optional_for(u).for_each(&mut push);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        out
    }

    /// All edges `(from, to, kind)`; optional edges only if requested.
    pub fn edges(&self, include_optional: bool) -> Vec<(usize, usize, EdgeKind)> {
        use petgraph::visit::EdgeRef;
        self.graph
            .edge_references()
            .filter(|e| include_optional || *e.weight() == EdgeKind::Hard)
            .map(|e| (e.source().index(), e.target().index(), *e.weight()))
            .collect()
    }

    pub fn stats(&self) -> Stats {
        let mut s = Stats {
            total: self.len(),
            ..Stats::default()
        };
        for (i, p) in self.packages.iter().enumerate() {
            s.total_size += p.size;
            match p.reason {
                Reason::Explicit => s.explicit += 1,
                Reason::Dependency => s.dependencies += 1,
            }
            if p.is_foreign() {
                s.foreign += 1;
            }
            if self.is_orphan(i) {
                s.orphans += 1;
                s.orphan_size += p.size;
            }
        }
        s.edges_hard = self.hard_out.iter().map(Vec::len).sum();
        s.edges_optional = self.opt_out.iter().map(Vec::len).sum();
        s
    }

    /// Stable hash of the package set + edges, used as layout cache key.
    pub fn structure_hash(&self) -> u64 {
        let mut h = crate::cache::Fnv::default();
        for (i, p) in self.packages.iter().enumerate() {
            h.write(p.name.as_bytes());
            h.write(p.version.as_bytes());
            h.write(&(self.hard_out[i].len() as u32).to_le_bytes());
        }
        h.0
    }
}

pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < U.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", U[u])
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn pkg(name: &str, reason: Reason, depends: &[&str]) -> Package {
        Package {
            name: name.into(),
            version: "1.0-1".into(),
            size: 1000,
            reason,
            depends: depends.iter().map(|s| s.to_string()).collect(),
            repo: Some("extra".into()),
            ..Package::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::pkg;
    use super::*;
    use Reason::*;

    fn names(g: &DepGraph, v: &[usize]) -> Vec<String> {
        let mut n: Vec<String> = v.iter().map(|&i| g.packages[i].name.clone()).collect();
        n.sort();
        n
    }

    #[test]
    fn constraints_stripped_and_edges_built() {
        let g = DepGraph::build(vec![
            pkg("app", Explicit, &["lib>=1.0", "libfoo.so=1-64"]),
            pkg("lib", Dependency, &[]),
            Package {
                provides: vec!["libfoo.so=1-64".into()],
                ..pkg("foo", Dependency, &[])
            },
        ]);
        let app = g.find("app").unwrap();
        assert_eq!(
            names(&g, &g.depends_on(app).collect::<Vec<_>>()),
            vec!["foo", "lib"]
        );
        assert!(g.unresolved[app].is_empty());
    }

    #[test]
    fn unsatisfied_constraint_is_unresolved() {
        let g = DepGraph::build(vec![
            pkg("a", Explicit, &["b>=5"]),
            pkg("b", Dependency, &[]),
        ]);
        assert_eq!(g.unresolved[0], vec!["b>=5"]);
        assert!(g.is_orphan(1));
    }

    #[test]
    fn virtual_provides() {
        let mut bash = pkg("bash", Explicit, &[]);
        bash.provides = vec!["sh".into()];
        let mut jdk = pkg("jdk17-openjdk", Dependency, &[]);
        jdk.provides = vec!["java-runtime=17".into(), "java-runtime-headless=17".into()];
        let g = DepGraph::build(vec![
            pkg("script", Explicit, &["sh", "java-runtime>=11"]),
            bash,
            jdk,
        ]);
        let s = g.find("script").unwrap();
        assert_eq!(
            names(&g, &g.depends_on(s).collect::<Vec<_>>()),
            vec!["bash", "jdk17-openjdk"]
        );
        // unversioned provision must not satisfy versioned dep
        let mut p = pkg("p", Dependency, &[]);
        p.provides = vec!["virt".into()];
        let g = DepGraph::build(vec![pkg("q", Explicit, &["virt>=1"]), p]);
        assert_eq!(g.unresolved[0].len(), 1);
    }

    #[test]
    fn orphan_detection_matches_qdt() {
        let mut opt_user = pkg("editor", Explicit, &[]);
        opt_user.optdepends = vec!["helper: nice to have".into()];
        let g = DepGraph::build(vec![
            pkg("app", Explicit, &["lib"]),
            pkg("lib", Dependency, &[]),
            pkg("lonely", Dependency, &[]),
            pkg("helper", Dependency, &[]),
            pkg("explicit-alone", Explicit, &[]),
            opt_user,
        ]);
        // helper is optionally required -> not an orphan under -Qdt
        assert_eq!(names(&g, &g.orphans()), vec!["lonely"]);
        let h = g.find("helper").unwrap();
        let e = g.find("editor").unwrap();
        assert_eq!(g.optdep_description(e, h).as_deref(), Some("nice to have"));
    }

    #[test]
    fn dependency_cycle_terminates() {
        let g = DepGraph::build(vec![
            pkg("root", Explicit, &["a"]),
            pkg("a", Dependency, &["b"]),
            pkg("b", Dependency, &["a", "c"]),
            pkg("c", Dependency, &["root"]),
        ]);
        // cycle members require each other: none are orphans
        assert!(g.orphans().is_empty());
        let imp = g.removal_impact(g.find("root").unwrap());
        // a is still required by b (outside removal set) -> nothing freed beyond root
        assert!(imp.members.is_empty());
        let n = g.neighborhood(0, 5, Direction::Both, false);
        assert_eq!(n.len(), 4);
    }

    #[test]
    fn removal_impact_recursive() {
        let g = DepGraph::build(vec![
            pkg("firefox", Explicit, &["gtk3", "nss"]),
            pkg("other", Explicit, &["nss"]),
            pkg("gtk3", Dependency, &["atk"]),
            pkg("atk", Dependency, &[]),
            pkg("nss", Dependency, &[]),
        ]);
        let imp = g.removal_impact(g.find("firefox").unwrap());
        assert_eq!(names(&g, &imp.members), vec!["atk", "gtk3"]); // nss kept: `other` needs it
        assert_eq!(imp.freed_size, 3000);
        let imp = g.removal_impact(g.find("gtk3").unwrap());
        assert_eq!(imp.blocked_by.len(), 1);
    }

    #[test]
    fn resolved_dep_prefers_literal_name_then_first_provider() {
        // systemd -> dbus-units: the package literally named dbus-units wins
        // over dbus-broker-units (which sorts first and also provides it)
        let mut broker = pkg("dbus-broker-units", Dependency, &[]);
        broker.provides = vec!["dbus-units".into()];
        let mut noto = pkg("noto-fonts", Dependency, &[]);
        noto.provides = vec!["ttf-font".into()];
        let mut dejavu = pkg("ttf-dejavu", Dependency, &[]);
        dejavu.provides = vec!["ttf-font".into()];
        let mut gnu = pkg("gnu-free-fonts", Dependency, &[]);
        gnu.provides = vec!["ttf-font".into()];
        let g = DepGraph::build(vec![
            pkg("systemd", Explicit, &["dbus-units", "ttf-font", "ttf-font"]),
            broker,
            pkg("dbus-units", Dependency, &[]),
            noto,
            dejavu,
            gnu,
        ]);
        let s = g.find("systemd").unwrap();
        let r: Vec<&str> = g
            .resolved_deps(s)
            .map(|i| g.packages[i].name.as_str())
            .collect();
        assert_eq!(r, vec!["dbus-units", "gnu-free-fonts"]);
        // the graph still knows every satisfier (pacman's "required by")
        assert_eq!(g.depends_on(s).count(), 5);
        for name in ["noto-fonts", "ttf-dejavu", "dbus-broker-units"] {
            assert!(!g.is_orphan(g.find(name).unwrap()), "{name}");
        }
    }

    #[test]
    fn non_eq_versioned_provision_is_unversioned() {
        // libalpm only honours `provides=name=ver`; `name>=ver` must not
        // satisfy a versioned dependency
        let mut p = pkg("p", Dependency, &[]);
        p.provides = vec!["virt>=5".into()];
        let g = DepGraph::build(vec![pkg("q", Explicit, &["virt>=1", "virt"]), p]);
        assert_eq!(g.unresolved[0], vec!["virt>=1"]);
        assert_eq!(g.depends_on(0).count(), 1);
    }

    #[test]
    fn removal_blocked_only_when_last_provider_goes() {
        // removing bash is fine for a `sh` dependent while dash also provides sh
        let mut bash = pkg("bash", Explicit, &[]);
        bash.provides = vec!["sh".into()];
        let mut dash = pkg("dash", Explicit, &[]);
        dash.provides = vec!["sh".into()];
        let g = DepGraph::build(vec![
            pkg("script", Explicit, &["sh"]),
            bash.clone(),
            dash,
            pkg("needs-bash", Explicit, &["bash"]),
        ]);
        let imp = g.removal_impact(g.find("bash").unwrap());
        assert_eq!(names(&g, &imp.blocked_by), vec!["needs-bash"]);
        let g = DepGraph::build(vec![pkg("script", Explicit, &["sh"]), bash]);
        assert_eq!(names(&g, &g.removal_impact(1).blocked_by), vec!["script"]);
    }

    #[test]
    fn removal_through_cycle_and_self_dependency() {
        // libalpm's can_remove_package: `a` is still needed by `b`, which is
        // not (yet) a target, so `pacman -Rs root` leaves the a <-> b cycle
        // installed. Must terminate and match that.
        let g = DepGraph::build(vec![
            pkg("root", Explicit, &["a"]),
            pkg("a", Dependency, &["b"]),
            pkg("b", Dependency, &["a"]),
        ]);
        let imp = g.removal_impact(0);
        assert!(imp.members.is_empty());
        assert!(imp.blocked_by.is_empty());
        assert_eq!(imp.freed_size, 1000);
        // a package that depends on (a provision of) itself is "required by"
        // itself in libalpm: not removable by -Rs and never an orphan
        let mut selfish = pkg("selfish", Dependency, &["virt"]);
        selfish.provides = vec!["virt".into()];
        let mut optself = pkg("optself", Dependency, &[]);
        optself.optdepends = vec!["optself: itself".into()];
        let g = DepGraph::build(vec![
            pkg("root", Explicit, &["selfish", "plain"]),
            selfish,
            pkg("plain", Dependency, &[]),
            optself,
        ]);
        assert_eq!(names(&g, &g.removal_impact(0).members), vec!["plain"]);
        assert!(g.requires_itself(1));
        assert!(g.optionally_requires_itself(3));
        assert!(g.orphans().is_empty());
        assert_eq!(g.depends_on(1).count(), 0);
    }

    #[test]
    fn large_cycle_is_iterative() {
        // 200k-node ring: recursion here would overflow the stack
        let n = 200_000;
        let mut v: Vec<Package> = (0..n)
            .map(|i| {
                let dep = format!("p{}", (i + 1) % n);
                pkg(&format!("p{i}"), Dependency, &[dep.as_str()])
            })
            .collect();
        v[0].reason = Explicit;
        let g = DepGraph::build(v);
        let imp = g.removal_impact(0);
        assert_eq!(imp.members.len(), n - 1);
        assert_eq!(g.neighborhood(0, n, Direction::Both, false).len(), n);
        assert!(g.orphans().is_empty());
    }

    #[test]
    fn explicit_deps_not_pulled_by_rs() {
        let g = DepGraph::build(vec![pkg("a", Explicit, &["b"]), pkg("b", Explicit, &[])]);
        assert!(g.removal_impact(0).members.is_empty());
    }

    #[test]
    fn stats_and_sizes() {
        let g = DepGraph::build(vec![
            pkg("a", Explicit, &["b"]),
            pkg("b", Dependency, &[]),
            pkg("c", Dependency, &[]),
        ]);
        let s = g.stats();
        assert_eq!(
            (s.total, s.explicit, s.dependencies, s.orphans),
            (3, 1, 2, 1)
        );
        assert_eq!(s.orphan_size, 1000);
        assert_eq!(human_size(1536), "1.5 KiB");
    }
}
