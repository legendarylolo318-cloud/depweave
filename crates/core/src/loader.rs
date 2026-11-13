//! Loads the pacman databases into a [`DepGraph`].

use crate::cache::{self, SyncCache, SyncEntry};
use crate::conf::read_conf;
use crate::desc::parse_local_desc;
use crate::error::{io_err, Error, Result};
use crate::graph::{DepGraph, Package};
use crate::syncdb::read_sync_names;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Override DBPath (else read from pacman.conf, else /var/lib/pacman).
    pub db_path: Option<PathBuf>,
    /// Override the pacman.conf location (default /etc/pacman.conf).
    pub conf_path: Option<PathBuf>,
    /// Skip the on-disk sync-name cache (used by tests).
    pub no_cache: bool,
}

pub fn resolve_db_path(opts: &LoadOptions) -> (PathBuf, Vec<String>) {
    let conf = read_conf(
        opts.conf_path
            .as_deref()
            .unwrap_or_else(|| Path::new("/etc/pacman.conf")),
    );
    let path = opts
        .db_path
        .clone()
        .unwrap_or_else(|| conf.effective_db_path());
    (path, conf.repos)
}

fn mtime_secs(p: &Path) -> (u64, u64) {
    std::fs::metadata(p)
        .ok()
        .map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            (t, m.len())
        })
        .unwrap_or((0, 0))
}

/// Parse every `local/*/desc`, in parallel. Malformed entries are skipped.
pub fn read_local(db: &Path) -> Result<(Vec<Package>, Vec<String>)> {
    let local = db.join("local");
    let rd = std::fs::read_dir(&local).map_err(|e| io_err(&local, e))?;
    let dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    let chunk = dirs.len().div_ceil(threads).max(1);
    let mut pkgs = Vec::with_capacity(dirs.len());
    let mut warns = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = dirs
            .chunks(chunk)
            .map(|c| {
                s.spawn(move || {
                    let mut ok = Vec::new();
                    let mut w = Vec::new();
                    for d in c {
                        let f = d.join("desc");
                        // lossy: one stray non-UTF-8 byte in a description
                        // must not make the whole package disappear
                        match std::fs::read(&f) {
                            Ok(b) => match parse_local_desc(&String::from_utf8_lossy(&b)) {
                                Ok(p) => ok.push(p),
                                Err(e) => w.push(format!("{}: {e}", f.display())),
                            },
                            Err(e) => w.push(format!("{}: {e}", f.display())),
                        }
                    }
                    (ok, w)
                })
            })
            .collect();
        for h in handles {
            match h.join() {
                Ok((p, w)) => {
                    pkgs.extend(p);
                    warns.extend(w);
                }
                Err(_) => warns.push("a parser thread panicked; some packages skipped".into()),
            }
        }
    });
    for w in &warns {
        log::warn!("{w}");
    }
    Ok((pkgs, warns))
}

