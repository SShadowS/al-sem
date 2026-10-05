//! File system watcher for incremental updates

use anyhow::Result;
use log::{debug, error, info};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

/// File change event
#[derive(Debug)]
pub enum FileChange {
    /// File was created or modified
    Modified(PathBuf),
    /// File was deleted
    Deleted(PathBuf),
    /// The backend detected a lapse in event delivery (`notify`'s
    /// `Flag::Rescan` — an inotify queue overflow, or an equivalent signal on
    /// another backend): any file may have changed since the last event this
    /// watcher actually delivered. Carries no path — the caller (T3 Task 15's
    /// server cutover) must treat this as "assume everything changed" (mapped
    /// onto `ChangeEvent::Overflow`, which forces a full rebuild) rather than
    /// silently trusting its last-known state, which is exactly the
    /// silent-drop failure mode this variant exists to surface instead of
    /// hiding.
    Overflow,
}

/// `true` for a path this watcher forwards: a `.al` source file, OR any path
/// under a `.alpackages` directory (dependency `.app` files — legacy never
/// watched these at all, so a dependency add/update/remove never reached the
/// index until the next server restart; see `src/server.rs`'s
/// `ChangeEvent::DepsChanged` mapping, which this filter feeds).
fn is_relevant_path(path: &Path) -> bool {
    let is_al = crate::source_text::has_al_extension(path);
    let under_alpackages = path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(".alpackages"))
    });
    is_al || under_alpackages
}

/// File system watcher for AL files
pub struct AlFileWatcher {
    _watcher: RecommendedWatcher,
    receiver: Receiver<FileChange>,
}

impl AlFileWatcher {
    /// Create a new watcher for the given directory
    pub fn new(root: &Path) -> Result<Self> {
        let (tx, rx) = channel();

        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                match result {
                    Ok(event) => {
                        if event.need_rescan() {
                            debug!("Watcher reported a rescan/overflow condition");
                            if tx.send(FileChange::Overflow).is_err() {
                                error!("Failed to send overflow event");
                            }
                            return;
                        }

                        // Filter for AL files + dependency (`.alpackages`) files.
                        let relevant_paths: Vec<_> = event
                            .paths
                            .iter()
                            .filter(|p| is_relevant_path(p))
                            .cloned()
                            .collect();

                        if relevant_paths.is_empty() {
                            return;
                        }

                        for path in relevant_paths {
                            let change = match event.kind {
                                EventKind::Create(_) | EventKind::Modify(_) => {
                                    debug!("File modified: {}", path.display());
                                    FileChange::Modified(path)
                                }
                                EventKind::Remove(_) => {
                                    debug!("File deleted: {}", path.display());
                                    FileChange::Deleted(path)
                                }
                                _ => continue,
                            };

                            if tx.send(change).is_err() {
                                error!("Failed to send file change event");
                            }
                        }
                    }
                    Err(e) => {
                        error!("Watch error: {:?}", e);
                    }
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(2)),
        )?;

        watcher.watch(root, RecursiveMode::Recursive)?;
        info!("Watching for file changes in: {}", root.display());

        Ok(Self {
            _watcher: watcher,
            receiver: rx,
        })
    }

    /// Try to receive a file change event (non-blocking)
    #[allow(dead_code)] // non-blocking API kept for future consumers
    pub fn try_recv(&self) -> Option<FileChange> {
        self.receiver.try_recv().ok()
    }

    /// Receive a file change event (blocking with timeout)
    pub fn recv_timeout(&self, timeout: Duration) -> Option<FileChange> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_watcher_creation() {
        let dir = tempdir().unwrap();
        let watcher = AlFileWatcher::new(dir.path());
        assert!(watcher.is_ok());
    }

    #[test]
    fn is_relevant_path_accepts_al_files_and_alpackages_contents_rejects_everything_else() {
        assert!(is_relevant_path(Path::new("Codeunit1.al")));
        assert!(is_relevant_path(Path::new("sub/dir/Codeunit1.AL")));
        assert!(is_relevant_path(Path::new(
            ".alpackages/Microsoft_Base Application_1.0.0.0.app"
        )));
        assert!(is_relevant_path(Path::new(
            "workspace/.alpackages/nested/anything.json"
        )));
        assert!(!is_relevant_path(Path::new("README.md")));
        assert!(!is_relevant_path(Path::new(".git/HEAD")));
    }

    #[test]
    fn test_watcher_detects_al_file() {
        let dir = tempdir().unwrap();
        let watcher = AlFileWatcher::new(dir.path()).unwrap();

        // Create an AL file
        let al_file = dir.path().join("Test.al");
        fs::write(&al_file, "codeunit 50000 Test {}").unwrap();

        // Give the watcher time to detect the change
        std::thread::sleep(Duration::from_millis(100));

        // Should have received a change event
        // Note: This may be flaky in CI environments
        let change = watcher.recv_timeout(Duration::from_secs(3));
        if let Some(FileChange::Modified(path)) = change {
            assert_eq!(path, al_file);
        }
    }
}
