//! Where Marsad keeps its files on a machine (ARCHITECTURE.md §4.2).
//!
//! The data root (`~/.marsad`; Windows `%LOCALAPPDATA%\Marsad`) may be a home directory that
//! several machines share (NFS and similar). So:
//!
//! ```text
//! <root>/bin/<os>-<arch>/marsad-agent      helper cache (shared; immutable per build+target)
//! <root>/machines/<key>/                   durable data of ONE machine:
//!     sessions/<id>/{session.json, journal.ndjson, blobs/, attachments/, shim.log, …}
//!     wasl/                                Wasl task journal
//!     store/                               hub settings (hosts, workstreams, …)
//!     marsad-agent.log                     daemon log
//! <run>/                                   machine-local runtime dir (0700, owner-checked):
//!     marsad.sock, marsad.sock.lock        daemon endpoint + single-instance lock
//!     sessions/<id>/{ctl.sock, shim.lock}  each shim's endpoint + liveness lock
//! ```
//!
//! `<key>` is [`wasit_runtime::machine::machine_key`]; `<run>` is
//! `<runtime base>/<8 hex of the root path>` with the base from
//! [`wasit_runtime::machine::runtime_dir`] (so two data roots on one machine — tests — never
//! share sockets). Overrides: `MARSAD_MACHINE_KEY`, `MARSAD_RUNTIME_DIR` (the base).

use std::io;
use std::path::{Path, PathBuf};

use wasit_runtime::machine;

/// Environment override of the machine key.
pub const MACHINE_KEY_ENV: &str = "MARSAD_MACHINE_KEY";
/// Environment override of the runtime base directory.
pub const RUNTIME_DIR_ENV: &str = "MARSAD_RUNTIME_DIR";

/// Resolved layout of one data root on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The (possibly shared) data root.
    pub root: PathBuf,
    /// This machine's key.
    pub machine: String,
    /// `<root>/machines/<key>`.
    pub data: PathBuf,
    /// Machine-local runtime directory.
    pub run: PathBuf,
}

impl Layout {
    /// Resolve (and create the runtime directory of) the layout for `root`. `run` overrides the
    /// runtime directory itself.
    pub fn resolve(root: impl Into<PathBuf>, run: Option<PathBuf>) -> io::Result<Layout> {
        let root = root.into();
        let key_env = std::env::var(MACHINE_KEY_ENV).ok();
        let machine = machine::machine_key(key_env.as_deref())?;
        let run = match run {
            Some(r) => {
                machine::ensure_private_dir(&r)?;
                r
            }
            None => {
                let base_env = std::env::var_os(RUNTIME_DIR_ENV)
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from);
                let base = machine::runtime_dir("marsad", base_env.as_deref())?;
                let r = base.join(root_tag(&root));
                machine::ensure_private_dir(&r)?;
                r
            }
        };
        Ok(Layout {
            data: root.join("machines").join(&machine),
            root,
            machine,
            run,
        })
    }

    /// The daemon endpoint path (on Windows the path its named pipe derives from).
    pub fn socket(&self) -> PathBuf {
        self.run.join("marsad.sock")
    }

    /// The daemon log.
    pub fn log(&self) -> PathBuf {
        self.data.join("marsad-agent.log")
    }

    /// The endpoint path of builds before the per-machine layout (`<root>/marsad.sock`).
    pub fn legacy_socket(&self) -> PathBuf {
        self.root.join("marsad.sock")
    }
}

/// 8 hex characters naming a data root inside the runtime base.
fn root_tag(root: &Path) -> String {
    let abs = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    machine::key_from_raw(&format!("root:{}", abs.display()))[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_per_machine_and_per_root() {
        let t = std::env::temp_dir().join(format!("marsad-layout-{}", std::process::id()));
        let a = Layout::resolve(t.join("a"), Some(t.join("run-a"))).unwrap();
        assert_eq!(a.data, t.join("a").join("machines").join(&a.machine));
        assert_eq!(a.socket(), t.join("run-a").join("marsad.sock"));
        assert_eq!(a.legacy_socket(), t.join("a").join("marsad.sock"));
        assert_ne!(root_tag(&t.join("a")), root_tag(&t.join("b")));
        assert_eq!(root_tag(&t.join("a")), root_tag(&t.join("a")));
        let _ = std::fs::remove_dir_all(&t);
    }
}