/// Map package name -> repo (first repo in priority order wins). Only repos
/// configured in pacman.conf count, like `pacman -Qm`; stale `*.db` files of
/// removed repos are ignored. If no repo is configured (pacman.conf missing),
/// every `*.db` in `sync/` is used.
pub fn read_sync(
    db: &Path,
    conf_repos: &[String],
    use_cache: bool,
) -> (HashMap<String, String>, Vec<String>) {
    let sync = db.join("sync");
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(&sync)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "db"))
                .filter_map(|p| {
                    let n = p.file_stem()?.to_string_lossy().into_owned();
                    Some((n, p))
                })
                .filter(|(n, _)| conf_repos.is_empty() || conf_repos.contains(n))
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|(n, _)| {
        (
            conf_repos.iter().position(|r| r == n).unwrap_or(usize::MAX),
            n.clone(),
        )
    });
    let old: SyncCache = if use_cache {
        cache::load_sync_cache()
    } else {
        Default::default()
    };
    let mut warns = Vec::new();
    type SyncResult = (String, String, Option<SyncEntry>, Option<String>);
    let mut results: Vec<SyncResult> = Vec::new();
    std::thread::scope(|s| {
        let hs: Vec<_> = files
            .iter()
            .map(|(name, path)| {
                let key = path.to_string_lossy().into_owned();
                let cached = old.get(&key);
                s.spawn(move || {
                    let (mt, sz) = mtime_secs(path);
                    if let Some(c) = cached {
                        if c.mtime == mt && c.size == sz {
                            return (name.clone(), key, Some(c.clone()), None);
                        }
                    }
                    match read_sync_names(path) {
                        Ok(names) => (
                            name.clone(),
                            key,
                            Some(SyncEntry {
                                mtime: mt,
                                size: sz,
                                names,
                            }),
                            None,
                        ),
                        Err(e) => (name.clone(), key, None, Some(e.to_string())),
                    }
                })
            })
            .collect();
        for h in hs {
            if let Ok(r) = h.join() {
                results.push(r);
            }
        }
    });
    let mut map = HashMap::new();
    let mut new_cache = SyncCache::new();
    for (name, key, entry, warn) in results {
        if let Some(w) = warn {
            log::warn!("{w}");
            warns.push(w);
        }
        if let Some(e) = entry {
            for n in &e.names {
                map.entry(n.clone()).or_insert_with(|| name.clone());
            }
            new_cache.insert(key, e);
        }
    }
    if use_cache {
        cache::save_sync_cache(&new_cache);
    }
    (map, warns)
}

