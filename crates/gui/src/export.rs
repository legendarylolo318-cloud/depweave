//! File export helpers (PNG/SVG/DOT/JSON) used by the GUI.

use std::path::PathBuf;

/// Where GUI exports go. depweave only ever writes below its own cache /
/// config directories, so the default is `~/.cache/depweave/exports`;
/// `DEPWEAVE_EXPORT_DIR` is an explicit user override.
pub fn export_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DEPWEAVE_EXPORT_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    depweave_core::cache::cache_dir().map(|d| d.join("exports"))
}

pub fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    format!(
        "{}-{h:02}{m:02}{s:02}",
        depweave_cli::format_date(secs).replace('-', "")
    )
}

pub fn target_path(ext: &str) -> std::io::Result<PathBuf> {
    let dir = export_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no HOME / XDG_CACHE_HOME")
    })?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("depweave-{}.{ext}", timestamp())))
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_dir_stays_in_cache() {
        if std::env::var_os("DEPWEAVE_EXPORT_DIR").is_none() {
            let d = super::export_dir().unwrap();
            assert!(d.ends_with("depweave/exports"), "{}", d.display());
        }
    }

    #[test]
    fn timestamp_shape() {
        let t = super::timestamp();
        assert_eq!(t.len(), 15);
        assert_eq!(&t[8..9], "-");
    }
}
