//! Dependency / provides string parsing.

use crate::version::vercmp;
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ge,
    Le,
    Gt,
    Lt,
}

/// A parsed dependency: `name[op version]`. Sonames like `libfoo.so=1-64`
/// parse naturally as name `libfoo.so` with `=1-64`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepSpec {
    pub name: String,
    pub constraint: Option<(Op, String)>,
}

impl DepSpec {
    /// Does a provider with (optional) version satisfy this dependency?
    /// Mirrors libalpm: an unversioned provision never satisfies a versioned dep.
    pub fn satisfied_by(&self, provided_version: Option<&str>) -> bool {
        match (&self.constraint, provided_version) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some((op, want)), Some(have)) => {
                let o = vercmp(have, want);
                match op {
                    Op::Eq => o == Ordering::Equal,
                    Op::Ge => o != Ordering::Less,
                    Op::Le => o != Ordering::Greater,
                    Op::Gt => o == Ordering::Greater,
                    Op::Lt => o == Ordering::Less,
                }
            }
        }
    }
}

/// Parse `name`, `name>=1.2`, `libfoo.so=1-64`, ...  Returns `None` for empty input.
pub fn parse_dep(s: &str) -> Option<DepSpec> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let pos = s.find(['<', '>', '=']);
    let Some(p) = pos else {
        return Some(DepSpec {
            name: s.to_string(),
            constraint: None,
        });
    };
    let name = s[..p].trim();
    if name.is_empty() {
        return None;
    }
    let rest = &s[p..];
    let (op, len) = if rest.starts_with(">=") {
        (Op::Ge, 2)
    } else if rest.starts_with("<=") {
        (Op::Le, 2)
    } else if rest.starts_with('>') {
        (Op::Gt, 1)
    } else if rest.starts_with('<') {
        (Op::Lt, 1)
    } else {
        (Op::Eq, 1)
    };
    let ver = rest[len..].trim();
    Some(DepSpec {
        name: name.to_string(),
        constraint: if ver.is_empty() {
            None
        } else {
            Some((op, ver.to_string()))
        },
    })
}

/// Parse an optdepends entry: `name[constraint]: description`.
pub fn parse_optdep(s: &str) -> Option<(DepSpec, Option<String>)> {
    let (spec, desc) = match s.split_once(':') {
        // `: ` separates; a bare ':' could also be an epoch inside a version,
        // so require the following space (or end).
        Some(_) => match s.find(": ") {
            Some(i) => (&s[..i], Some(s[i + 2..].trim().to_string())),
            None => (s, None),
        },
        None => (s, None),
    };
    parse_dep(spec).map(|d| (d, desc.filter(|d| !d.is_empty())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_constraints() {
        assert_eq!(parse_dep("glibc").unwrap().constraint, None);
        let d = parse_dep("glibc>=2.38").unwrap();
        assert_eq!(d.name, "glibc");
        assert_eq!(d.constraint, Some((Op::Ge, "2.38".into())));
        assert_eq!(
            parse_dep("a<2").unwrap().constraint,
            Some((Op::Lt, "2".into()))
        );
        assert_eq!(
            parse_dep("a<=2").unwrap().constraint,
            Some((Op::Le, "2".into()))
        );
        assert_eq!(
            parse_dep("a>2").unwrap().constraint,
            Some((Op::Gt, "2".into()))
        );
        assert_eq!(
            parse_dep("a=2:1.0-1").unwrap().constraint,
            Some((Op::Eq, "2:1.0-1".into()))
        );
        assert!(parse_dep("").is_none());
    }

    #[test]
    fn soname() {
        let d = parse_dep("libfoo.so=1-64").unwrap();
        assert_eq!(d.name, "libfoo.so");
        assert!(d.satisfied_by(Some("1-64")));
        assert!(!d.satisfied_by(Some("2-64")));
        assert!(!d.satisfied_by(None));
    }

    #[test]
    fn satisfaction() {
        let d = parse_dep("x>=1.5").unwrap();
        assert!(d.satisfied_by(Some("1.5-1")));
        assert!(d.satisfied_by(Some("1.10-1")));
        assert!(!d.satisfied_by(Some("1.4-3")));
        assert!(parse_dep("x").unwrap().satisfied_by(None));
    }

    #[test]
    fn optdeps() {
        let (d, t) = parse_optdep("python: for scripting").unwrap();
        assert_eq!(d.name, "python");
        assert_eq!(t.as_deref(), Some("for scripting"));
        let (d, t) = parse_optdep("foo>=2: bar: baz").unwrap();
        assert_eq!(d.name, "foo");
        assert_eq!(t.as_deref(), Some("bar: baz"));
        let (d, t) = parse_optdep("bare").unwrap();
        assert_eq!((d.name.as_str(), t), ("bare", None));
    }
}
