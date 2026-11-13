//! Row builders for the sidebar lists, details panel and "Why" panel.
//! Pure functions over the core graph so they can be unit-tested.

use crate::Row;
use depweave_cli::format_date;
use depweave_core::dep::parse_optdep;
use depweave_core::graph::human_size;
use depweave_core::why::{removal_command, why};
use depweave_core::{DepGraph, Reason};
use slint::{ModelRc, SharedString, VecModel};
use std::collections::HashSet;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct R {
    pub kind: i32,
    pub a: String,
    pub b: String,
    pub link: String,
    pub segs: Vec<String>,
}

impl R {
    pub fn header(a: impl Into<String>) -> R {
        R {
            kind: 0,
            a: a.into(),
            ..R::default()
        }
    }
    pub fn kv(a: &str, b: impl Into<String>) -> R {
        R {
            kind: 1,
            a: a.into(),
            b: b.into(),
            ..R::default()
        }
    }
    pub fn link(a: &str, b: impl Into<String>) -> R {
        R {
            kind: 2,
            a: a.into(),
            b: b.into(),
            link: a.into(),
            ..R::default()
        }
    }
    pub fn text(a: impl Into<String>) -> R {
        R {
            kind: 3,
            a: a.into(),
            ..R::default()
        }
    }
    pub fn note(a: impl Into<String>) -> R {
        R {
            kind: 5,
            a: a.into(),
            ..R::default()
        }
    }
    pub fn warn(a: impl Into<String>) -> R {
        R {
            kind: 6,
            a: a.into(),
            ..R::default()
        }
    }
    pub fn chain(segs: Vec<String>) -> R {
        R {
            kind: 4,
            segs,
            ..R::default()
        }
    }
    pub fn to_row(&self) -> Row {
        Row {
            kind: self.kind,
            a: SharedString::from(self.a.as_str()),
            b: SharedString::from(self.b.as_str()),
            link: SharedString::from(self.link.as_str()),
            segs: ModelRc::new(VecModel::from(
                self.segs
                    .iter()
                    .map(|s| SharedString::from(s.as_str()))
                    .collect::<Vec<_>>(),
            )),
        }
    }
}

pub fn to_model(rows: &[R]) -> ModelRc<Row> {
    ModelRc::new(VecModel::from(
        rows.iter().map(R::to_row).collect::<Vec<_>>(),
    ))
}

