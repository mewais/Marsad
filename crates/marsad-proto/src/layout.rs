//! Where Marsad keeps its files on a machine (ARCHITECTURE.md §4.2).
//!
//! The data root (`~/.marsad`; Windows `%LOCALAPPDATA%\Marsad`) may be a home directory that
//! several machines share (NFS and similar). So:
//!
//! ```text
//! <root>/bin/<build>/<os>-<arch>/marsad-agent   helper cache (shared; immutable per build+target;
//!                                               a session's MCP servers run from the helper of
//!                                               the daemon that launched it, see boot.rs)
//! <root>/machines/<key>/                   durable data of ONE machine:
//!     sessions/<id>/{session.json, journal.ndjson, blobs/, attachments/, shim.log, …}
//!     wasl/                                Wasl task journal
//!     store/                               hub settings (hosts, workstreams, …)
//!     diag/                                diagnostics (private: 0700 / 0600):
//!         daemon.ndjson[.1-.3]                 the daemon's records, after it holds the lock
//!         aux.ndjson[.1]                       short-lived processes' records (bridge, stop,
//!                                              askpass, a daemon that lost the lock race)
//!     marsad-agent.log                     the daemon's stderr: raw crash output only
//! <run>/                                   machine-local runtime dir (0700, owner-checked):
//!     marsad.sock, marsad.sock.lock        daemon endpoint + single-instance lock
//!     sessions/<id>/{ctl.sock, shim.lock}  each shim's endpoint + liveness lock
//!     ssh/<16 hex>, ssh/<16 hex>.log       hub only: the control socket of a host's shared ssh
//!                                          connection (it outlives the daemon) and its log
//! ```
//!
//! `<key>` is [`wasit_runtime::machine::machine_key`]; `<run>` is
//! `<runtime base>/<8 hex of the root path>` with the base from
//! [`wasit_runtime::machine::runtime_dir`] (so two data roots on one machine — tests — never
//! share sockets). Overrides: `MARSAD_MACHINE_KEY`, `MARSAD_RUNTIME_DIR` (the base).

use std::io;
use std::path::{Path, PathBuf};

use wasit_runtime::machine;

/// The daemon's diagnostics file in [`Layout::diag`] (rotated to `.1` … `.3`).
pub const DIAG_DAEMON: &str = "daemon.ndjson";
/// The file short-lived processes of a machine append their diagnostics to, in
/// [`Layout::diag`] (rotated to `.1`).
pub const DIAG_AUX: &str = "aux.ndjson";
/// The daemon's stderr file is cut to nothing, when a starter opens it, if it is larger.
pub const LOG_MAX_BYTES: u64 = 1 << 20;

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

/// This machine's key: the environment override, else the machine's own identity (see
/// `wasit_runtime::machine::machine_key`).
pub fn machine_key() -> io::Result<String> {
    let key_env = std::env::var(MACHINE_KEY_ENV).ok();
    machine::machine_key(key_env.as_deref())
}

impl Layout {
    /// Resolve (and create the runtime directory of) the layout for `root`. `run` overrides the
    /// runtime directory itself.
    pub fn resolve(root: impl Into<PathBuf>, run: Option<PathBuf>) -> io::Result<Layout> {
        Self::resolve_with_machine(root, run, machine_key()?)
    }

    /// [`Layout::resolve`] for a machine key already known (see [`machine_key`]). Asking the
    /// machine for its key runs processes on some platforms; a caller that resolves often keeps
    /// the key and calls this, which still checks and creates the runtime directory each time.
    pub fn resolve_with_machine(
        root: impl Into<PathBuf>,
        run: Option<PathBuf>,
        machine: String,
    ) -> io::Result<Layout> {
        let root = root.into();
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

    /// Where the hub keeps the control sockets (and their logs) of its shared ssh connections:
    /// `<run>/ssh`, one socket per ssh host, named by a hash of the host's id and route.
    pub fn ssh_dir(&self) -> PathBuf {
        self.run.join("ssh")
    }

    /// The daemon's stderr file: raw crash output (and `MARSAD_LOG` developer text), never
    /// diagnostics records and never exported. Opened by whoever starts a daemon, with
    /// [`Layout::open_log`].
    pub fn log(&self) -> PathBuf {
        self.data.join("marsad-agent.log")
    }

    /// The diagnostics directory (`0700`; its files `0600` on Unix; created by the sink's
    /// writer).
    pub fn diag(&self) -> PathBuf {
        self.data.join("diag")
    }

    /// Opens [`Layout::log`] for a daemon's stderr: append, private to the owner (`0600` on
    /// Unix; an older, wider file is tightened), and cut to nothing first when it is larger
    /// than [`LOG_MAX_BYTES`]. The one definition both starters of a daemon (the GUI client
    /// and the bridge) use. Blocking: call it from the blocking pool.
    pub fn open_log(&self) -> io::Result<std::fs::File> {
        let path = self.log();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
            // A handle of its own: an append-only one may not set the file's length (Windows).
            drop(wasit_diag::private::open(&path, |o| {
                o.write(true).truncate(true);
            })?);
        }
        wasit_diag::private::open(&path, |o| {
            o.append(true);
        })
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
    fn the_stderr_file_is_private_appended_and_cut_when_large() {
        let t = std::env::temp_dir().join(format!("marsad-log-{}", std::process::id()));
        let l = Layout::resolve(t.join("a"), Some(t.join("run-a"))).unwrap();
        assert_eq!(l.diag(), l.data.join("diag"));
        let mut f = l.open_log().unwrap();
        std::io::Write::write_all(&mut f, b"first\n").unwrap();
        drop(f);
        let mut f = l.open_log().unwrap();
        std::io::Write::write_all(&mut f, b"second\n").unwrap();
        drop(f);
        assert_eq!(std::fs::read_to_string(l.log()).unwrap(), "first\nsecond\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(l.log()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Over the limit: cut to nothing when opened.
        std::fs::write(l.log(), vec![b'x'; LOG_MAX_BYTES as usize + 1]).unwrap();
        drop(l.open_log().unwrap());
        assert_eq!(std::fs::metadata(l.log()).unwrap().len(), 0);
        // At the limit: kept.
        std::fs::write(l.log(), vec![b'x'; LOG_MAX_BYTES as usize]).unwrap();
        drop(l.open_log().unwrap());
        assert_eq!(std::fs::metadata(l.log()).unwrap().len(), LOG_MAX_BYTES);
        let _ = std::fs::remove_dir_all(&t);
    }

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
