//! Non-blocking git-changes probe for the desktop's `Core`.
//!
//! `Core::on_agent_line` runs on the reader thread *while holding the shared
//! `Core` mutex*, and it calls `GitProbe::changes` to refresh a session's
//! working-tree changes. The production `RealGit` shells out to `git` — on a
//! slow filesystem (e.g. WSL2) that subprocess can take hundreds of
//! milliseconds, and because it runs under the lock it freezes every UI command
//! for that whole time → the intermittent whole-app hangs.
//!
//! `BackgroundGit` makes `changes()` return *instantly*: it serves the last
//! cached result for the project and hands the (slow) `git` call off to a single
//! background worker thread that refreshes the cache off the lock. The Changes
//! view is therefore eventually-consistent (it picks up the fresh result on the
//! next session event) but the UI never blocks on `git`.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use marsad_core::{GitProbe, RealGit};
use marsad_viewmodel::FileChange;

pub struct BackgroundGit {
    cache: Arc<Mutex<HashMap<String, Vec<FileChange>>>>,
    tx: Sender<String>,
}

impl BackgroundGit {
    pub fn new() -> Self {
        let cache: Arc<Mutex<HashMap<String, Vec<FileChange>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = channel::<String>();

        let cache_worker = Arc::clone(&cache);
        std::thread::spawn(move || {
            let real = RealGit;
            // Blocks until the sender is dropped (app shutdown).
            while let Ok(project) = rx.recv() {
                let fresh = real.changes(&project);
                if let Ok(mut c) = cache_worker.lock() {
                    c.insert(project, fresh);
                }
            }
        });

        BackgroundGit { cache, tx }
    }
}

impl Default for BackgroundGit {
    fn default() -> Self {
        Self::new()
    }
}

impl GitProbe for BackgroundGit {
    fn changes(&self, project: &str) -> Vec<FileChange> {
        // Request an off-lock refresh, then return whatever we have cached now.
        let _ = self.tx.send(project.to_string());
        self.cache
            .lock()
            .ok()
            .and_then(|c| c.get(project).cloned())
            .unwrap_or_default()
    }
}
