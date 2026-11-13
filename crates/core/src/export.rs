//! Graphviz DOT and JSON export.

use crate::graph::{DepGraph, EdgeKind, Reason};
use serde_json::json;
use std::collections::HashSet;

fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn selected(g: &DepGraph, nodes: Option<&[usize]>) -> Vec<usize> {
    match nodes {
        Some(n) => n.to_vec(),
        None => (0..g.len()).collect(),
    }
}

fn sub_edges(g: &DepGraph, set: &HashSet<usize>, opt: bool) -> Vec<(usize, usize, EdgeKind)> {
    g.edges(opt)
        .into_iter()
        .filter(|(a, b, _)| set.contains(a) && set.contains(b))
        .collect()
}

pub fn to_dot(g: &DepGraph, nodes: Option<&[usize]>, include_optional: bool) -> String {
    let nodes = selected(g, nodes);
    let set: HashSet<usize> = nodes.iter().copied().collect();
    let mut s = String::from(
        "digraph depweave {\n  rankdir=LR;\n  node [shape=box, fontname=\"sans-serif\"];\n",
    );
    for &i in &nodes {
        let p = &g.packages[i];
        let style = if p.reason == Reason::Explicit {
            ", style=bold"
        } else {
            ""
        };
        s.push_str(&format!(
            "  \"{}\" [tooltip=\"{} ({})\"{}];\n",
            dot_escape(&p.name),
            dot_escape(&p.version),
            p.repo_label(),
            style
        ));
    }
    for (a, b, k) in sub_edges(g, &set, include_optional) {
        let attr = if k == EdgeKind::Optional {
            " [style=dashed]"
        } else {
            ""
        };
        s.push_str(&format!(
            "  \"{}\" -> \"{}\"{};\n",
            dot_escape(&g.packages[a].name),
            dot_escape(&g.packages[b].name),
            attr
        ));
    }
    s.push_str("}\n");
    s
}

pub fn to_json(g: &DepGraph, nodes: Option<&[usize]>, include_optional: bool) -> String {
    let nodes = selected(g, nodes);
    let set: HashSet<usize> = nodes.iter().copied().collect();
    let nj: Vec<_> = nodes
        .iter()
        .map(|&i| {
            let p = &g.packages[i];
            json!({
                "name": p.name, "version": p.version, "description": p.description,
                "size": p.size, "repo": p.repo_label(),
                "reason": if p.reason == Reason::Explicit { "explicit" } else { "dependency" },
                "orphan": g.is_orphan(i),
            })
        })
        .collect();
    let ej: Vec<_> = sub_edges(g, &set, include_optional)
        .into_iter()
        .map(|(a, b, k)| {
            json!({
                "from": g.packages[a].name, "to": g.packages[b].name,
                "kind": if k == EdgeKind::Hard { "depends" } else { "optdepends" },
            })
        })
        .collect();
    serde_json::to_string_pretty(&json!({ "nodes": nj, "edges": ej })).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::testutil::pkg;
    use Reason::*;

    #[test]
    fn dot_and_json_subgraph() {
        let mut a = pkg("a\"q", Explicit, &["b"]);
        a.optdepends = vec!["c: x".into()];
        let g = DepGraph::build(vec![
            a,
            pkg("b", Dependency, &[]),
            pkg("c", Dependency, &[]),
        ]);
        let dot = to_dot(&g, None, true);
        assert!(dot.contains("\"a\\\"q\" -> \"b\";"));
        assert!(dot.contains("[style=dashed]"));
        let sub = [0usize, 1];
        let dot = to_dot(&g, Some(&sub), true);
        assert!(!dot.contains("\"c\""));
        let j: serde_json::Value = serde_json::from_str(&to_json(&g, Some(&sub), true)).unwrap();
        assert_eq!(j["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(j["edges"].as_array().unwrap().len(), 1);
    }
}
