//! Parser for pacman `desc` files (local db and sync db format).

use crate::error::{Error, Result};
use crate::graph::{Package, Reason};
use std::collections::HashMap;

/// Split a desc file into `%SECTION%` -> lines.
fn sections(text: &str) -> HashMap<&str, Vec<&str>> {
    let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut cur: Option<&str> = None;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            cur = None;
        } else if line.len() > 2 && line.starts_with('%') && line.ends_with('%') {
            let key = &line[1..line.len() - 1];
            map.entry(key).or_default();
            cur = Some(key);
        } else if let Some(k) = cur {
            if let Some(v) = map.get_mut(k) {
                v.push(line);
            }
        }
    }
    map
}

fn first<'a>(m: &HashMap<&str, Vec<&'a str>>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(|v| v.first().copied())
}

fn list(m: &HashMap<&str, Vec<&str>>, k: &str) -> Vec<String> {
    m.get(k)
        .map(|v| v.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

fn num<T: std::str::FromStr + Default>(m: &HashMap<&str, Vec<&str>>, k: &str, name: &str) -> T {
    match first(m, k) {
        None => T::default(),
        Some(s) => s.trim().parse().unwrap_or_else(|_| {
            log::warn!("{name}: bad numeric value for %{k}%: {s:?}");
            T::default()
        }),
    }
}

/// Parse a local-db `desc` file. Fails only if `%NAME%` or `%VERSION%` is missing.
pub fn parse_local_desc(text: &str) -> Result<Package> {
    let m = sections(text);
    let name = first(&m, "NAME")
        .ok_or_else(|| Error::Parse("desc without %NAME%".into()))?
        .to_string();
    let version = first(&m, "VERSION")
        .ok_or_else(|| Error::Parse(format!("{name}: desc without %VERSION%")))?
        .to_string();
    let reason = match first(&m, "REASON").map(str::trim) {
        Some("1") => Reason::Dependency,
        _ => Reason::Explicit,
    };
    Ok(Package {
        description: first(&m, "DESC").unwrap_or("").to_string(),
        url: first(&m, "URL").unwrap_or("").to_string(),
        arch: first(&m, "ARCH").unwrap_or("").to_string(),
        packager: first(&m, "PACKAGER").unwrap_or("").to_string(),
        build_date: num(&m, "BUILDDATE", &name),
        install_date: num(&m, "INSTALLDATE", &name),
        size: num(&m, "SIZE", &name),
        reason,
        depends: list(&m, "DEPENDS"),
        optdepends: list(&m, "OPTDEPENDS"),
        provides: list(&m, "PROVIDES"),
        conflicts: list(&m, "CONFLICTS"),
        replaces: list(&m, "REPLACES"),
        repo: None,
        name,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = "%NAME%\nfoo\n\n%VERSION%\n1.0-1\n\n%DESC%\nA thing\n\n%SIZE%\n1234\n\n%REASON%\n1\n\n%DEPENDS%\nbar>=1\nbaz\n\n%OPTDEPENDS%\nqux: extra\n\n%PROVIDES%\nlibfoo.so=1-64\n";

    #[test]
    fn parses_fields() {
        let p = parse_local_desc(DESC).unwrap();
        assert_eq!(p.name, "foo");
        assert_eq!(p.size, 1234);
        assert_eq!(p.reason, Reason::Dependency);
        assert_eq!(p.depends, vec!["bar>=1", "baz"]);
        assert_eq!(p.optdepends, vec!["qux: extra"]);
        assert_eq!(p.provides, vec!["libfoo.so=1-64"]);
    }

    #[test]
    fn missing_reason_is_explicit() {
        let p = parse_local_desc("%NAME%\na\n\n%VERSION%\n1-1\n").unwrap();
        assert_eq!(p.reason, Reason::Explicit);
    }

    #[test]
    fn malformed_is_error_not_panic() {
        assert!(parse_local_desc("").is_err());
        assert!(parse_local_desc("%VERSION%\n1\n").is_err());
        assert!(parse_local_desc("garbage\n\n%%\n%X").is_err());
        let p = parse_local_desc("%NAME%\na\n\n%VERSION%\n1\n\n%SIZE%\nnotanumber\n").unwrap();
        assert_eq!(p.size, 0);
    }
}