/// Greedy word wrap to `width` characters (fixed-height list rows can't wrap).
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            lines.push(head);
        }
        if cur.is_empty() {
            cur = word;
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(&word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

const WRAP: usize = 46;

pub fn pkg_row(g: &DepGraph, i: usize) -> R {
    let p = &g.packages[i];
    R::link(&p.name, human_size(p.size))
}

pub fn details_rows(g: &DepGraph, i: usize) -> Vec<R> {
    let p = &g.packages[i];
    let mut v = Vec::new();
    for l in wrap(&p.description, WRAP) {
        v.push(R::text(l));
    }
    v.push(R::kv("Repository", p.repo_label()));
    v.push(R::kv(
        "Install reason",
        if p.reason == Reason::Explicit {
            "explicitly installed"
        } else {
            "installed as dependency"
        },
    ));
    v.push(R::kv("Installed size", human_size(p.size)));
    v.push(R::kv("Installed on", format_date(p.install_date)));
    v.push(R::kv("Built on", format_date(p.build_date)));
    v.push(R::kv("Architecture", &p.arch));
    if !p.url.is_empty() {
        v.push(R::kv("URL", &p.url));
    }
    if !p.packager.is_empty() {
        v.push(R::kv("Packager", &p.packager));
    }
    if g.is_orphan(i) {
        v.push(R::warn(
            "Orphan: installed as a dependency, nothing needs it",
        ));
    }

    // removal impact
    let imp = g.removal_impact(i);
    v.push(R::header("Removal impact (pacman -Rs)"));
    v.push(R::text(format!(
        "Frees {} in total",
        human_size(imp.freed_size)
    )));
    if !imp.blocked_by.is_empty() {
        v.push(R::warn(format!(
            "Breaks {} package(s): -Rs would refuse",
            imp.blocked_by.len()
        )));
    }
    if imp.members.is_empty() {
        v.push(R::note("No other packages would become orphans"));
    } else {
        v.push(R::text(format!(
            "Also removes {} package(s):",
            imp.members.len()
        )));
        let mut m = imp.members.clone();
        m.sort_by_key(|&k| std::cmp::Reverse(g.packages[k].size));
        for &k in m.iter().take(40) {
            v.push(pkg_row(g, k));
        }
        if m.len() > 40 {
            v.push(R::note(format!("… and {} more", m.len() - 40)));
        }
    }

    // what pacman resolves each dependency to (one package per entry)
    let deps: Vec<usize> = g.resolved_deps(i).collect();
    v.push(R::header(format!(
        "Depends on ({})",
        deps.len() + g.unresolved[i].len()
    )));
    for d in deps {
        v.push(pkg_row(g, d));
    }
    for u in &g.unresolved[i] {
        v.push(R::note(format!("not installed: {u}")));
    }

    let mut req: Vec<usize> = g.required_by(i).collect();
    req.sort_by(|&a, &b| g.packages[a].name.cmp(&g.packages[b].name));
    v.push(R::header(format!("Required by ({})", req.len())));
    for r in req {
        v.push(pkg_row(g, r));
    }

    if !p.optdepends.is_empty() {
        v.push(R::header(format!(
            "Optional dependencies ({})",
            p.optdepends.len()
        )));
        for line in &p.optdepends {
            let Some((spec, desc)) = parse_optdep(line) else {
                continue;
            };
            let desc = desc.unwrap_or_default();
            match g.find_or_provider(&spec.name) {
                Some(t) => v.push(R::link(&g.packages[t].name, desc)),
                None => v.push(R::note(format!("{} — {} (not installed)", spec.name, desc))),
            }
        }
    }
    let mut of: Vec<usize> = g.optional_for(i).collect();
    of.sort_by(|&a, &b| g.packages[a].name.cmp(&g.packages[b].name));
    if !of.is_empty() {
        v.push(R::header(format!("Optional for ({})", of.len())));
        for f in of {
            v.push(R::link(
                &g.packages[f].name,
                g.optdep_description(f, i).unwrap_or_default(),
            ));
        }
    }
    for (title, list) in [
        ("Provides", &p.provides),
        ("Conflicts with", &p.conflicts),
        ("Replaces", &p.replaces),
    ] {
        if !list.is_empty() {
            v.push(R::header(title));
            for l in wrap(&list.join("  "), WRAP) {
                v.push(R::text(l));
            }
        }
    }
    v
}

pub struct WhyRows {
    pub title: String,
    pub rows: Vec<R>,
    pub truncated: bool,
    /// Non-empty only for orphans: copy-only removal command.
    pub command: String,
}

pub fn why_rows(g: &DepGraph, i: usize, limit: usize) -> WhyRows {
    let p = &g.packages[i];
    let r = why(g, i, limit);
    let mut rows = Vec::new();
    let mut command = String::new();
    if r.target_is_explicit {
        rows.push(R::text("Explicitly installed"));
        rows.push(R::kv("Installed on", format_date(p.install_date)));
        let n = g.required_by(i).count();
        if n > 0 {
            rows.push(R::note(format!("Also required by {n} other package(s)")));
        }
    }
    if r.is_orphan {
        rows.push(R::warn("Orphan: nothing requires this package"));
        rows.push(R::text(format!("Removing it frees {}", human_size(p.size))));
        command = removal_command(g, i);
    }
    if !r.chains.is_empty() {
        let roots: HashSet<usize> = r.chains.iter().map(|c| c[0]).collect();
        rows.push(R::header(format!(
            "Pulled in by {}{} explicit package(s)",
            roots.len(),
            if r.truncated { "+" } else { "" }
        )));
        for c in &r.chains {
            rows.push(R::chain(
                c.iter().map(|&k| g.packages[k].name.clone()).collect(),
            ));
        }
        if r.truncated {
            rows.push(R::note("More chains exist — use “Show more”"));
        }
    }
    if !r.optional_for.is_empty() {
        rows.push(R::header("Optionally wanted by"));
        for (f, d) in &r.optional_for {
            rows.push(R::link(&g.packages[*f].name, d.clone().unwrap_or_default()));
        }
    }
    if rows.is_empty() {
        rows.push(R::note("No installation chain found"));
    }
    WhyRows {
        title: format!("Why is {} installed?", p.name),
        rows,
        truncated: r.truncated,
        command,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use depweave_core::Package;

    fn pkg(name: &str, reason: Reason, deps: &[&str]) -> Package {
        Package {
            name: name.into(),
            version: "1-1".into(),
            size: 2048,
            reason,
            depends: deps.iter().map(|s| s.to_string()).collect(),
            description: "A fairly long description that should wrap over several lines nicely"
                .into(),
            ..Package::default()
        }
    }

    #[test]
    fn wrapping() {
        let l = wrap("aaa bbb ccc", 7);
        assert_eq!(l, vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap("x".repeat(10).as_str(), 4), vec!["xxxx", "xxxx", "xx"]);
        assert!(wrap("", 5).is_empty());
    }

    #[test]
    fn why_for_orphan_and_chain() {
        let g = DepGraph::build(vec![
            pkg("app", Reason::Explicit, &["lib"]),
            pkg("lib", Reason::Dependency, &[]),
            pkg("old", Reason::Dependency, &[]),
        ]);
        let w = why_rows(&g, g.find("old").unwrap(), 10);
        assert_eq!(w.command, "sudo pacman -Rns old");
        assert!(w.rows.iter().any(|r| r.kind == 6));
        let w = why_rows(&g, g.find("lib").unwrap(), 10);
        assert!(w.command.is_empty());
        assert_eq!(
            w.rows.iter().find(|r| r.kind == 4).unwrap().segs,
            vec!["app", "lib"]
        );
    }

    #[test]
    fn details_has_sections() {
        let g = DepGraph::build(vec![
            pkg("app", Reason::Explicit, &["lib", "ghost"]),
            pkg("lib", Reason::Dependency, &[]),
        ]);
        let d = details_rows(&g, g.find("app").unwrap());
        assert!(d
            .iter()
            .any(|r| r.kind == 0 && r.a.starts_with("Depends on (2)")));
        assert!(d.iter().any(|r| r.kind == 2 && r.link == "lib"));
        assert!(d.iter().any(|r| r.a.contains("not installed: ghost")));
        assert!(d.iter().any(|r| r.a.contains("Also removes 1")));
    }

    #[test]
    fn details_lists_resolved_provider_once() {
        let mut a = pkg("font-a", Reason::Dependency, &[]);
        a.provides = vec!["ttf-font".into()];
        let mut b = pkg("font-b", Reason::Dependency, &[]);
        b.provides = vec!["ttf-font".into()];
        let g = DepGraph::build(vec![pkg("browser", Reason::Explicit, &["ttf-font"]), a, b]);
        let d = details_rows(&g, 0);
        let start = d
            .iter()
            .position(|r| r.kind == 0 && r.a == "Depends on (1)")
            .unwrap();
        let section: Vec<&str> = d[start + 1..]
            .iter()
            .take_while(|r| r.kind != 0)
            .map(|r| r.link.as_str())
            .collect();
        assert_eq!(section, vec!["font-a"]);
    }
}
