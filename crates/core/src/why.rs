//! "Why is this installed?" chain enumeration.

use crate::graph::{DepGraph, Reason};
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct WhyResult {
    pub target: usize,
    pub target_is_explicit: bool,
    pub is_orphan: bool,
    /// Chains `explicit-root -> ... -> target`, shortest first, with the first
    /// chain of each distinct root ahead of alternative chains.
    pub chains: Vec<Vec<usize>>,
    /// True if enumeration hit the safety cap (more chains exist).
    pub truncated: bool,
    /// Packages that list the target as an optional dependency, with the reason text.
    pub optional_for: Vec<(usize, Option<String>)>,
}

const MAX_PARTIAL: usize = 200_000;

/// Enumerate up to `limit` chains from explicitly installed packages down to
/// `target`. Walking stops at the first explicit package on each path, so
/// intermediate nodes are always dependency-installed. Simple paths only, so
/// cycles cannot loop. BFS order gives shortest chains first.
pub fn why(g: &DepGraph, target: usize, limit: usize) -> WhyResult {
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let mut truncated = false;
    if g.packages[target].reason != Reason::Explicit {
        // partial path stored target-first: [target, ..., current head]
        let mut queue: VecDeque<Vec<usize>> = VecDeque::from([vec![target]]);
        let mut visited = 0usize;
        'outer: while let Some(path) = queue.pop_front() {
            let head = *path.last().unwrap_or(&target);
            for parent in g.required_by(head) {
                if path.contains(&parent) {
                    continue;
                }
                visited += 1;
                if visited > MAX_PARTIAL {
                    truncated = true;
                    break 'outer;
                }
                let mut np = path.clone();
                np.push(parent);
                if g.packages[parent].reason == Reason::Explicit {
                    np.reverse();
                    chains.push(np);
                    if chains.len() >= limit.saturating_mul(50).max(500) {
                        truncated = true;
                        break 'outer;
                    }
                } else {
                    queue.push_back(np);
                }
            }
        }
    }
    // roots-first ordering (stable => still shortest first within each group)
    let mut seen_roots = HashSet::new();
    let (mut primary, mut rest) = (Vec::new(), Vec::new());
    for c in chains {
        if seen_roots.insert(c[0]) {
            primary.push(c);
        } else {
            rest.push(c);
        }
    }
    primary.extend(rest);
    if primary.len() > limit {
        truncated = true;
        primary.truncate(limit);
    }
    let optional_for = g
        .optional_for(target)
        .map(|f| (f, g.optdep_description(f, target)))
        .collect();
    WhyResult {
        target,
        target_is_explicit: g.packages[target].reason == Reason::Explicit,
        is_orphan: g.is_orphan(target),
        chains: primary,
        truncated,
        optional_for,
    }
}

pub fn format_chain(g: &DepGraph, chain: &[usize]) -> String {
    chain
        .iter()
        .map(|&i| g.packages[i].name.as_str())
        .collect::<Vec<_>>()
        .join(" → ")
}

pub fn removal_command(g: &DepGraph, i: usize) -> String {
    format!("sudo pacman -Rns {}", g.packages[i].name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::testutil::pkg;
    use Reason::*;

    fn sample() -> DepGraph {
        DepGraph::build(vec![
            pkg("firefox", Explicit, &["gtk3"]),
            pkg("gtk3", Dependency, &["at-spi2-core"]),
            pkg("at-spi2-core", Dependency, &["dbus"]),
            pkg("dbus", Dependency, &[]),
            pkg("thunar", Explicit, &["gtk3", "dbus"]),
        ])
    }

    #[test]
    fn chains_shortest_first() {
        let g = sample();
        let r = why(&g, g.find("dbus").unwrap(), 10);
        let s: Vec<String> = r.chains.iter().map(|c| format_chain(&g, c)).collect();
        assert_eq!(s[0], "thunar → dbus");
        assert!(s.contains(&"firefox → gtk3 → at-spi2-core → dbus".to_string()));
        assert!(s.contains(&"thunar → gtk3 → at-spi2-core → dbus".to_string()));
        assert_eq!(s.len(), 3);
        assert!(!r.truncated && !r.is_orphan);
    }

    #[test]
    fn limit_truncates() {
        let g = sample();
        let r = why(&g, g.find("dbus").unwrap(), 2);
        assert_eq!(r.chains.len(), 2);
        assert!(r.truncated);
        // both distinct roots come first
        assert_ne!(r.chains[0][0], r.chains[1][0]);
    }

    #[test]
    fn explicit_and_orphan() {
        let g = DepGraph::build(vec![pkg("a", Explicit, &[]), pkg("o", Dependency, &[])]);
        let r = why(&g, 0, 5);
        assert!(r.target_is_explicit && r.chains.is_empty());
        let r = why(&g, 1, 5);
        assert!(r.is_orphan && r.chains.is_empty());
        assert_eq!(removal_command(&g, 1), "sudo pacman -Rns o");
    }

    #[test]
    fn cycles_do_not_hang() {
        let g = DepGraph::build(vec![
            pkg("root", Explicit, &["a"]),
            pkg("a", Dependency, &["b"]),
            pkg("b", Dependency, &["a", "c"]),
            pkg("c", Dependency, &["b"]),
        ]);
        let r = why(&g, g.find("c").unwrap(), 10);
        assert_eq!(r.chains.len(), 1);
        assert_eq!(format_chain(&g, &r.chains[0]), "root → a → b → c");
    }

    #[test]
    fn optional_pull() {
        let mut e = pkg("editor", Explicit, &[]);
        e.optdepends = vec!["helper: spellcheck".into()];
        let g = DepGraph::build(vec![e, pkg("helper", Dependency, &[])]);
        let r = why(&g, 1, 5);
        assert!(r.chains.is_empty() && !r.is_orphan);
        assert_eq!(r.optional_for, vec![(0, Some("spellcheck".to_string()))]);
    }
}
