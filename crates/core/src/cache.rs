//! On-disk caches in ~/.cache/depweave (sync-db name lists and layout positions).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("depweave"))
}

fn write_atomic(name: &str, bytes: &[u8]) {
    let Some(dir) = cache_dir() else { return };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("cache dir: {e}");
        return;
    }
    // unique per process + thread: two instances (or two layout workers)
    // must never interleave writes into the same temp file
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!("{name}.{}.{seq}.tmp", std::process::id()));
    match std::fs::write(&tmp, bytes) {
        Ok(()) => {
            if let Err(e) = std::fs::rename(&tmp, dir.join(name)) {
                log::warn!("cache rename: {e}");
                let _ = std::fs::remove_file(&tmp);
            }
        }
        Err(e) => {
            log::warn!("cache write: {e}");
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn read(name: &str) -> Option<Vec<u8>> {
    std::fs::read(cache_dir()?.join(name)).ok()
}

// ---- sync db name cache ---------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct SyncEntry {
    pub mtime: u64,
    pub size: u64,
    pub names: Vec<String>,
}

pub type SyncCache = HashMap<String, SyncEntry>;

pub fn load_sync_cache() -> SyncCache {
    read("sync.json")
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_sync_cache(c: &SyncCache) {
    if let Ok(b) = serde_json::to_vec(c) {
        write_atomic("sync.json", &b);
    }
}

// ---- layout cache -----------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
pub struct LayoutCache {
    /// Hash of the package/edge set the positions were computed for.
    pub key: u64,
    /// True if the layout had converged when saved.
    pub converged: bool,
    pub positions: HashMap<String, [f32; 2]>,
}

pub fn load_layout() -> Option<LayoutCache> {
    serde_json::from_slice(&read("layout.json")?).ok()
}

pub fn save_layout(c: &LayoutCache) {
    if let Ok(b) = serde_json::to_vec(c) {
        write_atomic("layout.json", &b);
    }
}

/// FNV-1a 64-bit, stable across Rust versions.
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf29ce484222325)
    }
}

impl Fnv {
    pub fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}
