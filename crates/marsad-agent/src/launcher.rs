//! The host's stable helper pointer: `<home>/.marsad/bin/marsad-agent`.
//!
//! A session's MCP servers (`marsad-agent mcp`, `ask-mcp`) are rendered at launch with a helper
//! path, and the spec is stored in the session record: a continued session replays that spec.
//! With the helper's per-build path stored there, a continued session would keep running an old
//! helper after an upgrade. So sessions name this pointer instead, and the bootstrap points it at
//! the build it runs ([`publish`], POSIX only: a symlink is replaced by renaming a new one over
//! it, which is atomic, and a copy would go stale). On Windows nothing is published: a running
//! `.exe` cannot be replaced safely, so sessions keep the per-build path there. That limitation
//! stays until the Windows helper can be replaced while it runs.
//!
//! The daemon reports the pointer as its `launcher` only when the pointer resolves to the daemon's
//! own executable; otherwise the hub falls back to the daemon's per-build `exe`.

use std::path::{Path, PathBuf};

/// The pointer's file name, directly under the account's `bin` directory.
pub const POINTER: &str = "marsad-agent";

/// The helper directories of the account: `<bin>/<build>/<os>-<arch>/<exe>`. Returns the
/// platform directory, the build directory and `bin`, or `None` when `exe` is not in that shape
/// (a development or test build run from elsewhere: nothing is published or cleaned there).
pub fn helper_bin(exe: &Path) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let platform = exe.parent()?;
    let build = platform.parent()?;
    let bin = build.parent()?;
    let named = |p: &Path, n: &str| p.file_name().and_then(|f| f.to_str()) == Some(n);
    let home = bin.parent()?;
    (named(bin, "bin") && (named(home, ".marsad") || named(home, "Marsad"))).then(|| {
        (
            platform.to_path_buf(),
            build.to_path_buf(),
            bin.to_path_buf(),
        )
    })
}

/// The pointer's path for the helper `exe`, when `exe` is in the helper layout.
pub fn pointer_of(exe: &Path) -> Option<PathBuf> {
    let (_, _, bin) = helper_bin(exe)?;
    Some(bin.join(POINTER))
}

/// Points the stable pointer at `exe` (the running build's helper). Returns the pointer's path,
/// or `Ok(None)` when `exe` is not in the helper layout or the host is not POSIX.
///
/// The symlink is created under a temporary name beside the pointer and renamed over it: the
/// pointer always exists, either as the old link or the new one. Nothing is retried; a failure
/// leaves the old pointer in place and removes the temporary link.
#[cfg(unix)]
pub fn publish(exe: &Path) -> Result<Option<PathBuf>, String> {
    let Some(pointer) = pointer_of(exe) else {
        return Ok(None);
    };
    let tmp = pointer.with_file_name(format!(".{POINTER}.tmp.{}", std::process::id()));
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", tmp.display())),
    }
    std::os::unix::fs::symlink(exe, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, &pointer) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", pointer.display()));
    }
    Ok(Some(pointer))
}

/// Windows keeps the per-build path (see the module's note): nothing is published.
#[cfg(not(unix))]
pub fn publish(_exe: &Path) -> Result<Option<PathBuf>, String> {
    Ok(None)
}

/// The pointer's path when the pointer exists and resolves to `exe` itself: what the daemon
/// reports as its `launcher`. `None` for another build's pointer, a missing pointer, a daemon
/// outside the helper layout, and on Windows.
pub fn launcher_of(exe: &Path) -> Option<PathBuf> {
    if !cfg!(unix) {
        return None;
    }
    let pointer = pointer_of(exe)?;
    let resolves_here = std::fs::canonicalize(&pointer).ok()? == std::fs::canonicalize(exe).ok()?;
    resolves_here.then_some(pointer)
}

