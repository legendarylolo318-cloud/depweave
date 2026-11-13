//! Watches the pacman local DB and triggers a debounced reload.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// Start watching `dir`; `on_change` is called (on a helper thread) once the
/// directory has been quiet for 1.5 s after a burst of events.
pub fn start(dir: &Path, on_change: impl Fn() + Send + 'static) -> Option<RecommendedWatcher> {
    let (tx, rx) = mpsc::channel::<()>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if !ev.kind.is_access() {
                let _ = tx.send(());
            }
        }
    })
    .map_err(|e| log::warn!("file watcher unavailable: {e}"))
    .ok()?;
    if w.watch(dir, RecursiveMode::Recursive).is_err() {
        if let Err(e) = w.watch(dir, RecursiveMode::NonRecursive) {
            log::warn!("cannot watch {}: {e}", dir.display());
            return None;
        }
    }
    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            while rx.recv_timeout(Duration::from_millis(1500)).is_ok() {}
            on_change();
        }
    });
    Some(w)
}