/// Load everything and build the graph.
pub fn load(opts: &LoadOptions) -> Result<DepGraph> {
    let (db, conf_repos) = resolve_db_path(opts);
    if !db.join("local").is_dir() {
        return Err(Error::Parse(format!(
            "{} is not a pacman database (no local/ directory)",
            db.display()
        )));
    }
    let use_cache = !opts.no_cache;
    let (sync, local) = std::thread::scope(|s| {
        let h = s.spawn(|| read_sync(&db, &conf_repos, use_cache));
        let local = read_local(&db);
        (h.join().unwrap_or_default(), local)
    });
    let (mut pkgs, mut warns) = local?;
    let (repo_map, sync_warns) = sync;
    warns.extend(sync_warns);
    for p in &mut pkgs {
        p.repo = repo_map.get(&p.name).cloned();
    }
    let mut g = DepGraph::build(pkgs);
    // keep repo legend in priority order
    let order: Vec<String> = conf_repos.to_vec();
    g.repos.sort_by_key(|r| {
        (
            order.iter().position(|x| x == r).unwrap_or(usize::MAX),
            r.clone(),
        )
    });
    g.warnings.extend(warns);
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    pub fn write_local(db: &Path, dir: &str, desc: &str) {
        let d = db.join("local").join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("desc"), desc).unwrap();
    }

    #[test]
    fn fixture_db_end_to_end() {
        let root = std::env::temp_dir().join(format!("depweave-fx-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_local(
            &root,
            "app-1.0-1",
            "%NAME%\napp\n\n%VERSION%\n1.0-1\n\n%DEPENDS%\nsh\nlibz.so=1-64\n",
        );
        write_local(
            &root,
            "bash-5-1",
            "%NAME%\nbash\n\n%VERSION%\n5-1\n\n%REASON%\n1\n\n%PROVIDES%\nsh\n",
        );
        write_local(
            &root,
            "zlib-1-1",
            "%NAME%\nzlib\n\n%VERSION%\n1-1\n\n%REASON%\n1\n\n%PROVIDES%\nlibz.so=1-64\n",
        );
        write_local(&root, "junk-1-1", "this is not a desc\n");
        write_local(
            &root,
            "dust-1-1",
            "%NAME%\ndust\n\n%VERSION%\n1-1\n\n%REASON%\n1\n",
        );
        fs::create_dir_all(root.join("sync")).unwrap();
        fs::write(
            root.join("sync/core.db"),
            crate::syncdb::make_tar_gz(&["bash-5-1", "zlib-1-1"]),
        )
        .unwrap();
        fs::write(root.join("sync/extra.db"), b"corrupt").unwrap();

        let g = load(&LoadOptions {
            db_path: Some(root.clone()),
            conf_path: Some(root.join("none.conf")),
            no_cache: true,
        })
        .unwrap();
        assert_eq!(g.len(), 4); // junk skipped
        assert_eq!(
            g.packages[g.find("bash").unwrap()].repo.as_deref(),
            Some("core")
        );
        assert!(g.packages[g.find("app").unwrap()].is_foreign());
        let app = g.find("app").unwrap();
        assert_eq!(g.depends_on(app).count(), 2);
        assert_eq!(g.orphans(), vec![g.find("dust").unwrap()]);
        assert!(g.warnings.iter().any(|w| w.contains("junk")));
        assert!(g.warnings.iter().any(|w| w.contains("extra.db")));
        let _ = fs::remove_dir_all(&root);
    }

    fn tmpdb(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("depweave-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn stale_sync_db_and_rootdir_and_odd_descs() {
        let root = tmpdb("stale");
        let db = root.join("var/lib/pacman");
        write_local(&db, "a-1-1", "%NAME%\na\n\n%VERSION%\n1-1\n");
        write_local(&db, "b-1-1", "%NAME%\nb\n\n%VERSION%\n1-1\n");
        // non-UTF-8 byte in the description must not drop the package
        let d = db.join("local/latin1-1-1");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("desc"),
            b"%NAME%\nlatin1\n\n%VERSION%\n1-1\n\n%DESC%\ncaf\xe9\n",
        )
        .unwrap();
        // package mid-install: directory exists, desc not written yet
        fs::create_dir_all(db.join("local/partial-1-1")).unwrap();
        // truncated desc (no %VERSION%) is skipped with a warning
        write_local(&db, "cut-1-1", "%NAME%\ncut\n\n%VERS");
        fs::write(db.join("local/ALPM_DB_VERSION"), "9\n").unwrap();
        fs::create_dir_all(db.join("sync")).unwrap();
        fs::write(
            db.join("sync/core.db"),
            crate::syncdb::make_tar_gz(&["a-1-1"]),
        )
        .unwrap();
        // repo removed from pacman.conf, db file left behind
        fs::write(
            db.join("sync/old.db"),
            crate::syncdb::make_tar_gz(&["b-1-1"]),
        )
        .unwrap();
        let conf = root.join("pacman.conf");
        fs::write(
            &conf,
            format!(
                "[options]\nRootDir = {}\n[core]\nInclude = /x\n",
                root.display()
            ),
        )
        .unwrap();
        let g = load(&LoadOptions {
            db_path: None,
            conf_path: Some(conf),
            no_cache: true,
        })
        .unwrap();
        let names: Vec<&str> = {
            let mut v: Vec<&str> = g.packages.iter().map(|p| p.name.as_str()).collect();
            v.sort();
            v
        };
        assert_eq!(names, vec!["a", "b", "latin1"]);
        assert_eq!(
            g.packages[g.find("a").unwrap()].repo.as_deref(),
            Some("core")
        );
        assert!(
            g.packages[g.find("b").unwrap()].is_foreign(),
            "stale repo ignored"
        );
        assert!(g.packages[g.find("latin1").unwrap()]
            .description
            .starts_with("caf"));
        assert!(g.warnings.iter().any(|w| w.contains("partial")));
        assert!(g.warnings.iter().any(|w| w.contains("cut")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_or_missing_sync_dir_means_all_foreign() {
        let root = tmpdb("nosync");
        write_local(&root, "a-1-1", "%NAME%\na\n\n%VERSION%\n1-1\n");
        let opts = LoadOptions {
            db_path: Some(root.clone()),
            conf_path: Some(root.join("none.conf")),
            no_cache: true,
        };
        let g = load(&opts).unwrap();
        assert!(g.packages[0].is_foreign());
        fs::create_dir_all(root.join("sync")).unwrap();
        let g = load(&opts).unwrap();
        assert!(g.packages[0].is_foreign());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_db_is_error() {
        let r = load(&LoadOptions {
            db_path: Some("/nonexistent-depweave".into()),
            conf_path: None,
            no_cache: true,
        });
        assert!(r.is_err());
    }
}