/// The name of the build directory (directly under `bin`) that the pointer targets, when the
/// pointer is a link into `bin`. Cleanup keeps that build.
pub fn pointer_build(bin: &Path) -> Option<String> {
    let target = std::fs::read_link(bin.join(POINTER)).ok()?;
    let rel = target.strip_prefix(bin).ok()?;
    rel.components()
        .next()?
        .as_os_str()
        .to_str()
        .map(str::to_owned)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A home (named `.marsad`) with builds `names`, each holding a helper file. Returns the
    /// home's root (removed by the caller) and each build's helper, in order.
    fn home_with_builds(tag: &str, names: &[&str]) -> (PathBuf, Vec<PathBuf>) {
        let root = std::env::temp_dir().join(format!("mrs-launch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let helpers = names
            .iter()
            .map(|n| {
                let p = root
                    .join(".marsad")
                    .join("bin")
                    .join(n)
                    .join("Linux-x86_64")
                    .join("marsad-agent");
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, b"helper").unwrap();
                p
            })
            .collect();
        (root, helpers)
    }

    fn bin_of(root: &Path) -> PathBuf {
        root.join(".marsad").join("bin")
    }

    #[test]
    fn install_creates_the_pointer_as_a_link_to_the_build() {
        let (root, h) = home_with_builds("install", &["b1"]);
        let pointer = publish(&h[0]).unwrap().expect("in the layout");
        assert_eq!(pointer, bin_of(&root).join(POINTER));
        let meta = std::fs::symlink_metadata(&pointer).unwrap();
        assert!(meta.file_type().is_symlink(), "a link, never a copy");
        assert_eq!(std::fs::read_link(&pointer).unwrap(), h[0]);
        assert_eq!(launcher_of(&h[0]), Some(pointer));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_after_an_upgrade_repoints_the_pointer_and_leaves_no_temporary() {
        let (root, h) = home_with_builds("upgrade", &["old", "new"]);
        publish(&h[0]).unwrap();
        assert_eq!(
            std::fs::read_link(bin_of(&root).join(POINTER)).unwrap(),
            h[0]
        );
        publish(&h[1]).unwrap();
        assert_eq!(
            std::fs::read_link(bin_of(&root).join(POINTER)).unwrap(),
            h[1]
        );
        let leftovers: Vec<_> = std::fs::read_dir(bin_of(&root))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&format!(".{POINTER}.tmp.")))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temporary link left: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pointer_is_never_missing_while_it_is_replaced() {
        let (root, h) = home_with_builds("atomic", &["a", "b"]);
        let pointer = bin_of(&root).join(POINTER);
        publish(&h[0]).unwrap();
        let done = std::sync::atomic::AtomicBool::new(false);
        let missing = std::thread::scope(|s| {
            let reader = s.spawn(|| {
                let mut missing = 0usize;
                while !done.load(std::sync::atomic::Ordering::SeqCst) {
                    if std::fs::read_link(&pointer).is_err() {
                        missing += 1;
                    }
                }
                missing
            });
            for i in 0..300 {
                publish(&h[i % 2]).unwrap();
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            reader.join().unwrap()
        });
        assert_eq!(missing, 0, "the pointer was absent during a replacement");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_outside_the_layout_publishes_nothing() {
        let root = std::env::temp_dir().join(format!("mrs-launch-dev-{}", std::process::id()));
        let exe = root
            .join("target")
            .join("debug")
            .join("x")
            .join("marsad-agent");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        assert_eq!(publish(&exe).unwrap(), None);
        assert_eq!(launcher_of(&exe), None);
        assert!(!root.join("target").join("bin").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_daemon_reports_the_pointer_only_when_it_resolves_to_itself() {
        let (root, h) = home_with_builds("launcher", &["old", "new"]);
        let pointer = bin_of(&root).join(POINTER);
        assert_eq!(launcher_of(&h[1]), None, "no pointer yet");
        publish(&h[0]).unwrap();
        assert_eq!(launcher_of(&h[1]), None, "the pointer names another build");
        assert_eq!(launcher_of(&h[0]), Some(pointer.clone()));
        publish(&h[1]).unwrap();
        assert_eq!(
            launcher_of(&h[1]),
            Some(pointer),
            "the pointer names this build"
        );
        assert_eq!(
            launcher_of(&h[0]),
            None,
            "the older daemon does not claim it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pointer_names_the_build_it_targets() {
        let (root, h) = home_with_builds("target", &["old", "new"]);
        let bin = bin_of(&root);
        assert_eq!(pointer_build(&bin), None);
        publish(&h[0]).unwrap();
        assert_eq!(pointer_build(&bin).as_deref(), Some("old"));
        publish(&h[1]).unwrap();
        assert_eq!(pointer_build(&bin).as_deref(), Some("new"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
