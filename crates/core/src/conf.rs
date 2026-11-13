//! Minimal /etc/pacman.conf reader: DBPath and repository order.

use std::path::{Path, PathBuf};

pub const DEFAULT_DBPATH: &str = "/var/lib/pacman";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacmanConf {
    pub db_path: Option<PathBuf>,
    /// `RootDir`; when set without `DBPath`, pacman uses `<RootDir>/var/lib/pacman`.
    pub root_dir: Option<PathBuf>,
    /// Repository section names in config order.
    pub repos: Vec<String>,
}

pub fn parse_conf(text: &str) -> PacmanConf {
    let mut conf = PacmanConf::default();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            if section != "options" && !conf.repos.contains(&section) {
                conf.repos.push(section.clone());
            }
            continue;
        }
        if section == "options" {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                if v.is_empty() {
                    continue;
                }
                match k.trim() {
                    "DBPath" => conf.db_path = Some(PathBuf::from(v)),
                    "RootDir" => conf.root_dir = Some(PathBuf::from(v)),
                    _ => {}
                }
            }
        }
    }
    conf
}

impl PacmanConf {
    /// The effective database directory, following pacman's rules.
    pub fn effective_db_path(&self) -> PathBuf {
        match (&self.db_path, &self.root_dir) {
            (Some(p), _) => p.clone(),
            (None, Some(root)) => root.join(DEFAULT_DBPATH.trim_start_matches('/')),
            (None, None) => PathBuf::from(DEFAULT_DBPATH),
        }
    }
}

/// Read a pacman.conf; returns defaults if it is missing or unreadable.
pub fn read_conf(path: &Path) -> PacmanConf {
    match std::fs::read_to_string(path) {
        Ok(t) => parse_conf(&t),
        Err(e) => {
            log::warn!("cannot read {}: {e}; using defaults", path.display());
            PacmanConf::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbpath_and_repos() {
        let c = parse_conf(
            "[options]\n#DBPath = /nope\nDBPath = /x/db/ # c\nHoldPkg = pacman\n[core]\nInclude = /m\n[extra]\n[custom]\nServer=file:///r\n",
        );
        assert_eq!(c.db_path, Some(PathBuf::from("/x/db/")));
        assert_eq!(c.repos, vec!["core", "extra", "custom"]);
    }

    #[test]
    fn rootdir_without_dbpath() {
        let c = parse_conf("[options]\nRootDir = /mnt\n[core]\n[core]\n");
        assert_eq!(c.effective_db_path(), PathBuf::from("/mnt/var/lib/pacman"));
        assert_eq!(c.repos, vec!["core"]);
        let c = parse_conf("[options]\nRootDir=/mnt\nDBPath=/db\n");
        assert_eq!(c.effective_db_path(), PathBuf::from("/db"));
        assert_eq!(
            parse_conf("").effective_db_path(),
            PathBuf::from("/var/lib/pacman")
        );
        // repo sections before [options] and inline comments
        let c = parse_conf("[custom] # mine\nServer = x\n[options]\nDBPath=/z#c\n");
        assert_eq!(c.repos, vec!["custom"]);
        assert_eq!(c.db_path, Some(PathBuf::from("/z")));
    }

    #[test]
    fn commented_dbpath_is_ignored() {
        assert_eq!(parse_conf("[options]\n#DBPath = /a\n").db_path, None);
    }
}
